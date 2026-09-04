//! 既読・フラグ・削除といったメッセージ状態の変更。既読/フラグはDBだけで完結するが、
//! 削除は検索インデックスにも及ぶ操作（`account_setup`同様、複数ストアにまたがる
//! 操作を一箇所にまとめる目的でここに置く）。

use crate::db::Connection;
use crate::db::messages;
use crate::search::{SearchError, SearchIndex};

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
            })
            .unwrap();
        search_index.commit().unwrap();
        assert_eq!(search_index.search("findable", 10).unwrap(), vec![id]);

        set_deleted(&conn, &search_index, id, true).unwrap();

        assert!(db_messages::get(&conn, id).unwrap().unwrap().is_deleted);
        assert!(search_index.search("findable", 10).unwrap().is_empty());
    }
}
