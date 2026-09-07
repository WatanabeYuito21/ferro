use rusqlite::Connection;

struct Migration {
    version: i64,
    sql: &'static str,
}

/// バージョン番号順に並んだ全マイグレーション。
/// 新しいマイグレーションを追加するときは末尾に足すだけでよく、
/// 既存のSQLファイルは変更しない（スキーマ変更は新しいファイルを追加する）。
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("../../migrations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("../../migrations/0002_accounts_name_unique.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("../../migrations/0003_folders_and_labels.sql"),
    },
    Migration {
        version: 4,
        sql: include_str!("../../migrations/0004_unindexed_lookup_index.sql"),
    },
];

/// 未適用のマイグレーションを`schema_migrations`テーブルの記録に基づいて適用する。
/// 何度呼んでも安全（適用済みバージョンはスキップされる）。
pub fn run(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
        )",
    )?;

    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    for migration in MIGRATIONS.iter().filter(|m| m.version > current) {
        conn.execute_batch(migration.sql)?;
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, strftime('%s', 'now'))",
            [migration.version],
        )?;
    }

    Ok(())
}
