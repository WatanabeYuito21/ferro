//! 新着メールの通知（GUIのデスクトップ通知）に出す内容を集める。
//!
//! 同期の前に`messages`の最大idを覚えておき、同期の後にそれより大きいidの
//! メッセージを新着とみなす（`SyncSummary`は件数しか持たないため）。`labels`を
//! 指定すると、そのどれかのラベルが付いた新着だけを数える（受信時の自動ラベル付け
//! ルールと組み合わせて「重要なアラートだけ通知する」ために使う）。既読のもの（受信ルールの
//! 「既読にする」が当たった等）と、受信ルールの「通知しない」が当たったものは数えない。

use rusqlite::ToSql;

use crate::db::Connection;
use crate::db::messages::{self, Message};

/// いまの最大のメッセージid（メッセージが無ければ0）。同期の前に呼ぶ。
pub fn latest_id(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM messages", [], |row| row.get(0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMail {
    /// 通知の対象になる新着の件数。
    pub count: usize,
    /// そのうち新しい方から最大`sample_limit`件（通知の本文に件名を並べる用）。
    pub samples: Vec<Message>,
}

/// `after_id`より後に受信したメッセージのうち、通知の対象になるもの。
/// `labels`が空なら全部、そうでなければそのどれかのラベル（名前、大文字小文字を
/// 区別しない）が付いたものだけ。削除済み・既読・通知しない印付き（`notify_muted`）は除く。
pub fn since(
    conn: &Connection,
    after_id: i64,
    labels: &[String],
    sample_limit: usize,
) -> rusqlite::Result<NewMail> {
    let mut filter = String::from("id > ? AND is_deleted = 0 AND is_read = 0 AND notify_muted = 0");
    let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(after_id)];
    if !labels.is_empty() {
        let placeholders = vec!["?"; labels.len()].join(", ");
        filter.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM message_labels JOIN labels ON labels.id = message_labels.label_id \
             WHERE message_labels.message_id = messages.id AND labels.name COLLATE NOCASE IN ({placeholders}))"
        ));
        params.extend(labels.iter().map(|name| Box::new(name.clone()) as Box<dyn ToSql>));
    }
    let param_refs: Vec<&dyn ToSql> = params.iter().map(|p| p.as_ref()).collect();

    let count: i64 =
        conn.query_row(&format!("SELECT COUNT(*) FROM messages WHERE {filter}"), param_refs.as_slice(), |row| {
            row.get(0)
        })?;
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(&format!("SELECT id FROM messages WHERE {filter} ORDER BY id DESC LIMIT ?"))?;
        let mut with_limit = param_refs.clone();
        let limit = sample_limit as i64;
        with_limit.push(&limit);
        stmt.query_map(with_limit.as_slice(), |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    let mut samples = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(message) = messages::get(conn, id)? {
            samples.push(message);
        }
    }
    Ok(NewMail { count: count as usize, samples })
}

/// 通知のタイトルと本文。1件なら差出人と件名、複数なら件数と件名を最大3行。
pub fn notification_text(new_mail: &NewMail) -> (String, String) {
    let subject = |m: &Message| m.subject.clone().unwrap_or_else(|| "(件名なし)".to_string());
    if new_mail.count == 1
        && let Some(message) = new_mail.samples.first()
    {
        let from = message
            .from_name
            .clone()
            .or_else(|| message.from_addr.clone())
            .unwrap_or_else(|| "新着メール".to_string());
        return (from, subject(message));
    }
    let mut body: Vec<String> = new_mail.samples.iter().take(3).map(subject).collect();
    if new_mail.count > body.len() {
        body.push(format!("ほか{}件", new_mail.count - body.len()));
    }
    (format!("新着メール {}件", new_mail.count), body.join("\n"))
}

/// テスト用: `message_id`に`name`のラベルを付ける（無ければ作る）。
#[cfg(test)]
fn label(conn: &Connection, message_id: i64, name: &str) {
    let label_id = match crate::db::labels::find_by_name(conn, name).unwrap() {
        Some(label) => label.id,
        None => crate::db::labels::insert(conn, name, "#000000").unwrap(),
    };
    crate::db::labels::set_on_message(conn, message_id, label_id, true).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::NewMessage;
    use crate::db::open_in_memory;

    fn insert(conn: &Connection, uidl: &str, subject: &str) -> i64 {
        let account_id = match accounts::list(conn).unwrap().first() {
            Some(account) => account.id,
            None => accounts::insert(
                conn,
                &NewAccount { name: "a", host: "pop.example.com", port: 995, username: "u", use_tls: true },
            )
            .unwrap(),
        };
        messages::insert_new(
            conn,
            &NewMessage {
                account_id,
                uidl,
                message_id_header: None,
                subject: Some(subject),
                from_name: Some("Alice"),
                from_addr: Some("alice@example.com"),
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
    fn counts_only_messages_after_the_given_id() {
        let conn = open_in_memory().unwrap();
        assert_eq!(latest_id(&conn).unwrap(), 0);
        insert(&conn, "1", "old");
        let before = latest_id(&conn).unwrap();
        let a = insert(&conn, "2", "new a");
        let b = insert(&conn, "3", "new b");

        let new_mail = since(&conn, before, &[], 1).unwrap();
        assert_eq!(new_mail.count, 2);
        assert_eq!(new_mail.samples.iter().map(|m| m.id).collect::<Vec<_>>(), vec![b]);

        messages::set_deleted(&conn, a, true).unwrap();
        assert_eq!(since(&conn, before, &[], 5).unwrap().count, 1);
    }

    #[test]
    fn read_and_muted_messages_are_not_notified() {
        let conn = open_in_memory().unwrap();
        let read = insert(&conn, "1", "read by a rule");
        let muted = insert(&conn, "2", "muted by a rule");
        let normal = insert(&conn, "3", "normal");
        messages::set_read(&conn, read, true).unwrap();
        messages::mute_notification(&conn, muted).unwrap();

        let new_mail = since(&conn, 0, &[], 5).unwrap();
        assert_eq!(new_mail.count, 1);
        assert_eq!(new_mail.samples[0].id, normal);
    }

    #[test]
    fn label_filter_keeps_only_messages_with_one_of_the_labels() {
        let conn = open_in_memory().unwrap();
        let alert = insert(&conn, "1", "alert");
        let other = insert(&conn, "2", "other");
        label(&conn, alert, "重要");
        label(&conn, other, "その他");

        let new_mail = since(&conn, 0, &["重要".to_string(), "missing".to_string()], 5).unwrap();
        assert_eq!(new_mail.count, 1);
        assert_eq!(new_mail.samples[0].id, alert);
        assert_eq!(since(&conn, 0, &["missing".to_string()], 5).unwrap().count, 0);
    }

    #[test]
    fn notification_text_for_one_and_many() {
        let conn = open_in_memory().unwrap();
        insert(&conn, "1", "first");
        assert_eq!(
            notification_text(&since(&conn, 0, &[], 3).unwrap()),
            ("Alice".to_string(), "first".to_string())
        );
        for i in 2..=5 {
            insert(&conn, &i.to_string(), &format!("subject {i}"));
        }
        let (title, body) = notification_text(&since(&conn, 0, &[], 3).unwrap());
        assert_eq!(title, "新着メール 5件");
        assert_eq!(body, "subject 5\nsubject 4\nsubject 3\nほか2件");
    }
}
