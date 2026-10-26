//! 詳細検索の実行。検索欄の文字列を`search::query::parse`で分解し、全文検索の語と
//! 受信日は検索インデックス（Tantivy）で、フラグ・フォルダ・ラベル・添付の有無は
//! SQLiteで絞り込む（検索インデックスにはそれらを持たせていない。持たせると
//! 既読にするたびに索引を書き換えることになるため）。
//!
//! 全文検索の語とDB側の条件が両方ある場合は、まず検索インデックスから受信日時の
//! 新しい順に最大`CANDIDATE_LIMIT`件を取り、その中からDB側の条件に合うものを返す。
//! そのため、語に一致するメールが非常に多く、かつDB側の条件に合うものが古い方にしか
//! 無い場合は取りこぼしうる（日付は検索インデックス側で絞れるので、`after:`/`before:`で
//! 期間を狭めれば避けられる）。全文検索の語が無い場合はSQLiteだけで探す。

use rusqlite::ToSql;

use crate::db::Connection;
use crate::db::messages::{self, Folder};
use crate::search::query::ParsedQuery;
use crate::search::{SearchError, SearchIndex};

/// 全文検索の語とDB側の条件が両方あるとき、検索インデックスから取る候補の上限。
pub const CANDIDATE_LIMIT: usize = 5000;

#[derive(Debug, thiserror::Error)]
pub enum MessageSearchError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Search(#[from] SearchError),
}

/// 検索欄の文字列`query`をそのまま渡す版（解釈は`search::query::parse`）。
/// `utc_offset_seconds`は`after:`/`before:`の日付をどのタイムゾーンの0時とみなすか。
pub fn search_query(
    conn: &Connection,
    index: &SearchIndex,
    query: &str,
    utc_offset_seconds: i64,
    limit: usize,
) -> Result<Vec<i64>, MessageSearchError> {
    let parsed = crate::search::query::parse(query, utc_offset_seconds);
    search(conn, index, &parsed, limit, crate::db::now_unix())
}

/// `parsed`に合うメッセージのidを受信日時の新しい順に最大`limit`件返す。
/// `now`はスヌーズ中かどうかの判定（`in:inbox`/`in:snoozed`）に使う現在時刻。
pub fn search(
    conn: &Connection,
    index: &SearchIndex,
    parsed: &ParsedQuery,
    limit: usize,
    now: i64,
) -> Result<Vec<i64>, MessageSearchError> {
    if parsed.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    if parsed.has_text() {
        let candidates = if parsed.has_db_filters() { CANDIDATE_LIMIT.max(limit) } else { limit };
        let ids = index.search_terms(&parsed.text, parsed.after, parsed.before, candidates)?;
        if !parsed.has_db_filters() || ids.is_empty() {
            return Ok(ids);
        }
        return Ok(filter_in_db(conn, parsed, Some(&ids), limit, now)?);
    }
    Ok(filter_in_db(conn, parsed, None, limit, now)?)
}

/// DB側の条件で絞り込む。`candidates`があればその中から（日付は検索インデックス側で
/// 絞り込み済み）、無ければ全メッセージから（日付もここで絞る）。
fn filter_in_db(
    conn: &Connection,
    parsed: &ParsedQuery,
    candidates: Option<&[i64]>,
    limit: usize,
    now: i64,
) -> rusqlite::Result<Vec<i64>> {
    let mut conditions: Vec<String> = Vec::new();
    let mut params: Vec<Box<dyn ToSql>> = Vec::new();

    if let Some(ids) = candidates {
        let placeholders = vec!["?"; ids.len()].join(", ");
        conditions.push(format!("id IN ({placeholders})"));
        params.extend(ids.iter().map(|&id| Box::new(id) as Box<dyn ToSql>));
    } else {
        if let Some(after) = parsed.after {
            conditions.push("date_header >= ?".into());
            params.push(Box::new(after));
        }
        if let Some(before) = parsed.before {
            conditions.push("date_header < ?".into());
            params.push(Box::new(before));
        }
    }
    // ゴミ箱を明示的に指定しない限り、削除済みは出さない（通常の一覧と同じ）。
    if !parsed.folders.iter().any(|&(folder, negated)| folder == Folder::Trash && !negated) {
        conditions.push("is_deleted = 0".into());
    }
    for &(folder, negated) in &parsed.folders {
        let mut condition = String::new();
        messages::append_folder_condition(&mut condition, &mut params, folder, now);
        conditions.push(if negated { format!("NOT ({condition})") } else { format!("({condition})") });
    }
    if let Some(has) = parsed.has_attachment {
        conditions.push(if has { "attachment_count > 0" } else { "attachment_count = 0" }.into());
    }
    // フラグはバインド変数ではなくリテラルで書く。部分インデックス
    // （`idx_messages_unread`/`idx_messages_starred`、migration 0003/0009）の条件を
    // SQLiteが`?`からは推論しないため。
    if let Some(is_read) = parsed.is_read {
        conditions.push(if is_read { "is_read = 1" } else { "is_read = 0" }.into());
    }
    if let Some(is_flagged) = parsed.is_flagged {
        conditions.push(if is_flagged { "is_flagged = 1" } else { "is_flagged = 0" }.into());
    }
    for (name, negated) in &parsed.labels {
        let exists = "EXISTS (SELECT 1 FROM message_labels JOIN labels ON labels.id = message_labels.label_id \
                      WHERE message_labels.message_id = messages.id AND labels.name = ? COLLATE NOCASE)";
        conditions.push(if *negated { format!("NOT {exists}") } else { exists.to_string() });
        params.push(Box::new(name.clone()));
    }

    let where_clause = if conditions.is_empty() { "1".to_string() } else { conditions.join(" AND ") };
    let sql = format!("SELECT id FROM messages WHERE {where_clause} ORDER BY date_header DESC LIMIT ?");
    params.push(Box::new(limit as i64));
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map(param_refs.as_slice(), |row| row.get(0))?.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::{self as db_messages, NewMessage};
    use crate::db::{labels, open_in_memory};
    use crate::search::IndexableMessage;
    use crate::search::query::parse;

    struct Fixture {
        conn: Connection,
        index: SearchIndex,
        account_id: i64,
    }

    impl Fixture {
        fn new() -> Self {
            let conn = open_in_memory().unwrap();
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
            Fixture { conn, index: SearchIndex::create_in_ram().unwrap(), account_id }
        }

        fn add(&self, uidl: &str, subject: &str, from: &str, body: &str, date: i64, attachments: i64) -> i64 {
            let id = db_messages::insert_new(
                &self.conn,
                &NewMessage {
                    account_id: self.account_id,
                    uidl,
                    message_id_header: None,
                    subject: Some(subject),
                    from_name: None,
                    from_addr: Some(from),
                    to_addr: None,
                    date_header: date,
                    size_bytes: 0,
                    attachment_count: attachments,
                    preview: None,
                },
            )
            .unwrap()
            .unwrap();
            self.index
                .index_message(&IndexableMessage { id, subject, from, body, date_header: date })
                .unwrap();
            self.index.commit().unwrap();
            id
        }

        fn find(&self, query: &str) -> Vec<i64> {
            search(&self.conn, &self.index, &parse(query, 0), 50, 10_000).unwrap()
        }
    }

    #[test]
    fn field_operators_search_only_that_field() {
        let f = Fixture::new();
        let in_subject = f.add("a", "invoice", "alice@example.com", "hello", 1000, 0);
        let in_body = f.add("b", "hello", "bob@example.com", "invoice", 2000, 0);

        assert_eq!(f.find("invoice"), vec![in_body, in_subject]);
        assert_eq!(f.find("subject:invoice"), vec![in_subject]);
        assert_eq!(f.find("body:invoice"), vec![in_body]);
        assert_eq!(f.find("from:alice"), vec![in_subject]);
        assert_eq!(f.find("invoice -from:alice"), vec![in_body]);
        assert_eq!(f.find("-from:alice"), vec![in_body]);
    }

    #[test]
    fn dates_narrow_both_text_and_filter_only_searches() {
        let f = Fixture::new();
        let old = f.add("a", "report", "x@example.com", "", 1000, 1);
        let new = f.add("b", "report", "x@example.com", "", 5000, 1);

        assert_eq!(f.find("report before:1970-01-01"), Vec::<i64>::new());
        assert_eq!(f.find("report after:1970-01-01"), vec![new, old]);
        // date_header 1000..5000 の間で区切る（1970-01-01 01:00 = 3600）。
        let parsed = ParsedQuery { before: Some(3600), ..parse("report", 0) };
        assert_eq!(search(&f.conn, &f.index, &parsed, 50, 0).unwrap(), vec![old]);
        let parsed = ParsedQuery { after: Some(3600), ..parse("has:attachment", 0) };
        assert_eq!(search(&f.conn, &f.index, &parsed, 50, 0).unwrap(), vec![new]);
    }

    #[test]
    fn db_filters_combine_with_text_and_work_alone() {
        let f = Fixture::new();
        let with_attachment = f.add("a", "alert", "x@example.com", "", 1000, 2);
        let read_starred = f.add("b", "alert", "x@example.com", "", 2000, 0);
        let other = f.add("c", "news", "x@example.com", "", 3000, 0);
        db_messages::set_read(&f.conn, read_starred, true).unwrap();
        db_messages::set_flagged(&f.conn, read_starred, true).unwrap();

        assert_eq!(f.find("alert has:attachment"), vec![with_attachment]);
        assert_eq!(f.find("alert -has:attachment"), vec![read_starred]);
        assert_eq!(f.find("is:unread"), vec![other, with_attachment]);
        assert_eq!(f.find("is:starred"), vec![read_starred]);
        assert_eq!(f.find("alert is:read"), vec![read_starred]);
    }

    #[test]
    fn folder_and_label_filters() {
        let f = Fixture::new();
        let archived = f.add("a", "alert", "x@example.com", "", 1000, 0);
        let inbox = f.add("b", "alert", "x@example.com", "", 2000, 0);
        let deleted = f.add("c", "alert", "x@example.com", "", 3000, 0);
        db_messages::set_archived(&f.conn, archived, true).unwrap();
        db_messages::set_deleted(&f.conn, deleted, true).unwrap();
        let label = labels::insert(&f.conn, "Work", "#000000").unwrap();
        labels::set_on_message(&f.conn, archived, label, true).unwrap();

        assert_eq!(f.find("alert in:archive"), vec![archived]);
        assert_eq!(f.find("alert in:inbox"), vec![inbox]);
        assert_eq!(f.find("in:trash"), vec![deleted]);
        assert_eq!(f.find("alert -in:archive"), vec![inbox]);
        assert_eq!(f.find("label:work"), vec![archived]);
        assert_eq!(f.find("alert -label:work"), vec![inbox]);
        assert_eq!(f.find("label:nothing"), Vec::<i64>::new());
    }

    #[test]
    fn empty_queries_find_nothing() {
        let f = Fixture::new();
        f.add("a", "alert", "x@example.com", "", 1000, 0);
        assert_eq!(f.find(""), Vec::<i64>::new());
        assert_eq!(f.find("   "), Vec::<i64>::new());
    }
}
