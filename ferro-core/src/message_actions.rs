//! 既読・フラグ・削除といったメッセージ状態の変更。既読/フラグはDBだけで完結するが、
//! 削除は検索インデックスにも及ぶ操作（`account_setup`同様、複数ストアにまたがる
//! 操作を一箇所にまとめる目的でここに置く）。

use std::path::Path;

use crate::db::Connection;
use crate::db::messages;
use crate::maildir;
use crate::search::{SearchError, SearchIndex, with_commit_retry};

#[derive(Debug, thiserror::Error)]
pub enum MessageActionError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Search(#[from] SearchError),
}

/// 論理削除する/復元する。POP3サーバー側のDELEとは独立したローカルの削除フラグ。
///
/// 削除時は検索インデックスからも取り除く（DBのフラグだけ立てても、`ferro reindex`
/// するまで検索結果に残ってしまうため）。復元時は検索インデックスへの再投入は
/// 行わない（本文を読み直す必要があり、ここでは扱わない）。検索結果に出したい
/// 場合は`ferro reindex`で作り直すこと。
pub fn set_deleted(
    conn: &Connection,
    search_index: &SearchIndex,
    id: i64,
    is_deleted: bool,
) -> Result<(), MessageActionError> {
    messages::set_deleted(conn, id, is_deleted)?;
    if is_deleted {
        search_index.delete_message(id)?;
        search_index.commit()?;
    }
    Ok(())
}

/// メール保持期間（`settings::Settings::retention_days`）が切れたメッセージを
/// 1バッチ分（最大`limit`件、`date_header`が古い順）完全に削除する。
/// `set_deleted`の論理削除と違い、DB行・Maildirファイル・検索インデックスの
/// 全てから消す（ディスクを実際に回収するため）。スター付きは対象外
/// （`db::messages::list_older_than`参照）。
///
/// 戻り値は実際に削除した件数。0が返ったら`cutoff`より古い削除対象がもう無い
/// ことを意味する（呼び出し側はこれを目印にループを止める。`reindex::catch_up_unindexed`
/// と同じ「1回で全部やらずバッチを繰り返す」設計）。
///
/// Tantivyへの反映はバッチ全体を`with_commit_retry`で一括処理し、それでも
/// 失敗する場合は1件ずつのフォールバックに切り替える
/// （`reindex::index_batch_with_fallback`と同じ理由。`list_older_than`は
/// 常に「日付が古い順のN件」を返すため、丸ごと失敗を伝播させて呼び出し側が
/// バッチ全体を諦める実装だと、特定のバッチが繰り返し失敗した場合にそれより
/// 新しい期限切れメッセージへ永久に手が届かなくなる。実際に検索インデックスの
/// キャッチアップで踏んだのと同じ罠なので、削除処理でも同じ対策を入れておく）。
/// 個別に失敗したメッセージはDB行・Maildirファイルもまだ消さないので、
/// 次回の呼び出しで安全にやり直せる。
pub fn purge_expired_batch(
    conn: &Connection,
    maildir_base: &Path,
    search_index: &SearchIndex,
    cutoff: i64,
    limit: u32,
) -> Result<usize, MessageActionError> {
    let expired = messages::list_older_than(conn, cutoff, limit)?;
    if expired.is_empty() {
        return Ok(0);
    }

    let whole_batch = with_commit_retry(|| -> crate::search::Result<()> {
        for message in &expired {
            search_index.delete_message(message.id)?;
        }
        search_index.commit()
    });

    let removable: Vec<&messages::Message> = if whole_batch.is_ok() {
        expired.iter().collect()
    } else {
        expired
            .iter()
            .filter(|message| {
                with_commit_retry(|| -> crate::search::Result<()> {
                    search_index.delete_message(message.id)?;
                    search_index.commit()
                })
                .is_ok()
            })
            .collect()
    };

    for message in &removable {
        // Maildirファイルが既に無い場合等はベストエフォートで無視する
        // （`account_setup::remove`と同じ理由）。
        let _ = maildir::remove(maildir_base, message.account_id, &message.uidl);
    }

    let ids: Vec<i64> = removable.iter().map(|m| m.id).collect();
    messages::delete_by_ids(conn, &ids)?;
    Ok(ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::{self as db_messages, NewMessage};
    use crate::db::open_in_memory;

    #[test]
    fn set_deleted_true_removes_from_both_db_listing_and_search_index() {
        let conn = open_in_memory().unwrap();
        let search_index = SearchIndex::create_in_ram().unwrap();

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
        let id = db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "u1",
                message_id_header: None,
                subject: Some("findable"),
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

        search_index
            .index_message(&crate::search::IndexableMessage {
                id,
                subject: "findable",
                from: "",
                body: "",
                date_header: 1000,
            })
            .unwrap();
        search_index.commit().unwrap();
        assert_eq!(search_index.search("findable", 10).unwrap(), vec![id]);

        set_deleted(&conn, &search_index, id, true).unwrap();

        assert!(db_messages::get(&conn, id).unwrap().unwrap().is_deleted);
        assert!(search_index.search("findable", 10).unwrap().is_empty());
    }

    fn make_account(conn: &Connection) -> i64 {
        accounts::insert(
            conn,
            &NewAccount {
                name: "test",
                host: "pop.example.com",
                port: 995,
                username: "user",
                use_tls: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn purge_expired_batch_removes_db_row_maildir_file_and_search_entry() {
        let conn = open_in_memory().unwrap();
        let maildir_dir = tempfile::tempdir().unwrap();
        let search_index = SearchIndex::create_in_ram().unwrap();
        let account_id = make_account(&conn);

        let raw = b"Subject: old alert\r\n\r\nbody";
        crate::maildir::store(maildir_dir.path(), account_id, "old", raw).unwrap();
        let id = db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "old",
                message_id_header: None,
                subject: Some("old alert"),
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 100,
                size_bytes: raw.len() as i64,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap()
        .unwrap();
        search_index
            .index_message(&crate::search::IndexableMessage {
                id,
                subject: "old alert",
                from: "",
                body: "body",
                date_header: 100,
            })
            .unwrap();
        search_index.commit().unwrap();

        let purged = purge_expired_batch(&conn, maildir_dir.path(), &search_index, 500, 500).unwrap();

        assert_eq!(purged, 1);
        assert!(db_messages::get(&conn, id).unwrap().is_none());
        assert!(search_index.search("alert", 10).unwrap().is_empty());
        assert!(!crate::maildir::exists(maildir_dir.path(), account_id, "old"));

        // 拾うものが無くなれば以後は0を返し続ける。
        assert_eq!(
            purge_expired_batch(&conn, maildir_dir.path(), &search_index, 500, 500).unwrap(),
            0
        );
    }

    #[test]
    fn purge_expired_batch_never_removes_starred_messages() {
        let conn = open_in_memory().unwrap();
        let maildir_dir = tempfile::tempdir().unwrap();
        let search_index = SearchIndex::create_in_ram().unwrap();
        let account_id = make_account(&conn);

        let raw = b"Subject: keep me\r\n\r\nbody";
        crate::maildir::store(maildir_dir.path(), account_id, "starred", raw).unwrap();
        let id = db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "starred",
                message_id_header: None,
                subject: Some("keep me"),
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 100,
                size_bytes: raw.len() as i64,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap()
        .unwrap();
        db_messages::set_flagged(&conn, id, true).unwrap();

        let purged = purge_expired_batch(&conn, maildir_dir.path(), &search_index, 500, 500).unwrap();

        assert_eq!(purged, 0);
        assert!(db_messages::get(&conn, id).unwrap().is_some());
        assert!(crate::maildir::exists(maildir_dir.path(), account_id, "starred"));
    }
}
