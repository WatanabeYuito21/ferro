use std::path::Path;

use rusqlite::Connection;

use crate::db::accounts::Account;
use crate::db::messages::{self, NewMessage};
use crate::db::now_unix;
use crate::mail::parse::parse_full;
use crate::maildir;
use crate::pop3::{Pop3Client, Pop3Error};
use crate::search::{IndexableMessage, SearchError, SearchIndex};

/// RETRをまとめて送ってから順に読む単位。実サーバーで大きすぎると
/// 切断されることがあるため、CLAUDE.mdの実測に基づき20件に固定している。
const RETR_BATCH_SIZE: usize = 20;

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
#[allow(clippy::too_many_arguments)]
pub fn sync_account_with_limit(
    conn: &Connection,
    maildir_base: &Path,
    account: &Account,
    password: &str,
    allow_plaintext: bool,
    limit: Option<u32>,
    search_index: &SearchIndex,
) -> Result<SyncSummary, SyncError> {
    let mut total_fetched = 0u32;
    let mut budget = limit;
    let mut reconnects = 0u32;

    loop {
        let mut client = Pop3Client::connect(
            &account.host,
            account.port,
            account.use_tls,
            allow_plaintext,
        )?;
        client.user(&account.username)?;
        client.pass(password)?;

        let server_uidls = client.uidl()?;
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
        let (fetched_now, session_broken) = fetch_and_store(
            conn,
            maildir_base,
            &mut client,
            account.id,
            &pending[..take],
            search_index,
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
        if reconnects > MAX_RECONNECTS {
            return Ok(SyncSummary {
                fetched: total_fetched,
                remaining,
                ended_early: true,
            });
        }
    }
}

/// `items`をRETR_BATCH_SIZE単位でパイプライン取得し、Maildirへの保存とDBへの
/// 挿入をバッチ単位のトランザクションでまとめて行う。
/// 戻り値は(このセッションで取得できた件数, セッションが切断されたか)。
/// 切断は`Pop3Error::Io`/`ConnectionClosed`でのみ検出し、それ以外のエラー
/// （プロトコルエラー等）は復旧不能として呼び出し側にそのまま伝播する。
fn fetch_and_store(
    conn: &Connection,
    maildir_base: &Path,
    client: &mut Pop3Client,
    account_id: i64,
    items: &[(u32, String)],
    search_index: &SearchIndex,
) -> Result<(u32, bool), SyncError> {
    let mut fetched = 0u32;

    for batch in items.chunks(RETR_BATCH_SIZE) {
        let msg_nums: Vec<u32> = batch.iter().map(|(num, _)| *num).collect();
        let results = match client.retr_batch(&msg_nums) {
            Ok(results) => results,
            Err(e) if is_disconnect(&e) => return Ok((fetched, true)),
            Err(e) => return Err(e.into()),
        };

        let tx = conn.unchecked_transaction()?;
        let mut indexed_in_batch = false;
        for ((_, uidl), result) in batch.iter().zip(results) {
            let raw = match result {
                Ok(raw) => raw,
                Err(e) if is_disconnect(&e) => {
                    tx.commit()?;
                    if indexed_in_batch {
                        search_index.commit()?;
                    }
                    return Ok((fetched, true));
                }
                Err(e) => return Err(e.into()),
            };

            maildir::store(maildir_base, account_id, uidl, &raw)?;

            let parsed = parse_full(&raw);
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
                    date_header: parsed.headers.date_header.unwrap_or_else(now_unix),
                    size_bytes: raw.len() as i64,
                },
            )?;

            if let Some(id) = new_id {
                let from = format!(
                    "{} {}",
                    parsed.headers.from_name.as_deref().unwrap_or(""),
                    parsed.headers.from_addr.as_deref().unwrap_or("")
                );
                search_index.index_message(&IndexableMessage {
                    id,
                    subject: parsed.headers.subject.as_deref().unwrap_or(""),
                    from: &from,
                    body: parsed.body.as_deref().unwrap_or(""),
                })?;
                indexed_in_batch = true;
            }
            fetched += 1;
        }
        tx.commit()?;
        if indexed_in_batch {
            search_index.commit()?;
        }
    }

    Ok((fetched, false))
}

fn is_disconnect(err: &Pop3Error) -> bool {
    matches!(err, Pop3Error::Io(_) | Pop3Error::ConnectionClosed)
}

#[cfg(test)]
mod tests;
