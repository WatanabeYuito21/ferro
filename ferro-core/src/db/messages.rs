use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};

use super::now_unix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: i64,
    pub account_id: i64,
    pub uidl: String,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
    pub to_addr: Option<String>,
    pub date_header: i64,
    pub size_bytes: i64,
    pub is_read: bool,
    pub is_flagged: bool,
    pub is_deleted: bool,
}

pub struct NewMessage<'a> {
    pub account_id: i64,
    pub uidl: &'a str,
    pub message_id_header: Option<&'a str>,
    pub subject: Option<&'a str>,
    pub from_name: Option<&'a str>,
    pub from_addr: Option<&'a str>,
    pub to_addr: Option<&'a str>,
    pub date_header: i64,
    pub size_bytes: i64,
}

/// 新着メッセージを挿入する。同じ(account_id, uidl)が既にあれば何もせずNoneを返す。
/// sync再開時にUIDL差分方式で安全に重複を無視できるようにするための挙動。
pub fn insert_new(conn: &Connection, msg: &NewMessage) -> rusqlite::Result<Option<i64>> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO messages
            (account_id, uidl, message_id_header, subject, from_name, from_addr, to_addr,
             date_header, size_bytes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            msg.account_id,
            msg.uidl,
            msg.message_id_header,
            msg.subject,
            msg.from_name,
            msg.from_addr,
            msg.to_addr,
            msg.date_header,
            msg.size_bytes,
            now_unix(),
        ],
    )?;
    Ok((inserted > 0).then(|| conn.last_insert_rowid()))
}

/// syncの差分計算用: 指定UIDLが既に保存済みかどうか。
pub fn exists_by_uidl(conn: &Connection, account_id: i64, uidl: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT 1 FROM messages WHERE account_id = ?1 AND uidl = ?2",
        params![account_id, uidl],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Message>> {
    conn.query_row(
        "SELECT id, account_id, uidl, subject, from_name, from_addr, to_addr,
                date_header, size_bytes, is_read, is_flagged, is_deleted
         FROM messages WHERE id = ?1",
        [id],
        row_to_message,
    )
    .optional()
}

/// idの昇順で全件を漏れなく舐めるためのページネーション（全文検索インデックスの
/// 再構築専用）。`list_recent`のdate_headerカーソルは値が重複しうるため
/// 境界で取りこぼす可能性があるが、`id`は一意なのでこちらは完全に漏れなく辿れる。
pub fn list_all_for_reindex(
    conn: &Connection,
    after_id: i64,
    limit: u32,
) -> rusqlite::Result<Vec<Message>> {
    let mut stmt = conn.prepare(
        "SELECT id, account_id, uidl, subject, from_name, from_addr, to_addr,
                date_header, size_bytes, is_read, is_flagged, is_deleted
         FROM messages
         WHERE is_deleted = 0 AND id > ?1
         ORDER BY id ASC
         LIMIT ?2",
    )?;
    stmt.query_map(params![after_id, limit], row_to_message)?
        .collect()
}

/// キーセットページネーションで新着順（date_header降順）に一覧取得する。
///
/// `account_id`がNoneなら全アカウント横断。`before`を指定すると、そのdate_headerより
/// 古いメッセージから返す（無限スクロールの次ページカーソル）。
///
/// バインド変数を`(?1 IS NULL OR col = ?1)`のようにOR分岐させると、SQLiteが
/// インデックスを使った範囲検索(SEARCH)ではなく全件スキャン(SCAN)にフォールバック
/// することがあるため、フィルタが実際に指定された場合だけSQL文字列に条件を足す
/// （`build_list_recent_query`のテストで`EXPLAIN QUERY PLAN`により確認済み）。
pub fn list_recent(
    conn: &Connection,
    account_id: Option<i64>,
    before: Option<i64>,
    limit: u32,
) -> rusqlite::Result<Vec<Message>> {
    let (sql, params) = build_list_recent_query(account_id, before, limit);
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map(param_refs.as_slice(), row_to_message)?.collect()
}

fn build_list_recent_query(
    account_id: Option<i64>,
    before: Option<i64>,
    limit: u32,
) -> (String, Vec<Box<dyn ToSql>>) {
    let mut sql = String::from(
        "SELECT id, account_id, uidl, subject, from_name, from_addr, to_addr,
                date_header, size_bytes, is_read, is_flagged, is_deleted
         FROM messages
         WHERE is_deleted = 0",
    );
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(id) = account_id {
        sql.push_str(" AND account_id = ?");
        params.push(Box::new(id));
    }
    if let Some(cursor) = before {
        sql.push_str(" AND date_header < ?");
        params.push(Box::new(cursor));
    }
    sql.push_str(" ORDER BY date_header DESC LIMIT ?");
    params.push(Box::new(limit));

    (sql, params)
}

fn row_to_message(row: &Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        account_id: row.get(1)?,
        uidl: row.get(2)?,
        subject: row.get(3)?,
        from_name: row.get(4)?,
        from_addr: row.get(5)?,
        to_addr: row.get(6)?,
        date_header: row.get(7)?,
        size_bytes: row.get(8)?,
        is_read: row.get(9)?,
        is_flagged: row.get(10)?,
        is_deleted: row.get(11)?,
    })
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
                name: "Test",
                host: "pop.example.com",
                port: 110,
                username: "bob",
                use_tls: false,
            },
        )
        .unwrap()
    }

    fn insert_msg(conn: &Connection, account_id: i64, uidl: &str, date_header: i64) {
        insert_new(
            conn,
            &NewMessage {
                account_id,
                uidl,
                message_id_header: None,
                subject: Some("hi"),
                from_name: None,
                from_addr: Some("a@example.com"),
                to_addr: Some("b@example.com"),
                date_header,
                size_bytes: 100,
            },
        )
        .unwrap();
    }

    #[test]
    fn get_returns_message_or_none() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        insert_msg(&conn, account_id, "u1", 100);

        let all = list_recent(&conn, Some(account_id), None, 10).unwrap();
        let id = all[0].id;

        let found = get(&conn, id).unwrap().expect("message should exist");
        assert_eq!(found.uidl, "u1");
        assert!(get(&conn, id + 1000).unwrap().is_none());
    }

    /// `list_recent`のdate_headerカーソルは値が重複すると境界で取りこぼしうるが、
    /// idベースのこちらは重複date_headerがあっても全件を漏れなく辿れることを確認する。
    #[test]
    fn list_all_for_reindex_visits_every_row_exactly_once_even_with_duplicate_dates() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        for i in 0..25 {
            // 全メッセージが同じdate_headerを持つ、意図的な最悪ケース。
            insert_msg(&conn, account_id, &format!("u{i}"), 1000);
        }

        let mut seen = Vec::new();
        let mut after_id = 0i64;
        loop {
            let page = list_all_for_reindex(&conn, after_id, 10).unwrap();
            if page.is_empty() {
                break;
            }
            after_id = page.last().unwrap().id;
            seen.extend(page.into_iter().map(|m| m.uidl));
        }

        seen.sort();
        let mut expected: Vec<String> = (0..25).map(|i| format!("u{i}")).collect();
        expected.sort();
        assert_eq!(seen, expected);
    }

    #[test]
    fn insert_new_ignores_duplicate_uidl() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);

        let first = insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "uidl-1",
                message_id_header: Some("<1@example.com>"),
                subject: Some("hello"),
                from_name: Some("Alice"),
                from_addr: Some("alice@example.com"),
                to_addr: Some("bob@example.com"),
                date_header: 1000,
                size_bytes: 42,
            },
        )
        .unwrap();
        assert!(first.is_some());

        let duplicate = insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "uidl-1",
                message_id_header: None,
                subject: None,
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 2000,
                size_bytes: 1,
            },
        )
        .unwrap();
        assert!(duplicate.is_none());

        assert!(exists_by_uidl(&conn, account_id, "uidl-1").unwrap());
        assert!(!exists_by_uidl(&conn, account_id, "uidl-missing").unwrap());
    }

    #[test]
    fn insert_new_rejects_unknown_account() {
        let conn = open_in_memory().unwrap();
        let result = insert_new(
            &conn,
            &NewMessage {
                account_id: 999,
                uidl: "uidl-x",
                message_id_header: None,
                subject: None,
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 1,
                size_bytes: 1,
            },
        );
        assert!(result.is_err(), "foreign key violation should be rejected");
    }

    #[test]
    fn list_recent_orders_by_date_desc_and_paginates_with_cursor() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);

        for (uidl, date) in [("u1", 100), ("u2", 200), ("u3", 300), ("u4", 400)] {
            insert_msg(&conn, account_id, uidl, date);
        }

        let first_page = list_recent(&conn, Some(account_id), None, 2).unwrap();
        assert_eq!(
            first_page.iter().map(|m| m.uidl.as_str()).collect::<Vec<_>>(),
            vec!["u4", "u3"]
        );

        let cursor = first_page.last().unwrap().date_header;
        let second_page = list_recent(&conn, Some(account_id), Some(cursor), 2).unwrap();
        assert_eq!(
            second_page
                .iter()
                .map(|m| m.uidl.as_str())
                .collect::<Vec<_>>(),
            vec!["u2", "u1"]
        );
    }

    #[test]
    fn list_recent_excludes_soft_deleted_and_supports_cross_account() {
        let conn = open_in_memory().unwrap();
        let a1 = make_account(&conn);
        let a2 = make_account(&conn);
        insert_msg(&conn, a1, "u1", 100);
        insert_msg(&conn, a2, "u2", 200);
        conn.execute("UPDATE messages SET is_deleted = 1 WHERE uidl = 'u2'", [])
            .unwrap();

        // account_idを指定しない全アカウント横断一覧。
        let all = list_recent(&conn, None, None, 10).unwrap();
        assert_eq!(
            all.iter().map(|m| m.uidl.as_str()).collect::<Vec<_>>(),
            vec!["u1"]
        );
    }

    /// CLAUDE.mdの教訓: バインド変数の`IS NULL OR`分岐はSQLiteをCOVERING SCAN +
    /// 一時B-treeでのソート(`USE TEMP B-TREE FOR ORDER BY`)にフォールバックさせやすい。
    /// フィルタなし（cursorなしの先頭ページ、全アカウント横断など）の場合はインデックスを
    /// 順序どおりに舐める`SCAN ... USING INDEX`になるのが最適解であり、これは問題ない
    /// （SEARCHである必要はない）。悪いのは一時B-treeソートが発生すること。
    #[test]
    fn list_recent_query_uses_index_and_never_sorts_with_temp_btree() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        for i in 0..20 {
            insert_msg(&conn, account_id, &format!("u{i}"), i);
        }

        for (account_id, before) in [
            (Some(account_id), None),
            (Some(account_id), Some(10i64)),
            (None, Some(10i64)),
            (None, None),
        ] {
            let (sql, params) = build_list_recent_query(account_id, before, 50);
            let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
            let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();

            let mut stmt = conn.prepare(&explain_sql).unwrap();
            let plan: Vec<String> = stmt
                .query_map(param_refs.as_slice(), |row| row.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();

            assert!(
                plan.iter().any(|line| line.contains("USING INDEX")),
                "expected the query to use an index, got plan: {plan:?}"
            );
            assert!(
                !plan.iter().any(|line| line.contains("TEMP B-TREE")),
                "expected no temp b-tree sort, got plan: {plan:?}"
            );
        }
    }
}
