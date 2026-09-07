use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};

use super::now_unix;

/// `row_to_message`と各SELECT文の両方で使う共通の列挙。新しい列を足すときは
/// ここと`row_to_message`の両方を同時に更新すること（列の並び順が一致している必要がある）。
const MESSAGE_COLUMNS: &str = "id, account_id, uidl, subject, from_name, from_addr, to_addr, \
    date_header, size_bytes, is_read, is_flagged, is_deleted, is_archived, snoozed_until, \
    attachment_count, preview";

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
    pub is_archived: bool,
    pub snoozed_until: Option<i64>,
    pub attachment_count: i64,
    pub preview: Option<String>,
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
    pub attachment_count: i64,
    pub preview: Option<&'a str>,
}

/// 新着メッセージを挿入する。同じ(account_id, uidl)が既にあれば何もせずNoneを返す。
/// sync再開時にUIDL差分方式で安全に重複を無視できるようにするための挙動。
pub fn insert_new(conn: &Connection, msg: &NewMessage) -> rusqlite::Result<Option<i64>> {
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO messages
            (account_id, uidl, message_id_header, subject, from_name, from_addr, to_addr,
             date_header, size_bytes, attachment_count, preview, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            msg.attachment_count,
            msg.preview,
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

pub fn set_read(conn: &Connection, id: i64, is_read: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE messages SET is_read = ?1 WHERE id = ?2",
        params![is_read, id],
    )?;
    Ok(())
}

pub fn set_flagged(conn: &Connection, id: i64, is_flagged: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE messages SET is_flagged = ?1 WHERE id = ?2",
        params![is_flagged, id],
    )?;
    Ok(())
}

pub fn set_archived(conn: &Connection, id: i64, is_archived: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE messages SET is_archived = ?1 WHERE id = ?2",
        params![is_archived, id],
    )?;
    Ok(())
}

/// `until`にNoneを渡すとスヌーズ解除（即座に受信箱へ戻す）。
pub fn set_snoozed(conn: &Connection, id: i64, until: Option<i64>) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE messages SET snoozed_until = ?1 WHERE id = ?2",
        params![until, id],
    )?;
    Ok(())
}

/// ソフトデリート。POP3サーバー側のDELEとは独立したローカルの削除フラグを立てる/戻す。
/// `is_deleted=1`にすると`list_recent`/`list_all_for_reindex`から即座に外れる。
/// 検索インデックス側からも取り除きたい場合は`message_actions::set_deleted`を使うこと
/// （このDB単体の関数は検索インデックスには一切触れない）。
pub fn set_deleted(conn: &Connection, id: i64, is_deleted: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE messages SET is_deleted = ?1 WHERE id = ?2",
        params![is_deleted, id],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, id: i64) -> rusqlite::Result<Option<Message>> {
    conn.query_row(
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1"),
        [id],
        row_to_message,
    )
    .optional()
}

/// アカウント削除時、Maildir/検索インデックスのクリーンアップに使うため
/// そのアカウントの全メッセージ（`is_deleted`に関わらず）を一覧する。
pub fn list_all_for_account(conn: &Connection, account_id: i64) -> rusqlite::Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages WHERE account_id = ?1"
    ))?;
    stmt.query_map([account_id], row_to_message)?.collect()
}

/// アカウントの全メッセージ行を削除する。`accounts`への外部キー参照が
/// (`ON DELETE CASCADE`を付けていないため)残っていると`db::accounts::delete`が
/// 失敗するので、アカウント削除の前に呼ぶ（`account_setup::remove`参照）。
pub fn delete_all_for_account(conn: &Connection, account_id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM messages WHERE account_id = ?1", [account_id])?;
    Ok(())
}

/// idの昇順で全件を漏れなく舐めるためのページネーション（全文検索インデックスの
/// 再構築専用）。`list_recent`のdate_headerカーソルは値が重複しうるため
/// 境界で取りこぼす可能性があるが、`id`は一意なのでこちらは完全に漏れなく辿れる。
pub fn list_all_for_reindex(
    conn: &Connection,
    after_id: i64,
    limit: u32,
) -> rusqlite::Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages
         WHERE is_deleted = 0 AND id > ?1
         ORDER BY id ASC
         LIMIT ?2"
    ))?;
    stmt.query_map(params![after_id, limit], row_to_message)?
        .collect()
}

/// 検索インデックス未投入(`fts_indexed_at IS NULL`)のメッセージを、`after_id`より
/// idが大きいものだけ古い順に一定件数返す。syncのバッチ単位commitがTantivyの
/// IndexWriterクラッシュでリトライを使い切って失敗した場合、そのバッチのメッセージは
/// DB/Maildirには保存済みだが検索インデックスには入らないまま残る。
/// `reindex::catch_up_unindexed`がこれを定期的に拾い直すために使う。
///
/// `after_id`があるのは、同じバッチが繰り返し失敗し続けるケース（実機で確認済み。
/// Tantivy IndexWriterクラッシュが頻発する状況では珍しくない）で、常に同じ
/// 「先頭からN件」を返し続けると呼び出し側が永久に同じ集合でブロックされ、
/// その後ろにある未投入メッセージに一生手が届かなくなるため。呼び出し側は
/// 前回処理した末尾のidを`after_id`として渡すことで、成功・失敗に関わらず
/// 前進できる（拾いきれなかった分は次の周回でafter_id=0からやり直せば再度チャンスがある）。
pub fn list_unindexed(conn: &Connection, after_id: i64, limit: u32) -> rusqlite::Result<Vec<Message>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages
         WHERE is_deleted = 0 AND fts_indexed_at IS NULL AND id > ?1
         ORDER BY id ASC
         LIMIT ?2"
    ))?;
    stmt.query_map(params![after_id, limit], row_to_message)?.collect()
}

/// 検索インデックスへの投入が成功したメッセージにマークを付ける
/// (`list_unindexed`が二度と拾わないようにするため。sync/reindex_all両方から呼ぶ)。
pub fn mark_indexed(conn: &Connection, ids: &[i64]) -> rusqlite::Result<()> {
    let indexed_at = now_unix();
    for id in ids {
        conn.execute(
            "UPDATE messages SET fts_indexed_at = ?1, fts_doc_version = fts_doc_version + 1 WHERE id = ?2",
            params![indexed_at, id],
        )?;
    }
    Ok(())
}

/// `attachment_count`/`preview`を再計算した値で上書きする。`reindex_all`が
/// Maildirから読み直したメッセージについて呼ぶ（プレビューの切り詰め長を伸ばした
/// ([`crate::mail::parse::make_preview`]) 際に、sync時点で既に古い(短い)previewが
/// 保存されている既存メッセージへ遡って反映するため。色分けルールがpreviewの
/// 途中までしかマッチ対象に見えていなかった、という形で実際に踏んだ）。
pub fn update_preview(
    conn: &Connection,
    id: i64,
    attachment_count: i64,
    preview: Option<&str>,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE messages SET attachment_count = ?1, preview = ?2 WHERE id = ?3",
        params![attachment_count, preview, id],
    )?;
    Ok(())
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
    let mut sql = format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE is_deleted = 0");
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

/// フォルダ別一覧の対象。「送信済み/下書き」に相当するものは無い
/// （Ferroは受信専用。CLAUDE.md参照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    Inbox,
    Starred,
    Snoozed,
    Archive,
    Trash,
}

/// フォルダ別のキーセットページネーション一覧。`list_recent`(受信箱専用、
/// CLI等の既存呼び出し元がある)とは別関数にして、既存の挙動・テストに触れない。
/// `now`はスヌーズ判定用の現在時刻(呼び出し側から渡す。テスト容易性のため)。
pub fn list_by_folder(
    conn: &Connection,
    folder: Folder,
    account_id: Option<i64>,
    before: Option<i64>,
    limit: u32,
    now: i64,
) -> rusqlite::Result<Vec<Message>> {
    let (sql, params) = build_folder_query(folder, account_id, before, limit, now);
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map(param_refs.as_slice(), row_to_message)?.collect()
}

/// `sql`は末尾が"WHERE "で終わっている前提で、フォルダ条件をそこに追記する
/// （`build_folder_query`/`build_folder_count_query`で共有する）。
fn append_folder_condition(
    sql: &mut String,
    params: &mut Vec<Box<dyn ToSql>>,
    folder: Folder,
    now: i64,
) {
    match folder {
        Folder::Inbox => {
            sql.push_str(
                "is_deleted = 0 AND is_archived = 0 AND (snoozed_until IS NULL OR snoozed_until <= ?)",
            );
            params.push(Box::new(now));
        }
        Folder::Starred => {
            sql.push_str("is_deleted = 0 AND is_flagged = 1");
        }
        Folder::Snoozed => {
            sql.push_str("is_deleted = 0 AND snoozed_until IS NOT NULL AND snoozed_until > ?");
            params.push(Box::new(now));
        }
        Folder::Archive => {
            sql.push_str("is_deleted = 0 AND is_archived = 1");
        }
        Folder::Trash => {
            sql.push_str("is_deleted = 1");
        }
    }
}

fn build_folder_query(
    folder: Folder,
    account_id: Option<i64>,
    before: Option<i64>,
    limit: u32,
    now: i64,
) -> (String, Vec<Box<dyn ToSql>>) {
    let mut sql = format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE ");
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();
    append_folder_condition(&mut sql, &mut params, folder, now);

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

/// サイドバーのフォルダ件数表示用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FolderCounts {
    pub inbox: i64,
    pub starred: i64,
    pub snoozed: i64,
    pub archive: i64,
    pub trash: i64,
}

pub fn folder_counts(
    conn: &Connection,
    account_id: Option<i64>,
    now: i64,
) -> rusqlite::Result<FolderCounts> {
    let mut counts = FolderCounts::default();
    for folder in [
        Folder::Inbox,
        Folder::Starred,
        Folder::Snoozed,
        Folder::Archive,
        Folder::Trash,
    ] {
        let (sql, params) = build_folder_count_query(folder, account_id, now);
        let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let count: i64 = conn.query_row(&sql, param_refs.as_slice(), |row| row.get(0))?;
        match folder {
            Folder::Inbox => counts.inbox = count,
            Folder::Starred => counts.starred = count,
            Folder::Snoozed => counts.snoozed = count,
            Folder::Archive => counts.archive = count,
            Folder::Trash => counts.trash = count,
        }
    }
    Ok(counts)
}

fn build_folder_count_query(
    folder: Folder,
    account_id: Option<i64>,
    now: i64,
) -> (String, Vec<Box<dyn ToSql>>) {
    let mut sql = String::from("SELECT COUNT(*) FROM messages WHERE ");
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();
    append_folder_condition(&mut sql, &mut params, folder, now);

    if let Some(id) = account_id {
        sql.push_str(" AND account_id = ?");
        params.push(Box::new(id));
    }

    (sql, params)
}

/// 指定ラベルが付いたメッセージのキーセットページネーション一覧。
/// ラベルは通常メッセージ全体からすれば少数のサブセットのはずなので、
/// `messages`側の(account_id, date_header)インデックスは使えず一時ソートに
/// なりうるが、対象件数が少ないため許容する。
pub fn list_by_label(
    conn: &Connection,
    label_id: i64,
    before: Option<i64>,
    limit: u32,
) -> rusqlite::Result<Vec<Message>> {
    let mut sql = format!(
        "SELECT {MESSAGE_COLUMNS} FROM messages
         JOIN message_labels ON message_labels.message_id = messages.id
         WHERE message_labels.label_id = ? AND messages.is_deleted = 0"
    );
    let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(label_id)];

    if let Some(cursor) = before {
        sql.push_str(" AND messages.date_header < ?");
        params.push(Box::new(cursor));
    }
    sql.push_str(" ORDER BY messages.date_header DESC LIMIT ?");
    params.push(Box::new(limit));

    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map(param_refs.as_slice(), row_to_message)?.collect()
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
        is_archived: row.get(12)?,
        snoozed_until: row.get(13)?,
        attachment_count: row.get(14)?,
        preview: row.get(15)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::open_in_memory;

    fn make_account(conn: &Connection) -> i64 {
        make_named_account(conn, "Test")
    }

    fn make_named_account(conn: &Connection, name: &str) -> i64 {
        accounts::insert(
            conn,
            &NewAccount {
                name,
                host: "pop.example.com",
                port: 110,
                username: "bob",
                use_tls: false,
            },
        )
        .unwrap()
    }

    fn insert_msg(conn: &Connection, account_id: i64, uidl: &str, date_header: i64) -> i64 {
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
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap()
        .unwrap()
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

    #[test]
    fn set_read_set_flagged_set_deleted_update_only_the_target_row() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        insert_msg(&conn, account_id, "u1", 100);
        insert_msg(&conn, account_id, "u2", 200);
        let ids: Vec<i64> = list_recent(&conn, Some(account_id), None, 10)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        let (id1, id2) = (ids[0], ids[1]);

        set_read(&conn, id1, true).unwrap();
        set_flagged(&conn, id1, true).unwrap();
        let m1 = get(&conn, id1).unwrap().unwrap();
        assert!(m1.is_read);
        assert!(m1.is_flagged);

        let m2 = get(&conn, id2).unwrap().unwrap();
        assert!(!m2.is_read);
        assert!(!m2.is_flagged);

        set_deleted(&conn, id1, true).unwrap();
        assert!(get(&conn, id1).unwrap().unwrap().is_deleted);
        assert_eq!(
            list_recent(&conn, Some(account_id), None, 10)
                .unwrap()
                .len(),
            1,
            "soft-deleted message should be excluded from list_recent"
        );

        set_deleted(&conn, id1, false).unwrap();
        assert!(!get(&conn, id1).unwrap().unwrap().is_deleted);
        assert_eq!(
            list_recent(&conn, Some(account_id), None, 10)
                .unwrap()
                .len(),
            2,
            "restoring should bring it back into list_recent"
        );
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
                attachment_count: 0,
                preview: None,
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
                attachment_count: 0,
                preview: None,
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
                attachment_count: 0,
                preview: None,
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
        let a1 = make_named_account(&conn, "Account1");
        let a2 = make_named_account(&conn, "Account2");
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

    #[test]
    fn list_unindexed_after_id_skips_earlier_messages() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        let first = insert_msg(&conn, account_id, "u0", 0);
        let second = insert_msg(&conn, account_id, "u1", 1);
        let third = insert_msg(&conn, account_id, "u2", 2);

        // after_id=0(先頭から)なら3件とも返る。
        let all = list_unindexed(&conn, 0, 10).unwrap();
        assert_eq!(all.iter().map(|m| m.id).collect::<Vec<_>>(), vec![first, second, third]);

        // after_idに最初のメッセージのidを渡すと、それより後ろだけが返る。
        // これにより、あるメッセージ群の投入が(Tantivy側の事情で)繰り返し
        // 失敗しても、呼び出し側はafter_idを進めて後続を拾いに行ける
        // （同じ先頭集合に永久にブロックされない）。
        let after_first = list_unindexed(&conn, first, 10).unwrap();
        assert_eq!(after_first.iter().map(|m| m.id).collect::<Vec<_>>(), vec![second, third]);

        let after_all = list_unindexed(&conn, third, 10).unwrap();
        assert!(after_all.is_empty());
    }

    #[test]
    fn list_unindexed_query_uses_index_and_never_sorts_with_temp_btree() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        for i in 0..20 {
            insert_msg(&conn, account_id, &format!("u{i}"), i);
        }

        let explain_sql = format!(
            "EXPLAIN QUERY PLAN SELECT {MESSAGE_COLUMNS} FROM messages
             WHERE is_deleted = 0 AND fts_indexed_at IS NULL AND id > ?1
             ORDER BY id ASC
             LIMIT ?2"
        );
        let mut stmt = conn.prepare(&explain_sql).unwrap();
        let plan: Vec<String> = stmt
            .query_map(params![0i64, 10u32], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert!(
            plan.iter().any(|line| line.contains("USING INDEX idx_messages_unindexed")),
            "expected the query to use idx_messages_unindexed, got plan: {plan:?}"
        );
        assert!(
            !plan.iter().any(|line| line.contains("TEMP B-TREE")),
            "expected no temp b-tree sort, got plan: {plan:?}"
        );
    }

    #[test]
    fn list_by_folder_separates_starred_archived_snoozed_trash_and_inbox() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        let inbox = insert_msg(&conn, account_id, "inbox", 100);
        let starred = insert_msg(&conn, account_id, "starred", 200);
        let archived = insert_msg(&conn, account_id, "archived", 300);
        let snoozed_future = insert_msg(&conn, account_id, "snoozed-future", 400);
        let snoozed_past = insert_msg(&conn, account_id, "snoozed-past", 500);
        let trashed = insert_msg(&conn, account_id, "trashed", 600);

        set_flagged(&conn, starred, true).unwrap();
        set_archived(&conn, archived, true).unwrap();
        set_snoozed(&conn, snoozed_future, Some(10_000)).unwrap();
        set_snoozed(&conn, snoozed_past, Some(1)).unwrap();
        set_deleted(&conn, trashed, true).unwrap();

        let now = 5_000;
        let ids = |msgs: Vec<Message>| msgs.into_iter().map(|m| m.id).collect::<Vec<_>>();

        // 受信箱: アーカイブ/ゴミ箱/スヌーズ中を除く（スター付きは他の一覧同様、
        // 単なるフラグなので受信箱からは除外されない。スヌーズ期限が過ぎたものは戻る）。
        let inbox_ids = ids(list_by_folder(&conn, Folder::Inbox, None, None, 50, now).unwrap());
        assert_eq!(inbox_ids, vec![snoozed_past, starred, inbox]);

        assert_eq!(
            ids(list_by_folder(&conn, Folder::Starred, None, None, 50, now).unwrap()),
            vec![starred]
        );
        assert_eq!(
            ids(list_by_folder(&conn, Folder::Archive, None, None, 50, now).unwrap()),
            vec![archived]
        );
        assert_eq!(
            ids(list_by_folder(&conn, Folder::Snoozed, None, None, 50, now).unwrap()),
            vec![snoozed_future]
        );
        assert_eq!(
            ids(list_by_folder(&conn, Folder::Trash, None, None, 50, now).unwrap()),
            vec![trashed]
        );
    }

    #[test]
    fn list_by_folder_query_plans_use_an_index() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        for i in 0..20 {
            insert_msg(&conn, account_id, &format!("f{i}"), i);
        }

        for folder in [
            Folder::Inbox,
            Folder::Starred,
            Folder::Archive,
            Folder::Snoozed,
            Folder::Trash,
        ] {
            let (sql, params) = build_folder_query(folder, Some(account_id), None, 50, 1_000);
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
                "folder {folder:?}: expected the query to use an index, got plan: {plan:?}"
            );
        }
    }

    #[test]
    fn list_by_label_returns_only_tagged_non_deleted_messages_with_cursor() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        let m1 = insert_msg(&conn, account_id, "l1", 100);
        let m2 = insert_msg(&conn, account_id, "l2", 200);
        let m3 = insert_msg(&conn, account_id, "l3", 300);
        insert_msg(&conn, account_id, "untagged", 400);

        let label_id = crate::db::labels::insert(&conn, "旅行", "#8D9FC4").unwrap();
        crate::db::labels::set_on_message(&conn, m1, label_id, true).unwrap();
        crate::db::labels::set_on_message(&conn, m2, label_id, true).unwrap();
        crate::db::labels::set_on_message(&conn, m3, label_id, true).unwrap();
        set_deleted(&conn, m1, true).unwrap();

        let first_page = list_by_label(&conn, label_id, None, 1).unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].id, m3);

        let second_page = list_by_label(&conn, label_id, Some(first_page[0].date_header), 10).unwrap();
        assert_eq!(
            second_page.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![m2]
        );
    }

    #[test]
    fn folder_counts_matches_list_by_folder_lengths() {
        let conn = open_in_memory().unwrap();
        let account_id = make_account(&conn);
        let inbox = insert_msg(&conn, account_id, "inbox", 100);
        let starred = insert_msg(&conn, account_id, "starred", 200);
        let archived = insert_msg(&conn, account_id, "archived", 300);
        let snoozed = insert_msg(&conn, account_id, "snoozed", 400);
        let trashed = insert_msg(&conn, account_id, "trashed", 500);

        set_flagged(&conn, starred, true).unwrap();
        set_archived(&conn, archived, true).unwrap();
        set_snoozed(&conn, snoozed, Some(10_000)).unwrap();
        set_deleted(&conn, trashed, true).unwrap();

        let now = 1_000;
        let counts = folder_counts(&conn, None, now).unwrap();
        assert_eq!(counts.inbox, 2); // inbox本体 + starred(受信箱にも残る)
        assert_eq!(counts.starred, 1);
        assert_eq!(counts.archive, 1);
        assert_eq!(counts.snoozed, 1);
        assert_eq!(counts.trash, 1);
        let _ = inbox;
    }
}
