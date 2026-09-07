//! 色付きラベル（タグ）。アカウントをまたいだグローバルな一覧で、メッセージとは
//! `message_labels`テーブルで多対多。`db::accounts`と同じ薄いCRUDパターン。

use std::collections::HashMap;

use rusqlite::{Connection, Row, ToSql, params};

use super::now_unix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelWithCount {
    pub label: Label,
    pub count: i64,
}

pub fn insert(conn: &Connection, name: &str, color: &str) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO labels (name, color, created_at) VALUES (?1, ?2, ?3)",
        params![name, color, now_unix()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// ラベル自体と、付与関係(`message_labels`)も合わせて削除する。
pub fn delete(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM message_labels WHERE label_id = ?1", [id])?;
    conn.execute("DELETE FROM labels WHERE id = ?1", [id])?;
    Ok(())
}

/// サイドバー表示用。ラベルごとの付与件数(論理削除済みメッセージは除く)も一緒に返す。
pub fn list_with_counts(conn: &Connection) -> rusqlite::Result<Vec<LabelWithCount>> {
    let mut stmt = conn.prepare(
        "SELECT labels.id, labels.name, labels.color,
                (SELECT COUNT(*) FROM message_labels
                 JOIN messages ON messages.id = message_labels.message_id
                 WHERE message_labels.label_id = labels.id AND messages.is_deleted = 0)
         FROM labels ORDER BY labels.name",
    )?;
    stmt.query_map([], |row| {
        Ok(LabelWithCount {
            label: row_to_label(row, 0)?,
            count: row.get(3)?,
        })
    })?
    .collect()
}

/// メッセージへのラベル付与/解除。
pub fn set_on_message(
    conn: &Connection,
    message_id: i64,
    label_id: i64,
    assigned: bool,
) -> rusqlite::Result<()> {
    if assigned {
        conn.execute(
            "INSERT OR IGNORE INTO message_labels (message_id, label_id) VALUES (?1, ?2)",
            params![message_id, label_id],
        )?;
    } else {
        conn.execute(
            "DELETE FROM message_labels WHERE message_id = ?1 AND label_id = ?2",
            params![message_id, label_id],
        )?;
    }
    Ok(())
}

/// 1件のメッセージに付いているラベル一覧。
pub fn for_message(conn: &Connection, message_id: i64) -> rusqlite::Result<Vec<Label>> {
    let mut stmt = conn.prepare(
        "SELECT labels.id, labels.name, labels.color
         FROM labels JOIN message_labels ON message_labels.label_id = labels.id
         WHERE message_labels.message_id = ?1
         ORDER BY labels.name",
    )?;
    stmt.query_map([message_id], |row| row_to_label(row, 0))?.collect()
}

/// 複数メッセージ分のラベルをまとめて取得する(一覧表示でメッセージ1件ごとに
/// クエリを打つN+1を避けるため)。`message_id -> そのメッセージのラベル一覧`のマップを返す。
pub fn for_messages(
    conn: &Connection,
    message_ids: &[i64],
) -> rusqlite::Result<HashMap<i64, Vec<Label>>> {
    let mut map = HashMap::new();
    if message_ids.is_empty() {
        return Ok(map);
    }

    let placeholders = message_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT message_labels.message_id, labels.id, labels.name, labels.color
         FROM message_labels JOIN labels ON labels.id = message_labels.label_id
         WHERE message_labels.message_id IN ({placeholders})"
    );
    let param_refs: Vec<&dyn ToSql> =
        message_ids.iter().map(|id| id as &dyn ToSql).collect();

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(param_refs.as_slice(), |row| {
        Ok((row.get::<_, i64>(0)?, row_to_label(row, 1)?))
    })?;
    for row in rows {
        let (message_id, label) = row?;
        map.entry(message_id).or_insert_with(Vec::new).push(label);
    }
    Ok(map)
}

/// `offset`は結果行のうちラベルの`id`列が何番目から始まるか
/// （呼び出し元のSELECT列がラベル以外の列を前に持つことがあるため）。
fn row_to_label(row: &Row, offset: usize) -> rusqlite::Result<Label> {
    Ok(Label {
        id: row.get(offset)?,
        name: row.get(offset + 1)?,
        color: row.get(offset + 2)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::{self as db_messages, NewMessage};
    use crate::db::open_in_memory;

    fn make_account(conn: &Connection) -> i64 {
        accounts::insert(
            conn,
            &NewAccount {
                name: "Test",
                host: "pop.example.com",
                port: 995,
                username: "user",
                use_tls: true,
            },
        )
        .unwrap()
    }

    fn make_message(conn: &Connection, account_id: i64, uidl: &str) -> i64 {
        db_messages::insert_new(
            conn,
            &NewMessage {
                account_id,
                uidl,
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
        .unwrap()
        .unwrap()
    }

    #[test]
    fn insert_list_delete_roundtrip() {
        let conn = open_in_memory().unwrap();
        let id = insert(&conn, "仕事", "#C08A2E").unwrap();

        let all = list_with_counts(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].label.id, id);
        assert_eq!(all[0].count, 0);

        delete(&conn, id).unwrap();
        assert!(list_with_counts(&conn).unwrap().is_empty());
    }

    #[test]
    fn set_on_message_and_counts_and_for_messages() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        let label_id = insert(&conn, "旅行", "#8D9FC4").unwrap();
        let m1 = make_message(&conn, account_id, "u1");
        let m2 = make_message(&conn, account_id, "u2");

        set_on_message(&conn, m1, label_id, true).unwrap();
        set_on_message(&conn, m2, label_id, true).unwrap();

        let counts = list_with_counts(&conn).unwrap();
        assert_eq!(counts[0].count, 2);

        assert_eq!(for_message(&conn, m1).unwrap()[0].id, label_id);

        let map = for_messages(&conn, &[m1, m2]).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map[&m1][0].name, "旅行");

        set_on_message(&conn, m1, label_id, false).unwrap();
        assert!(for_message(&conn, m1).unwrap().is_empty());
        assert_eq!(list_with_counts(&conn).unwrap()[0].count, 1);
    }
}
