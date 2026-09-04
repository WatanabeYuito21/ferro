use std::path::Path;

pub use rusqlite::Connection;

pub mod accounts;
mod migrate;
pub mod messages;

/// DBファイルを開き、未適用のマイグレーションを適用してから返す。
/// CLIとGUI(Tauri)はどちらもこの関数を通して同じパスのDBを開く。
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    init(&conn)?;
    Ok(conn)
}

fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", true)?;
    migrate::run(conn)
}

#[cfg(test)]
pub(crate) fn open_in_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", true)?;
    migrate::run(&conn)?;
    Ok(conn)
}

pub(crate) fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_secs() as i64
}
