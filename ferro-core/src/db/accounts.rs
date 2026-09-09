use rusqlite::{Connection, OptionalExtension, Row, params};

use super::now_unix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub use_tls: bool,
    pub created_at: i64,
    /// UIDLコマンドに対応しているサーバーか。`None`は未判定（次回同期時に
    /// まず試す）。一部のレンタルサーバー等はUIDLに対応しておらず、送ると
    /// 接続を切断してくる（実際に踏んだ）ため、`ferro_core::sync`が一度
    /// 失敗を検出したら`Some(false)`を記録し、以後は最初からRETR後の内容
    /// ハッシュを代替uidlとして使うフォールバック経路に切り替える。
    pub uidl_supported: Option<bool>,
}

/// 新規アカウント作成時の入力。パスワードはここに含まない
/// （呼び出し側がkeyring crate経由でOS資格情報マネージャーに別途保存する）。
pub struct NewAccount<'a> {
    pub name: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub use_tls: bool,
}

pub fn insert(conn: &Connection, new: &NewAccount) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO accounts (name, host, port, username, use_tls, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            new.name,
            new.host,
            new.port,
            new.username,
            new.use_tls,
            now_unix(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Account>> {
    conn.query_row(
        "SELECT id, name, host, port, username, use_tls, created_at, uidl_supported
         FROM accounts WHERE id = ?1",
        [id],
        row_to_account,
    )
    .optional()
}

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Account>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, host, port, username, use_tls, created_at, uidl_supported
         FROM accounts ORDER BY id",
    )?;
    stmt.query_map([], row_to_account)?.collect()
}

/// `accounts.toml`との照合用。`name`には一意インデックスがあるので高々1件。
pub fn find_by_name(conn: &Connection, name: &str) -> rusqlite::Result<Option<Account>> {
    conn.query_row(
        "SELECT id, name, host, port, username, use_tls, created_at, uidl_supported
         FROM accounts WHERE name = ?1",
        [name],
        row_to_account,
    )
    .optional()
}

/// `ferro_core::sync`がUIDLコマンドの成否を検出した後に記録する
/// （`Account::uidl_supported`のドキュメント参照）。
pub fn set_uidl_supported(conn: &Connection, id: i64, supported: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE accounts SET uidl_supported = ?1 WHERE id = ?2",
        params![supported, id],
    )?;
    Ok(())
}

/// 設定ファイルとの照合（`account_config::reconcile`）で、既存アカウントの
/// 接続設定を上書きするために使う。`id`/`name`/`created_at`は変えない
/// （`id`を変えないことでkeyring/messages/Maildirとの紐付けを保つ）。
pub fn update_settings(
    conn: &Connection,
    id: i64,
    host: &str,
    port: u16,
    username: &str,
    use_tls: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE accounts SET host = ?1, port = ?2, username = ?3, use_tls = ?4 WHERE id = ?5",
        params![host, port, username, use_tls, id],
    )?;
    Ok(())
}

/// アカウント行を削除する。主にkeyringへのパスワード保存失敗時のロールバック用
/// （アカウント行だけ先にコミットされ、認証情報のない中途半端な状態が残るのを防ぐ）。
pub fn delete(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM accounts WHERE id = ?1", [id])?;
    Ok(())
}

fn row_to_account(row: &Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        name: row.get(1)?,
        host: row.get(2)?,
        port: row.get::<_, i64>(3)? as u16,
        username: row.get(4)?,
        use_tls: row.get(5)?,
        created_at: row.get(6)?,
        uidl_supported: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::messages::{self, NewMessage};
    use crate::db::open_in_memory;

    #[test]
    fn insert_get_list_roundtrip() {
        let conn = open_in_memory().unwrap();

        let id = insert(
            &conn,
            &NewAccount {
                name: "Work",
                host: "pop.example.com",
                port: 995,
                username: "alice",
                use_tls: true,
            },
        )
        .unwrap();

        let account = get(&conn, id).unwrap().expect("account should exist");
        assert_eq!(account.name, "Work");
        assert_eq!(account.host, "pop.example.com");
        assert_eq!(account.port, 995);
        assert_eq!(account.username, "alice");
        assert!(account.use_tls);

        assert_eq!(list(&conn).unwrap(), vec![account]);
        assert!(get(&conn, id + 1).unwrap().is_none());
    }

    #[test]
    fn names_must_be_unique() {
        let conn = open_in_memory().unwrap();
        insert(
            &conn,
            &NewAccount {
                name: "Work",
                host: "pop.example.com",
                port: 995,
                username: "alice",
                use_tls: true,
            },
        )
        .unwrap();

        let result = insert(
            &conn,
            &NewAccount {
                name: "Work",
                host: "other.example.com",
                port: 110,
                username: "bob",
                use_tls: false,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn find_by_name_and_update_settings() {
        let conn = open_in_memory().unwrap();
        let id = insert(
            &conn,
            &NewAccount {
                name: "Work",
                host: "pop.example.com",
                port: 995,
                username: "alice",
                use_tls: true,
            },
        )
        .unwrap();

        let found = find_by_name(&conn, "Work").unwrap().expect("should find it");
        assert_eq!(found.id, id);
        assert!(find_by_name(&conn, "Nope").unwrap().is_none());

        update_settings(&conn, id, "new-host.example.com", 110, "alice2", false).unwrap();
        let updated = get(&conn, id).unwrap().unwrap();
        assert_eq!(updated.id, id, "id must not change");
        assert_eq!(updated.name, "Work", "name must not change");
        assert_eq!(updated.host, "new-host.example.com");
        assert_eq!(updated.port, 110);
        assert_eq!(updated.username, "alice2");
        assert!(!updated.use_tls);
    }

    /// accountsとmessagesの間には`ON DELETE CASCADE`を付けていない（FK参照）ため、
    /// メッセージが1件でもあるアカウントは`delete`単体では消せない。
    /// 呼び出し側（`account_setup::remove`）が先にそのアカウントのメッセージを
    /// 消す責務を負うことを、この失敗自体で明示しておく。
    #[test]
    fn delete_fails_with_foreign_key_violation_when_messages_still_reference_the_account() {
        let conn = open_in_memory().unwrap();
        let account_id = insert(
            &conn,
            &NewAccount {
                name: "Work",
                host: "pop.example.com",
                port: 995,
                username: "alice",
                use_tls: true,
            },
        )
        .unwrap();
        messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "u1",
                message_id_header: None,
                subject: None,
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

        assert!(delete(&conn, account_id).is_err());
        assert!(get(&conn, account_id).unwrap().is_some());
    }
}
