use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use crate::db::accounts::{self, Account};
use crate::db::messages::{self, NewMessage};
use crate::db::now_unix;
use crate::mail::attachments;
use crate::mail::parse::{make_preview, parse_full};
use crate::maildir;
use crate::maildir::hash::fnv1a64;
use crate::pop3::{Pop3Client, Pop3Error};
use crate::search::{IndexableMessage, SearchError, SearchIndex, with_commit_retry};

/// RETRをまとめて送ってから順に読む単位。実サーバーで大きすぎると
/// 切断されることがあるため、CLAUDE.mdの実測に基づき20件に固定している。
const RETR_BATCH_SIZE: usize = 20;

/// 検索インデックスへの`commit`をまとめる単位。RETR_BATCH_SIZEとは独立に、
/// これだけメッセージが溜まってから初めてTantivyへcommitする。
/// Tantivyの`IndexWriter`クラッシュ（`with_commit_retry`のドキュメント参照）は
/// 低確率とはいえcommit呼び出し1回ごとに当たりうるため、RETR_BATCH_SIZE(20件)
/// ごとにcommitしていると数万件規模のメールボックスでは数千回commitすることになり、
/// どこかでリトライを使い切る確率が積み重なってしまう（実機で実際に踏んだ）。
/// commit頻度自体をここで大きく減らし、遭遇回数そのものを減らす。
const SEARCH_COMMIT_BATCH_SIZE: usize = 500;

/// 1回の同期呼び出しで許容する再接続回数の上限。これを超えたら諦めて
/// 途中経過を返す（UIDL差分方式なので次回のsync呼び出しで安全に続きから拾われる）。
const MAX_RECONNECTS: u32 = 5;

/// 再接続前に待機する基準時間（試行回数に比例させる簡易的な指数バックオフ。
/// `search::with_commit_retry`と同じ考え方）。以前は再接続を待機無しで
/// 即座に行っており、実サーバー相手に短時間で6回連続接続を試みる格好に
/// なっていた（レンタルサーバー宛のアカウントで実際に踏んだ）。もし
/// サーバー側やその手前のファイアウォールが短時間の異常接続パターンを
/// 検知して防御的にブロックしているのだとしたら、間隔を空けずに再接続
/// し続けることはむしろ状況を悪化させかねないため、行儀の良いクライアントで
/// あるよう最低限の間隔を空ける。テストではこの待機自体を検証したいわけではなく、
/// 実時間で待つとテストスイートが遅くなるだけなので、テストビルドでは
/// 極小の値にする。
#[cfg(not(test))]
const RECONNECT_BASE_DELAY: Duration = Duration::from_secs(2);
#[cfg(test)]
const RECONNECT_BASE_DELAY: Duration = Duration::from_millis(1);

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Pop3(#[from] Pop3Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error("failed to store message on disk: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Search(#[from] SearchError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncSummary {
    /// この呼び出しで新規に取得・保存できた件数。
    pub fetched: u32,
    /// サーバー上にはあるがまだ取得できていない件数（次回のsyncで拾われる）。
    pub remaining: u32,
    /// 再接続を試みても復旧できず、この呼び出しを打ち切ったかどうか。
    pub ended_early: bool,
}

/// アカウントを同期する。UIDLの差分は全件計算するが、実際にRETRするのは
/// `limit`件まで（Noneなら無制限）。パイプライン化したRETR中に実サーバーが
/// 接続を切断することがあるため、その場合は再接続して続きから再開する
/// （UIDL差分方式のため、既に保存済みのメッセージは再取得されず安全）。
///
/// `on_progress(fetched_so_far, target_total)`は1バッチ（`RETR_BATCH_SIZE`件）
/// 完了するごとに呼ばれる。呼び出し側（GUI/CLI）が"30/1000"のような進捗表示に使う。
/// `target_total`は今回のsync呼び出しで取得予定の件数（`fetched_so_far`にこの回で
/// 既に取得できた分を加えたもの）で、再接続をまたぐと（UIDL差分を取り直すため）
/// 変わることがある。
#[allow(clippy::too_many_arguments)]
pub fn sync_account_with_limit(
    conn: &Connection,
    maildir_base: &Path,
    account: &Account,
    password: &str,
    allow_plaintext: bool,
    limit: Option<u32>,
    search_index: &SearchIndex,
    mut on_progress: impl FnMut(u32, u32),
) -> Result<SyncSummary, SyncError> {
    let mut total_fetched = 0u32;
    let mut budget = limit;
    let mut reconnects = 0u32;
    // 一部のPOP3サーバー（レンタルサーバー等）はUIDLコマンドに対応しておらず、
    // 送ると-ERRではなく接続そのものを切断してくる（実際に踏んだ）。
    // `Account::uidl_supported`のドキュメント参照。前回までに非対応と判明していれば
    // 今回はもうUIDLを試さず、最初からRETR後の内容ハッシュを代替uidlとして使う
    // フォールバック経路（`fetch_and_store_by_hash`）に入る。
    let mut uidl_known_unsupported = account.uidl_supported == Some(false);

    // CONNECT/USER/PASS/UIDLの段階での切断は、以前は`fetch_and_store`中のRETR
    // パイプライン切断と違って即座にエラーを返していた（他人のレンタルサーバー環境で
    // 実際に"connection closed by server"として踏んだ）。ここも同じ再接続予算
    // （`MAX_RECONNECTS`）を共有してリトライする。ハンドシェイク段階では
    // まだ何も取得できていないため、予算を使い切った場合はfetch_and_store側の
    // ような`ended_early`付きの部分成功では表現できず、そのままエラーを返す。
    macro_rules! try_or_reconnect {
        ($stage:literal, $e:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) if is_disconnect(&e) => {
                    reconnects += 1;
                    let give_up = reconnects > MAX_RECONNECTS;
                    crate::logging::log_line(&format!(
                        "sync: disconnected during {} (account={:?} host={}:{} reconnect={}/{}{}): {e}",
                        $stage,
                        account.name,
                        account.host,
                        account.port,
                        reconnects,
                        MAX_RECONNECTS,
                        if give_up { ", giving up" } else { "" },
                    ));
                    if give_up {
                        return Err(e.into());
                    }
                    std::thread::sleep(RECONNECT_BASE_DELAY * reconnects);
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        };
    }

    loop {
        let mut client = try_or_reconnect!(
            "CONNECT",
            Pop3Client::connect(&account.host, account.port, account.use_tls, allow_plaintext,)
        );
        try_or_reconnect!("USER", client.user(&account.username));
        try_or_reconnect!("PASS", client.pass(password));

        if uidl_known_unsupported {
            let items = try_or_reconnect!("LIST", client.list());
            if items.is_empty() {
                return Ok(SyncSummary {
                    fetched: total_fetched,
                    remaining: 0,
                    ended_early: false,
                });
            }

            let take = budget.map_or(items.len(), |b| (b as usize).min(items.len()));
            let target_total = total_fetched + take as u32;
            let msg_nums: Vec<u32> = items[..take].iter().map(|(num, _)| *num).collect();
            let (new_count, attempted, session_broken) = fetch_and_store_by_hash(
                conn,
                maildir_base,
                &mut client,
                account.id,
                &msg_nums,
                search_index,
                total_fetched,
                target_total,
                &mut on_progress,
            )?;
            let _ = client.quit();

            total_fetched += new_count;
            if let Some(b) = budget.as_mut() {
                *b = b.saturating_sub(attempted);
            }

            // `items.len()`（budgetで絞る前の全件数）を使う。`fetch_and_store`側の
            // `remaining = pending.len() - fetched_now`と同じ考え方
            // （budget対象外だった分・切断で未処理だった分の両方を含める）。
            let remaining = (items.len() - attempted as usize) as u32;
            if !session_broken {
                return Ok(SyncSummary {
                    fetched: total_fetched,
                    remaining,
                    ended_early: false,
                });
            }

            reconnects += 1;
            let give_up = reconnects > MAX_RECONNECTS;
            crate::logging::log_line(&format!(
                "sync: disconnected mid-fetch (no-UIDL fallback) (account={:?} host={}:{} reconnect={}/{}{}), fetched_so_far={total_fetched} remaining={remaining}",
                account.name,
                account.host,
                account.port,
                reconnects,
                MAX_RECONNECTS,
                if give_up { ", giving up" } else { "" },
            ));
            if give_up {
                return Ok(SyncSummary {
                    fetched: total_fetched,
                    remaining,
                    ended_early: true,
                });
            }
            std::thread::sleep(RECONNECT_BASE_DELAY * reconnects);
            continue;
        }

        let server_uidls = match client.uidl() {
            Ok(uidls) => {
                if account.uidl_supported != Some(true) {
                    let _ = accounts::set_uidl_supported(conn, account.id, true);
                }
                uidls
            }
            Err(e) if is_disconnect(&e) => {
                // -ERRではなく接続切断という形でUIDL非対応を表現するサーバーが実在する
                // （`Account::uidl_supported`参照）。以後はこのアカウントに対して
                // 二度とUIDLを試さず、フォールバック経路へ切り替える。
                let _ = accounts::set_uidl_supported(conn, account.id, false);
                uidl_known_unsupported = true;
                reconnects += 1;
                let give_up = reconnects > MAX_RECONNECTS;
                crate::logging::log_line(&format!(
                    "sync: UIDL not supported by server, switching to content-hash fallback \
                     (account={:?} host={}:{} reconnect={}/{}{}): {e}",
                    account.name,
                    account.host,
                    account.port,
                    reconnects,
                    MAX_RECONNECTS,
                    if give_up { ", giving up" } else { "" },
                ));
                if give_up {
                    return Err(e.into());
                }
                std::thread::sleep(RECONNECT_BASE_DELAY * reconnects);
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        let mut pending = Vec::new();
        for (msg_num, uidl) in server_uidls {
            if !messages::exists_by_uidl(conn, account.id, &uidl)? {
                pending.push((msg_num, uidl));
            }
        }

        if pending.is_empty() {
            return Ok(SyncSummary {
                fetched: total_fetched,
                remaining: 0,
                ended_early: false,
            });
        }

        let take = budget.map_or(pending.len(), |b| (b as usize).min(pending.len()));
        let target_total = total_fetched + take as u32;
        let (fetched_now, session_broken) = fetch_and_store(
            conn,
            maildir_base,
            &mut client,
            account.id,
            &pending[..take],
            search_index,
            total_fetched,
            target_total,
            &mut on_progress,
        )?;
        let _ = client.quit();

        total_fetched += fetched_now;
        if let Some(b) = budget.as_mut() {
            *b = b.saturating_sub(fetched_now);
        }

        let remaining = (pending.len() - fetched_now as usize) as u32;
        if !session_broken {
            return Ok(SyncSummary {
                fetched: total_fetched,
                remaining,
                ended_early: false,
            });
        }

        reconnects += 1;
        let give_up = reconnects > MAX_RECONNECTS;
        crate::logging::log_line(&format!(
            "sync: disconnected mid-fetch (account={:?} host={}:{} reconnect={}/{}{}), fetched_so_far={total_fetched} remaining={remaining}",
            account.name,
            account.host,
            account.port,
            reconnects,
            MAX_RECONNECTS,
            if give_up { ", giving up" } else { "" },
        ));
        if give_up {
            return Ok(SyncSummary {
                fetched: total_fetched,
                remaining,
                ended_early: true,
            });
        }
        std::thread::sleep(RECONNECT_BASE_DELAY * reconnects);
    }
}

/// 検索インデックス投入待ちのメッセージ1件分。`with_commit_retry`でバッチ丸ごと
/// 再実行できるよう、パース結果(`raw`を借用する`ParsedMail`)から切り離した
/// 所有データとして保持する。
struct PendingIndex {
    id: i64,
    subject: String,
    from: String,
    body: String,
    date_header: i64,
}

/// 溜まった`pending`を検索インデックスへ投入してcommitする
/// （`with_commit_retry`で自己修復＋リトライ）。成功したら`messages::mark_indexed`で
/// `fts_indexed_at`を記録してから`pending`を空にする（`with_commit_retry`が
/// リトライを使い切って失敗した場合はmark_indexedを呼ばず`fts_indexed_at`をNULLの
/// ままにしておくことで、`reindex::catch_up_unindexed`が後から自動的に拾い直せる）。
fn flush_pending_index(
    conn: &Connection,
    search_index: &SearchIndex,
    pending: &mut Vec<PendingIndex>,
) -> Result<(), SyncError> {
    if pending.is_empty() {
        return Ok(());
    }
    with_commit_retry(|| -> crate::search::Result<()> {
        for item in pending.iter() {
            search_index.index_message(&IndexableMessage {
                id: item.id,
                subject: &item.subject,
                from: &item.from,
                body: &item.body,
                date_header: item.date_header,
            })?;
        }
        search_index.commit()
    })?;
    let ids: Vec<i64> = pending.iter().map(|item| item.id).collect();
    messages::mark_indexed(conn, &ids)?;
    pending.clear();
    Ok(())
}

/// RETRで取得した1件をMaildir/DBへ保存する。`uidl`が既に保存済みなら
/// Maildirへの書き込み・DB挿入とも何もせず`None`を返す
/// （`fetch_and_store_by_hash`が、RETR前には重複かどうか分からない
/// フォールバック経路で使う。通常のUIDL差分経路では`pending`が事前に
/// 未取得分だけへ絞り込まれているため、実際にはここで弾かれることはない）。
fn store_fetched_message(
    conn: &Connection,
    maildir_base: &Path,
    account_id: i64,
    uidl: &str,
    raw: &[u8],
) -> Result<Option<PendingIndex>, SyncError> {
    // 既知のuidlならMaildirへの書き込み自体を丸ごとスキップする。
    // `fetch_and_store`（通常のUIDL差分経路）では呼び出し側が事前に
    // `exists_by_uidl`で絞り込み済みなのでここは基本的に常にfalseだが、
    // `fetch_and_store_by_hash`（UIDL非対応サーバー向けのフォールバック経路。
    // RETR前に重複かどうか分からないため全件RETRする）では、この事前チェックが
    // 無いと同期のたびに毎回メールボックス全体をMaildirへ書き直すことになる
    // （DB側は`insert_new`のINSERT OR IGNOREで重複排除できていたが、
    // ディスクI/Oは防げていなかった。実際にユーザーがファンの唸り/大量の
    // I/Oバイト数として踏んだ）。
    if messages::exists_by_uidl(conn, account_id, uidl)? {
        return Ok(None);
    }
    maildir::store(maildir_base, account_id, uidl, raw)?;

    let parsed = parse_full(raw);
    // 一覧に添付件数チップ/プレビュー行を表示するため、sync時に一度だけ計算して
    // 保存しておく(一覧描画のたびにMaildirを読み直すのはCLAUDE.mdの
    // 「起動時に何も舐めない」方針に反するため)。
    let attachment_count = attachments::list_attachments(raw).len() as i64;
    let preview = make_preview(parsed.body.as_deref());
    let date_header = parsed.headers.date_header.unwrap_or_else(now_unix);
    let new_id = messages::insert_new(
        conn,
        &NewMessage {
            account_id,
            uidl,
            message_id_header: parsed.headers.message_id.as_deref(),
            subject: parsed.headers.subject.as_deref(),
            from_name: parsed.headers.from_name.as_deref(),
            from_addr: parsed.headers.from_addr.as_deref(),
            to_addr: parsed.headers.to_addr.as_deref(),
            date_header,
            size_bytes: raw.len() as i64,
            attachment_count,
            preview: preview.as_deref(),
        },
    )?;

    Ok(new_id.map(|id| {
        let from = format!(
            "{} {}",
            parsed.headers.from_name.as_deref().unwrap_or(""),
            parsed.headers.from_addr.as_deref().unwrap_or("")
        );
        PendingIndex {
            id,
            subject: parsed.headers.subject.as_deref().unwrap_or("").to_string(),
            from,
            body: parsed.body.as_deref().unwrap_or("").to_string(),
            date_header,
        }
    }))
}

/// `items`をRETR_BATCH_SIZE単位でパイプライン取得し、Maildirへの保存とDBへの
/// 挿入をバッチ単位のトランザクションでまとめて行う。
/// 戻り値は(このセッションで取得できた件数, セッションが切断されたか)。
/// 切断は`Pop3Error::Io`/`ConnectionClosed`でのみ検出し、それ以外のエラー
/// （プロトコルエラー等）は復旧不能として呼び出し側にそのまま伝播する。
///
/// 検索インデックスへの投入はDBトランザクションのcommit後に`pending_index`へ
/// 貯め、`SEARCH_COMMIT_BATCH_SIZE`件溜まるごと・接続切断時・関数の終わりに
/// まとめて`flush_pending_index`でTantivyへcommitする
/// （`SEARCH_COMMIT_BATCH_SIZE`のドキュメント参照）。
#[allow(clippy::too_many_arguments)]
fn fetch_and_store(
    conn: &Connection,
    maildir_base: &Path,
    client: &mut Pop3Client,
    account_id: i64,
    items: &[(u32, String)],
    search_index: &SearchIndex,
    already_fetched: u32,
    target_total: u32,
    on_progress: &mut dyn FnMut(u32, u32),
) -> Result<(u32, bool), SyncError> {
    let mut fetched = 0u32;
    let mut pending_index: Vec<PendingIndex> = Vec::new();

    for batch in items.chunks(RETR_BATCH_SIZE) {
        let msg_nums: Vec<u32> = batch.iter().map(|(num, _)| *num).collect();
        let results = match client.retr_batch(&msg_nums) {
            Ok(results) => results,
            Err(e) if is_disconnect(&e) => {
                flush_pending_index(conn, search_index, &mut pending_index)?;
                return Ok((fetched, true));
            }
            Err(e) => return Err(e.into()),
        };

        let tx = conn.unchecked_transaction()?;
        let mut disconnected = false;
        for ((_, uidl), result) in batch.iter().zip(results) {
            let raw = match result {
                Ok(raw) => raw,
                Err(e) if is_disconnect(&e) => {
                    disconnected = true;
                    break;
                }
                Err(e) => return Err(e.into()),
            };

            if let Some(item) = store_fetched_message(&tx, maildir_base, account_id, uidl, &raw)? {
                pending_index.push(item);
            }
            fetched += 1;
        }
        tx.commit()?;

        if pending_index.len() >= SEARCH_COMMIT_BATCH_SIZE {
            flush_pending_index(conn, search_index, &mut pending_index)?;
        }

        on_progress(already_fetched + fetched, target_total);

        if disconnected {
            flush_pending_index(conn, search_index, &mut pending_index)?;
            return Ok((fetched, true));
        }
    }

    flush_pending_index(conn, search_index, &mut pending_index)?;
    Ok((fetched, false))
}

/// RETRした内容から`messages.uidl`代わりの識別子を作る。UIDLコマンドに
/// 対応していないサーバー向けのフォールバック（`fetch_and_store_by_hash`参照）。
/// FNV-1a(64bit)は`ferro_core::maildir::hash`がMaildirのディレクトリ分散に
/// 使っているのと同じ仕様が固定されたハッシュ関数で、衝突耐性を要する
/// セキュリティ用途ではなく単なる重複排除用途なのでこれで十分。
/// `"ferro-hash-v1-"`という接頭辞を付けているのは、(1)サーバー由来の本物の
/// UIDLと見た目で区別できるようにする、(2)将来ハッシュ方式を変える場合に
/// 版を分けられるようにするため。
fn content_uidl(raw: &[u8]) -> String {
    format!("ferro-hash-v1-{:016x}", fnv1a64(raw))
}

/// `msg_nums`をRETR_BATCH_SIZE単位でパイプライン取得し、`fetch_and_store`と
/// 同じくMaildir/DBへ保存する。UIDLコマンドに対応していないサーバー向けの
/// フォールバック経路で使う（`Account::uidl_supported`参照）。
///
/// `fetch_and_store`と異なり、呼び出し側はRETR前にどれが新着メッセージかを
/// 判定できない（UIDLが無いため）。そのため`msg_nums`は絞り込まれておらず
/// 全件が対象になり、既に保存済みの内容であっても再度RETRする（同じ内容の
/// ハッシュ値になるため`store_fetched_message`内の`INSERT OR IGNORE`で
/// 静かにスキップされ、DB/検索インデックスは重複しない。ただし通信量は
/// 毎回全件分かかる）。
///
/// 戻り値は`(新規に保存できた件数, RETRを試みた件数, セッションが切断されたか)`。
/// 前者は`SyncSummary::fetched`（「新規に取得・保存できた件数」）にそのまま
/// 積み上げるための値、後者は`items.len() - 試みた件数`で`remaining`
/// （まだ処理できていない件数）を計算するための値で、常に一致するとは限らない
/// （既に保存済みの内容を再RETRした場合、試みてはいるが新規件数には入らない）。
#[allow(clippy::too_many_arguments)]
fn fetch_and_store_by_hash(
    conn: &Connection,
    maildir_base: &Path,
    client: &mut Pop3Client,
    account_id: i64,
    msg_nums: &[u32],
    search_index: &SearchIndex,
    already_fetched: u32,
    target_total: u32,
    on_progress: &mut dyn FnMut(u32, u32),
) -> Result<(u32, u32, bool), SyncError> {
    let mut new_count = 0u32;
    let mut attempted = 0u32;
    let mut pending_index: Vec<PendingIndex> = Vec::new();

    for batch in msg_nums.chunks(RETR_BATCH_SIZE) {
        let results = match client.retr_batch(batch) {
            Ok(results) => results,
            Err(e) if is_disconnect(&e) => {
                flush_pending_index(conn, search_index, &mut pending_index)?;
                return Ok((new_count, attempted, true));
            }
            Err(e) => return Err(e.into()),
        };

        let tx = conn.unchecked_transaction()?;
        let mut disconnected = false;
        for result in results {
            let raw = match result {
                Ok(raw) => raw,
                Err(e) if is_disconnect(&e) => {
                    disconnected = true;
                    break;
                }
                Err(e) => return Err(e.into()),
            };

            let uidl = content_uidl(&raw);
            if let Some(item) = store_fetched_message(&tx, maildir_base, account_id, &uidl, &raw)? {
                pending_index.push(item);
                new_count += 1;
            }
            attempted += 1;
        }
        tx.commit()?;

        if pending_index.len() >= SEARCH_COMMIT_BATCH_SIZE {
            flush_pending_index(conn, search_index, &mut pending_index)?;
        }

        on_progress(already_fetched + attempted, target_total);

        if disconnected {
            flush_pending_index(conn, search_index, &mut pending_index)?;
            return Ok((new_count, attempted, true));
        }
    }

    flush_pending_index(conn, search_index, &mut pending_index)?;
    Ok((new_count, attempted, false))
}

fn is_disconnect(err: &Pop3Error) -> bool {
    matches!(err, Pop3Error::Io(_) | Pop3Error::ConnectionClosed)
}

#[cfg(test)]
mod tests;
