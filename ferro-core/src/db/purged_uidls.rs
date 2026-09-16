use rusqlite::{Connection, OptionalExtension, params};

use super::now_unix;

/// 保持期間切れで完全削除した(uidl)を記録する
/// (`message_actions::purge_expired_batch`から呼ぶ)。同じuidlを二重に記録しても
/// 実害が無いので`INSERT OR IGNORE`にしている。
pub fn mark_purged(conn: &Connection, account_id: i64, uidl: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO purged_uidls (account_id, uidl, purged_at) VALUES (?1, ?2, ?3)",
        params![account_id, uidl, now_unix()],
    )?;
    Ok(())
}

/// syncの差分計算用: 指定uidlが過去に保持期間切れで完全削除済みかどうか。
/// `messages::exists_by_uidl`と合わせて使うことで、サーバー側にまだ残っている
/// (DELEを送らない設計のため)削除済みメッセージを新着として再取得しないようにする。
pub fn is_purged(conn: &Connection, account_id: i64, uidl: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM purged_uidls WHERE account_id = ?1 AND uidl = ?2",
        params![account_id, uidl],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::open_in_memory;

    fn make_account(conn: &Connection) -> i64 {
        accounts::insert(
            conn,
            &NewAccount {
                name: "test",
                host: "pop.example.com",
                port: 110,
                username: "bob",
                use_tls: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn mark_purged_then_is_purged_round_trips() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);

        assert!(!is_purged(&conn, account_id, "u1").unwrap());
        mark_purged(&conn, account_id, "u1").unwrap();
        assert!(is_purged(&conn, account_id, "u1").unwrap());
        assert!(!is_purged(&conn, account_id, "u2").unwrap());
    }

    #[test]
    fn mark_purged_is_idempotent() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);

        mark_purged(&conn, account_id, "u1").unwrap();
        mark_purged(&conn, account_id, "u1").unwrap();
        assert!(is_purged(&conn, account_id, "u1").unwrap());
    }

    #[test]
    fn removing_the_account_cascades_to_its_purged_uidls() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        mark_purged(&conn, account_id, "u1").unwrap();

        accounts::delete(&conn, account_id).unwrap();

        assert!(!is_purged(&conn, account_id, "u1").unwrap());
    }
}
