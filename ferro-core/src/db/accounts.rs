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
        "SELECT id, name, host, port, username, use_tls, created_at
         FROM accounts WHERE id = ?1",
        [id],
        row_to_account,
    )
    .optional()
}

pub fn list(conn: &Connection) -> rusqlite::Result<Vec<Account>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, host, port, username, use_tls, created_at
         FROM accounts ORDER BY id",
    )?;
    stmt.query_map([], row_to_account)?.collect()
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
