use std::path::Path;

use crate::db::Connection;
use crate::db::messages;
use crate::mail::parse::extract_plain_text_body;
use crate::maildir;
use crate::search::{IndexableMessage, SearchError, SearchIndex, with_commit_retry};

#[derive(Debug, thiserror::Error)]
pub enum ReindexError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Search(#[from] SearchError),
}

const PAGE_SIZE: u32 = 1000;

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

        let indexed_ids = index_batch_with_fallback(index, maildir_base, &page);
        messages::mark_indexed(conn, &indexed_ids)?;
        count += indexed_ids.len();
    }

    Ok(count)
}

/// バッチ全体を一括で`index_message`+`commit`する（従来どおり、成功すればこれが
/// 一番速い）。Tantivy `IndexWriter`クラッシュ（`SearchIndex::commit`のドキュメント
/// 参照）で`with_commit_retry`のリトライを使い切って全体が失敗した場合は、
/// 1件ずつの投入にフォールバックし、成功した分だけを返す。
///
/// これが要る理由: 以前は「バッチ全体が失敗したら丸ごと諦める」実装だったため、
/// 実機で特定のバッチが繰り返し失敗し続けるケース（`list_unindexed`は常に
/// 同じ「先頭からN件」を返すので、丸ごと失敗が続く限り永久に同じ集合を
/// 再試行し続ける）で、それより後ろにある未投入メッセージに一生手が届かず、
/// 検索できないメッセージが数万件規模で溜まり続けるという実害が出た。
/// 1件ずつのフォールバックなら、その中の大半（クラッシュの原因ではない
/// メッセージ）は普通に投入できる。
fn index_batch_with_fallback(
    index: &SearchIndex,
    maildir_base: &Path,
    page: &[messages::Message],
) -> Vec<i64> {
    let whole_batch = with_commit_retry(|| -> crate::search::Result<Vec<i64>> {
        let mut indexed_ids = Vec::new();
        for message in page {
            // Maildirに実体が無い(削除済み等)メッセージはスキップする。
            let Ok(raw) = maildir::load(maildir_base, message.account_id, &message.uidl) else {
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
            indexed_ids.push(message.id);
        }
        index.commit()?;
        Ok(indexed_ids)
    });

    if let Ok(indexed_ids) = whole_batch {
        return indexed_ids;
    }

    let mut indexed_ids = Vec::new();
    for message in page {
        let Ok(raw) = maildir::load(maildir_base, message.account_id, &message.uidl) else {
            continue;
        };
        let body = extract_plain_text_body(&raw).unwrap_or_default();
        let from = format!(
            "{} {}",
            message.from_name.as_deref().unwrap_or(""),
            message.from_addr.as_deref().unwrap_or("")
        );

        let result = with_commit_retry(|| -> crate::search::Result<()> {
            index.index_message(&IndexableMessage {
                id: message.id,
                subject: message.subject.as_deref().unwrap_or(""),
                from: &from,
                body: &body,
            })?;
            index.commit()?;
            Ok(())
        });
        if result.is_ok() {
            indexed_ids.push(message.id);
        }
    }
    indexed_ids
}

/// syncのバッチcommitがTantivyの`IndexWriter`クラッシュ（`SearchIndex::commit`の
/// ドキュメント参照）でリトライを使い切って失敗した場合、そのバッチのメッセージは
/// DB/Maildirには保存済みだが検索インデックスには入らないまま残る
/// (`fts_indexed_at`がNULLのまま)。これを定期的に(背景同期のたびに)少量ずつ
/// 拾い直して投入する。`reindex_all`と違い既存インデックスをクリアせず、
/// 未投入分だけを追記する。
///
/// `after_id`は呼び出し側が前回の戻り値から引き継ぐカーソル。戻り値の
/// `Some(id)`は「このidまでは見た」ことを意味し、呼び出し側は次回そのidを
/// `after_id`として渡すことで、このバッチが(部分的にでも)失敗していても
/// 後続の未投入メッセージへ前進できる。`None`は「これ以上未投入メッセージが
/// 無い」ことを意味し、呼び出し側は次回`after_id=0`から周回し直すとよい
/// （取りこぼした分に、状況が変わった後で再度チャンスを与えるため）。
pub fn catch_up_unindexed(
    conn: &Connection,
    maildir_base: &Path,
    index: &SearchIndex,
    after_id: i64,
) -> Result<(usize, Option<i64>), ReindexError> {
    const BATCH_SIZE: u32 = 500;
    let page = messages::list_unindexed(conn, after_id, BATCH_SIZE)?;
    if page.is_empty() {
        return Ok((0, None));
    }
    let last_id = page.last().expect("page checked non-empty above").id;

    let indexed_ids = index_batch_with_fallback(index, maildir_base, &page);
    messages::mark_indexed(conn, &indexed_ids)?;
    Ok((indexed_ids.len(), Some(last_id)))
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
                attachment_count: 0,
                preview: None,
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
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap();

        let count = reindex_all(&conn, maildir_dir.path(), &index).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn reindex_all_marks_messages_as_indexed() {
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

        let raw = b"Subject: Hi\r\n\r\nbody";
        maildir::store(maildir_dir.path(), account_id, "u1", raw).unwrap();
        db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "u1",
                message_id_header: None,
                subject: Some("Hi"),
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 1000,
                size_bytes: raw.len() as i64,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap();

        reindex_all(&conn, maildir_dir.path(), &index).unwrap();
        assert!(db_messages::list_unindexed(&conn, 0, 10).unwrap().is_empty());
    }

    #[test]
    fn catch_up_unindexed_indexes_and_marks_previously_missed_messages() {
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
        let id = db_messages::insert_new(
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
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap()
        .unwrap();

        // sync中にcommitがリトライを使い切って失敗した状態を模している:
        // DB/Maildirには保存済みだがfts_indexed_atはNULLのまま(insert_new直後は常にこう)。
        assert_eq!(db_messages::list_unindexed(&conn, 0, 10).unwrap().len(), 1);

        let (indexed, next_after_id) = catch_up_unindexed(&conn, maildir_dir.path(), &index, 0).unwrap();
        assert_eq!(indexed, 1);
        assert_eq!(next_after_id, Some(id));
        assert_eq!(index.search("findable", 10).unwrap(), vec![id]);
        assert!(db_messages::list_unindexed(&conn, 0, 10).unwrap().is_empty());

        // 2回目はもう拾うものが無い。
        assert_eq!(
            catch_up_unindexed(&conn, maildir_dir.path(), &index, 0).unwrap(),
            (0, None)
        );
    }

    /// あるバッチが(Tantivy側の事情で)丸ごと失敗し続けても、`after_id`を
    /// 進めていけば後続の未投入メッセージには手が届く、という設計の要（バグの
    /// 再発防止）。ここでは実際にTantivyを壊す代わりに、`list_unindexed`
    /// 自体が`after_id`で先頭集合を素通りできることを直接確認する
    /// （`index_batch_with_fallback`の1件ずつフォールバックと組み合わさることで、
    /// 呼び出し側が同じ「先頭からN件」に永久に足止めされなくなる）。
    #[test]
    fn catch_up_unindexed_advances_past_already_seen_messages_even_if_nothing_new_is_indexed() {
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

        // Maildirに実体が無い(index_batch_with_fallbackが素通りする)メッセージを2件。
        let first = db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "missing-1",
                message_id_header: None,
                subject: Some("Ghost 1"),
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 1000,
                size_bytes: 0,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap()
        .unwrap();
        db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "missing-2",
                message_id_header: None,
                subject: Some("Ghost 2"),
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 1001,
                size_bytes: 0,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap()
        .unwrap();

        // 何も投入できなくても(indexed == 0)、カーソルは末尾まで進む
        // （呼び出し側が同じ2件に永久に足止めされない）。
        let (indexed, next_after_id) = catch_up_unindexed(&conn, maildir_dir.path(), &index, 0).unwrap();
        assert_eq!(indexed, 0);
        assert!(next_after_id.is_some());
        assert!(next_after_id.unwrap() >= first);
    }
}
