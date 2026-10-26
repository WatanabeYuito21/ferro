//! スレッド（同じやり取りに属するメッセージのまとまり）の判定。
//!
//! 受信したメールの`References`/`In-Reply-To`に並ぶMessage-IDのどれかが、同じ
//! アカウントの受信済みメッセージにあれば、そのメッセージと同じスレッドにする。
//! 無ければ新しいスレッドの始まり（`thread_id`は自分自身のid）。件名が同じだけの
//! メールはまとめない（「障害通知」のような同じ件名の別件が大量にあるアラートメールを
//! 1つのスレッドにしてしまうため）。
//!
//! 返信が元のメールより先に届いた場合（POP3では受信順が送信順と限らない）、後から
//! 届いた元のメールは返信を参照していないので、別のスレッドになる（返信側の
//! `References`を保存していないため、後から繋ぎ直すことはしていない）。

use std::path::Path;

use rusqlite::{OptionalExtension, params};

use crate::db::Connection;
use crate::mail::parse::parse_headers;
use crate::maildir;

/// `References`が異常に長いメールで問い合わせが膨らまないよう、見るのは根に近い方から
/// この件数まで（スレッドの根と、その直後の数件が分かれば十分）。
const MAX_REFERENCES: usize = 20;

/// `references`（`ParsedHeaders::references`）から、`message_id`のメッセージが属する
/// スレッドを決めて`thread_id`に保存し、その値を返す。
pub fn assign(
    conn: &Connection,
    account_id: i64,
    message_id: i64,
    references: &[String],
) -> rusqlite::Result<i64> {
    let thread_id = find_thread(conn, account_id, references)?.unwrap_or(message_id);
    conn.execute(
        "UPDATE messages SET thread_id = ?1 WHERE id = ?2",
        params![thread_id, message_id],
    )?;
    Ok(thread_id)
}

/// 根に近い参照から順に探し、最初に見つかった受信済みメッセージのスレッドを返す。
/// そのメッセージ自体がまだスレッド未判定（`thread_id`がNULL）ならそのidを使う。
fn find_thread(conn: &Connection, account_id: i64, references: &[String]) -> rusqlite::Result<Option<i64>> {
    let mut stmt = conn.prepare_cached(
        "SELECT COALESCE(thread_id, id) FROM messages \
         WHERE account_id = ?1 AND message_id_header = ?2 ORDER BY id LIMIT 1",
    )?;
    for reference in references.iter().take(MAX_REFERENCES) {
        if let Some(thread_id) = stmt.query_row(params![account_id, reference], |row| row.get(0)).optional()? {
            return Ok(Some(thread_id));
        }
    }
    Ok(None)
}

/// このマイグレーション（0010）以前に受信していて、まだスレッドを判定していない
/// メッセージを`after_id`より後ろから最大`limit`件、Maildirのヘッダーを読み直して
/// 埋める。idの昇順（ほぼ受信順）に進めるので、元のメールは返信より先に判定される。
/// Maildirに実体が無いメッセージは単独のスレッドにする（同じ行を何度も拾わないように）。
///
/// 戻り値は(処理した件数, 次回渡す`after_id`)。`None`ならもう残っていない。
pub fn backfill_batch(
    conn: &Connection,
    maildir_base: &Path,
    after_id: i64,
    limit: u32,
) -> rusqlite::Result<(usize, Option<i64>)> {
    let pending: Vec<(i64, i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, account_id, uidl FROM messages WHERE thread_id IS NULL AND id > ?1 ORDER BY id LIMIT ?2",
        )?;
        stmt.query_map(params![after_id, limit], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let Some(&(last_id, _, _)) = pending.last() else {
        return Ok((0, None));
    };
    let tx = conn.unchecked_transaction()?;
    for (id, account_id, uidl) in &pending {
        let references = maildir::load(maildir_base, *account_id, uidl)
            .map(|raw| parse_headers(&raw).references)
            .unwrap_or_default();
        assign(&tx, *account_id, *id, &references)?;
    }
    tx.commit()?;
    Ok((pending.len(), Some(last_id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::{self, NewMessage};
    use crate::db::open_in_memory;

    fn account(conn: &Connection, name: &str) -> i64 {
        accounts::insert(
            conn,
            &NewAccount { name, host: "pop.example.com", port: 995, username: "user", use_tls: true },
        )
        .unwrap()
    }

    fn insert(conn: &Connection, account_id: i64, uidl: &str, message_id: Option<&str>) -> i64 {
        messages::insert_new(
            conn,
            &NewMessage {
                account_id,
                uidl,
                message_id_header: message_id,
                subject: Some("s"),
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

    fn thread_of(conn: &Connection, id: i64) -> Option<i64> {
        conn.query_row("SELECT thread_id FROM messages WHERE id = ?1", [id], |row| row.get(0)).unwrap()
    }

    #[test]
    fn replies_join_the_thread_of_the_message_they_reference() {
        let conn = open_in_memory().unwrap();
        let acc = account(&conn, "a");
        let root = insert(&conn, acc, "1", Some("root@x"));
        assert_eq!(assign(&conn, acc, root, &[]).unwrap(), root);

        let reply = insert(&conn, acc, "2", Some("reply@x"));
        assert_eq!(assign(&conn, acc, reply, &["root@x".into()]).unwrap(), root);

        // 返信への返信（Referencesに根と親、どちらかが分かればよい）。
        let reply2 = insert(&conn, acc, "3", Some("reply2@x"));
        assert_eq!(assign(&conn, acc, reply2, &["unknown@x".into(), "reply@x".into()]).unwrap(), root);
    }

    #[test]
    fn unrelated_messages_and_other_accounts_start_their_own_threads() {
        let conn = open_in_memory().unwrap();
        let a = account(&conn, "a");
        let b = account(&conn, "b");
        let root = insert(&conn, a, "1", Some("root@x"));
        assign(&conn, a, root, &[]).unwrap();

        let other_account = insert(&conn, b, "1", Some("reply@x"));
        assert_eq!(assign(&conn, b, other_account, &["root@x".into()]).unwrap(), other_account);
        let unknown = insert(&conn, a, "2", None);
        assert_eq!(assign(&conn, a, unknown, &["missing@x".into()]).unwrap(), unknown);
    }

    #[test]
    fn backfill_reads_headers_from_maildir_in_id_order() {
        let conn = open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let acc = account(&conn, "a");
        crate::maildir::store(dir.path(), acc, "1", b"Message-ID: <root@x>\r\nSubject: s\r\n\r\nbody").unwrap();
        crate::maildir::store(
            dir.path(),
            acc,
            "2",
            b"Message-ID: <reply@x>\r\nIn-Reply-To: <root@x>\r\nSubject: Re: s\r\n\r\nbody",
        )
        .unwrap();
        let root = insert(&conn, acc, "1", Some("root@x"));
        let reply = insert(&conn, acc, "2", Some("reply@x"));
        // Maildirに実体が無いメッセージ。
        let missing = insert(&conn, acc, "3", Some("missing@x"));

        assert_eq!(backfill_batch(&conn, dir.path(), 0, 2).unwrap(), (2, Some(reply)));
        assert_eq!(backfill_batch(&conn, dir.path(), reply, 2).unwrap(), (1, Some(missing)));
        assert_eq!(backfill_batch(&conn, dir.path(), missing, 2).unwrap(), (0, None));

        assert_eq!(thread_of(&conn, root), Some(root));
        assert_eq!(thread_of(&conn, reply), Some(root));
        assert_eq!(thread_of(&conn, missing), Some(missing));
        // 全部埋まったので、先頭から探してももう何も無い。
        assert_eq!(backfill_batch(&conn, dir.path(), 0, 10).unwrap(), (0, None));

        let info = messages::thread_info(&conn, &[root, reply, missing]).unwrap();
        assert_eq!(info[&root], (root, 2));
        assert_eq!(info[&reply], (root, 2));
        assert_eq!(info[&missing], (missing, 1));
        let thread: Vec<i64> = messages::list_thread(&conn, root, 50).unwrap().iter().map(|m| m.id).collect();
        assert_eq!(thread, vec![root, reply]);

        // 削除したメッセージは件数にも一覧にも入らない。
        messages::set_deleted(&conn, reply, true).unwrap();
        assert_eq!(messages::thread_info(&conn, &[root]).unwrap()[&root], (root, 1));
        assert_eq!(messages::list_thread(&conn, root, 50).unwrap().len(), 1);
    }
}
