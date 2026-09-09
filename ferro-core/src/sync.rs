use std::path::Path;

use rusqlite::Connection;

use crate::db::accounts::Account;
use crate::db::messages::{self, NewMessage};
use crate::db::now_unix;
use crate::mail::attachments;
use crate::mail::parse::{make_preview, parse_full};
use crate::maildir;
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

    // CONNECT/USER/PASS/UIDLの段階での切断は、以前は`fetch_and_store`中のRETR
    // パイプライン切断と違って即座にエラーを返していた（他人のレンタルサーバー環境で
    // 実際に"connection closed by server"として踏んだ）。ここも同じ再接続予算
    // （`MAX_RECONNECTS`）を共有してリトライする。ハンドシェイク段階では
    // まだ何も取得できていないため、予算を使い切った場合はfetch_and_store側の
    // ような`ended_early`付きの部分成功では表現できず、そのままエラーを返す。
    macro_rules! try_or_reconnect {
        ($e:expr) => {
            match $e {
                Ok(v) => v,
                Err(e) if is_disconnect(&e) => {
                    reconnects += 1;
                    let give_up = reconnects > MAX_RECONNECTS;
                    crate::logging::log_line(&format!(
                        "sync: disconnected during handshake (account={:?} host={}:{} reconnect={}/{}{}): {e}",
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
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        };
    }

    loop {
        let mut client = try_or_reconnect!(Pop3Client::connect(
            &account.host,
            account.port,
            account.use_tls,
            allow_plaintext,
        ));
        try_or_reconnect!(client.user(&account.username));
        try_or_reconnect!(client.pass(password));

        let server_uidls = try_or_reconnect!(client.uidl());
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

            maildir::store(maildir_base, account_id, uidl, &raw)?;

            let parsed = parse_full(&raw);
            // 一覧に添付件数チップ/プレビュー行を表示するため、sync時に一度だけ計算して
            // 保存しておく(一覧描画のたびにMaildirを読み直すのはCLAUDE.mdの
            // 「起動時に何も舐めない」方針に反するため)。
            let attachment_count = attachments::list_attachments(&raw).len() as i64;
            let preview = make_preview(parsed.body.as_deref());
            let date_header = parsed.headers.date_header.unwrap_or_else(now_unix);
            let new_id = messages::insert_new(
                &tx,
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

            if let Some(id) = new_id {
                let from = format!(
                    "{} {}",
                    parsed.headers.from_name.as_deref().unwrap_or(""),
                    parsed.headers.from_addr.as_deref().unwrap_or("")
                );
                pending_index.push(PendingIndex {
                    id,
                    subject: parsed.headers.subject.as_deref().unwrap_or("").to_string(),
                    from,
                    body: parsed.body.as_deref().unwrap_or("").to_string(),
                    date_header,
                });
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

fn is_disconnect(err: &Pop3Error) -> bool {
    matches!(err, Pop3Error::Io(_) | Pop3Error::ConnectionClosed)
}

#[cfg(test)]
mod tests;
