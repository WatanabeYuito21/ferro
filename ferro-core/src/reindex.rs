use std::path::Path;
use std::time::Duration;

use crate::db::Connection;
use crate::db::messages;
use crate::mail::parse::extract_plain_text_body;
use crate::maildir;
use crate::search::{IndexableMessage, SearchIndex, SearchError};

#[derive(Debug, thiserror::Error)]
pub enum ReindexError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Search(#[from] SearchError),
}

const PAGE_SIZE: u32 = 1000;

/// commit()はWindows実機で断続的に(tantivyのマージ/GCワーカースレッドが異常終了して)
/// 失敗することがある（`SearchIndex::commit`のドキュメント参照）。1バッチ分の
/// index_message+commitをまとめてリトライすることで、この一過性の失敗を吸収する。
/// 原因（おそらくアンチウイルスのリアルタイムスキャン等によるファイルI/O競合）が
/// 数百ms単位で解消することがあるため、指数バックオフを挟む。
const MAX_COMMIT_ATTEMPTS: u32 = 5;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

fn with_commit_retry<T>(mut f: impl FnMut() -> crate::search::Result<T>) -> crate::search::Result<T> {
    let mut last_err = None;
    for attempt in 0..MAX_COMMIT_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_BASE_DELAY * attempt);
        }
        match f() {
            Ok(value) => return Ok(value),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.expect("loop runs MAX_COMMIT_ATTEMPTS >= 1 times"))
}

/// 全メッセージから検索インデックスを作り直す。
///
/// 索引は完全にSQLite/Maildirから再構築可能な派生キャッシュという設計方針どおり、
/// 既存インデックスの中身を`clear`してから全件入れ直す（差分更新はしない）。
/// `db::messages::list_all_for_reindex`のidベースページネーションで、
/// 1000万件規模でも一度に全件をメモリに載せずに舐める。
pub fn reindex_all(
    conn: &Connection,
    maildir_base: &Path,
    index: &SearchIndex,
) -> Result<usize, ReindexError> {
    with_commit_retry(|| index.clear())?;

    let mut count = 0usize;
    let mut after_id = 0i64;
    loop {
        let page = messages::list_all_for_reindex(conn, after_id, PAGE_SIZE)?;
        if page.is_empty() {
            break;
        }
        after_id = page.last().expect("page checked non-empty above").id;

        let indexed_in_page = with_commit_retry(|| -> crate::search::Result<usize> {
            let mut indexed = 0usize;
            for message in &page {
                // Maildirに実体が無い(削除済み等)メッセージはスキップする。
                let Ok(raw) = maildir::load(maildir_base, message.account_id, &message.uidl)
                else {
                    continue;
                };
                let body = extract_plain_text_body(&raw).unwrap_or_default();
                let from = format!(
                    "{} {}",
                    message.from_name.as_deref().unwrap_or(""),
                    message.from_addr.as_deref().unwrap_or("")
                );

                index.index_message(&IndexableMessage {
                    id: message.id,
                    subject: message.subject.as_deref().unwrap_or(""),
                    from: &from,
                    body: &body,
                })?;
                indexed += 1;
            }
            index.commit()?;
            Ok(indexed)
        })?;
        count += indexed_in_page;
    }

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::{self as db_messages, NewMessage};
    use crate::db::open_in_memory;

    #[test]
    fn reindex_all_rebuilds_from_db_and_maildir() {
        let conn = open_in_memory().unwrap();
        let maildir_dir = tempfile::tempdir().unwrap();
        let index = SearchIndex::create_in_ram().unwrap();

        let account_id = accounts::insert(
            &conn,
            &NewAccount {
                name: "test",
                host: "pop.example.com",
                port: 995,
                username: "user",
                use_tls: true,
            },
        )
        .unwrap();

        let raw = b"Subject: Findable\r\nFrom: Alice <alice@example.com>\r\n\r\nsearchable body";
        maildir::store(maildir_dir.path(), account_id, "u1", raw).unwrap();
        db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "u1",
                message_id_header: None,
                subject: Some("Findable"),
                from_name: Some("Alice"),
                from_addr: Some("alice@example.com"),
                to_addr: None,
                date_header: 1000,
                size_bytes: raw.len() as i64,
            },
        )
        .unwrap();

        let count = reindex_all(&conn, maildir_dir.path(), &index).unwrap();
        assert_eq!(count, 1);
        assert_eq!(index.search("findable", 10).unwrap().len(), 1);
        assert_eq!(index.search("searchable", 10).unwrap().len(), 1);
        assert_eq!(index.search("alice", 10).unwrap().len(), 1);
    }

    #[test]
    fn reindex_all_skips_messages_missing_from_maildir() {
        let conn = open_in_memory().unwrap();
        let maildir_dir = tempfile::tempdir().unwrap();
        let index = SearchIndex::create_in_ram().unwrap();

        let account_id = accounts::insert(
            &conn,
            &NewAccount {
                name: "test",
                host: "pop.example.com",
                port: 995,
                username: "user",
                use_tls: true,
            },
        )
        .unwrap();

        // Maildirに実体を置かないまま、DB行だけ作る（壊れた/欠損した状態を模擬）。
        db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "missing",
                message_id_header: None,
                subject: Some("Ghost"),
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 1000,
                size_bytes: 0,
            },
        )
        .unwrap();

        let count = reindex_all(&conn, maildir_dir.path(), &index).unwrap();
        assert_eq!(count, 0);
    }
}
