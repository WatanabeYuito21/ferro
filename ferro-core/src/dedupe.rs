//! UIDL非対応サーバー向けフォールバック（`sync::fetch_and_store_by_hash`）が作った
//! 重複メッセージの検出と整理。
//!
//! 旧フォールバック(v1)はメール全体のハッシュを重複判定に使っていたため、RETRの
//! たびに経路上のゲートウェイ（WatchGuard等の`X-WatchGuard-Spam-ID`ヘッダー）が
//! 内容を付け替えるサーバーでは、同じメールが毎回別物として保存され続けた
//! （実データで約2千通のMessage-IDが5万行に膨れていた）。
//!
//! 対象は`accounts.uidl_supported = 0`（フォールバック経路を使うアカウント）の
//! 行だけに限る。UIDLを持つアカウントでは同じMessage-IDの別メッセージが
//! 正当に存在しうるうえ、DB行を消すと次回syncが「未取得の新着」として
//! 再取得してしまうため。
//!
//! 重複の判定: `message_id_header`がある行は(account_id, Message-ID)が同じもの、
//! 無い行は(account_id, 件名, 差出人アドレス, 宛先, 日時)が全て同じもの。
//! 各グループで最小idの行を残す。残す行には削除する行の既読/スター/ラベルを
//! 引き継ぐ（どれかが既読/スター付き/ラベル付きなら残す行もそうする）。

use std::collections::HashMap;
use std::path::Path;

use crate::db::Connection;
use crate::maildir;
use crate::search::{SearchIndex, with_commit_retry};

#[derive(Debug, thiserror::Error)]
pub enum DedupeError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
}

/// 重複グループ1つ。`keep_id`を残し、`remove_ids`を消す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub account_id: i64,
    pub keep_id: i64,
    pub remove_ids: Vec<i64>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DedupeSummary {
    pub groups: usize,
    pub removed: usize,
    /// 検索インデックスからの削除に失敗したため、今回は消さずに残した件数
    /// （もう一度実行すれば再挑戦される）。
    pub skipped: usize,
}

/// 重複グループを列挙する（読み取りのみ）。
pub fn find_duplicates(conn: &Connection) -> Result<Vec<DuplicateGroup>, DedupeError> {
    let mut groups = Vec::new();

    // Message-IDで判定できる行。
    let mut stmt = conn.prepare(
        "SELECT m.account_id, m.message_id_header, m.id
         FROM messages m JOIN accounts a ON a.id = m.account_id
         WHERE a.uidl_supported = 0 AND m.message_id_header IS NOT NULL
         ORDER BY m.account_id, m.message_id_header, m.id",
    )?;
    let mut current: Option<(i64, String)> = None;
    let mut ids: Vec<i64> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let account_id: i64 = row.get(0)?;
        let mid: String = row.get(1)?;
        let id: i64 = row.get(2)?;
        if current.as_ref().is_some_and(|(a, m)| *a == account_id && *m == mid) {
            ids.push(id);
        } else {
            push_group(&mut groups, current.as_ref().map(|(a, _)| *a), &mut ids);
            current = Some((account_id, mid));
            ids.push(id);
        }
    }
    push_group(&mut groups, current.as_ref().map(|(a, _)| *a), &mut ids);

    // Message-IDが無い行は、見た目のメタデータが全て同じものを同一とみなす。
    let mut stmt = conn.prepare(
        "SELECT m.account_id, m.subject, m.from_addr, m.to_addr, m.date_header, m.id
         FROM messages m JOIN accounts a ON a.id = m.account_id
         WHERE a.uidl_supported = 0 AND m.message_id_header IS NULL
         ORDER BY m.id",
    )?;
    type Key = (i64, Option<String>, Option<String>, Option<String>, i64);
    let mut by_key: HashMap<Key, Vec<i64>> = HashMap::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let key = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?);
        by_key.entry(key).or_default().push(row.get(5)?);
    }
    let mut no_mid_groups: Vec<DuplicateGroup> = by_key
        .into_iter()
        .filter(|(_, ids)| ids.len() > 1)
        .map(|((account_id, ..), mut ids)| {
            ids.sort_unstable();
            DuplicateGroup {
                account_id,
                keep_id: ids[0],
                remove_ids: ids[1..].to_vec(),
            }
        })
        .collect();
    no_mid_groups.sort_by_key(|g| g.keep_id);
    groups.extend(no_mid_groups);

    Ok(groups)
}

fn push_group(groups: &mut Vec<DuplicateGroup>, account_id: Option<i64>, ids: &mut Vec<i64>) {
    if let (Some(account_id), true) = (account_id, ids.len() > 1) {
        groups.push(DuplicateGroup {
            account_id,
            keep_id: ids[0],
            remove_ids: ids[1..].to_vec(),
        });
    }
    ids.clear();
}

/// `groups`の重複を実際に消す。DB行・Maildirファイル・検索インデックスの全てから
/// 取り除く（`message_actions::purge_expired_batch`と同じ手順だが、消した
/// 行はもう一度同期されても重複判定で弾かれるため、`purged_uidls`への墓標は
/// 残さない）。
pub fn remove_duplicates(
    conn: &Connection,
    maildir_base: &Path,
    search_index: &SearchIndex,
    groups: &[DuplicateGroup],
) -> Result<DedupeSummary, DedupeError> {
    let mut summary = DedupeSummary::default();

    for chunk in groups.chunks(200) {
        // 先に残す側へ状態を引き継ぐ。
        for group in chunk {
            merge_state_into_keeper(conn, group)?;
        }

        let mut targets: Vec<(i64, i64, String)> = Vec::new(); // (id, account_id, uidl)
        for group in chunk {
            for id in &group.remove_ids {
                let uidl: String =
                    conn.query_row("SELECT uidl FROM messages WHERE id = ?1", [id], |r| r.get(0))?;
                targets.push((*id, group.account_id, uidl));
            }
        }

        let whole_batch = with_commit_retry(|| -> crate::search::Result<()> {
            for (id, ..) in &targets {
                search_index.delete_message(*id)?;
            }
            search_index.commit()
        });
        let removable: Vec<&(i64, i64, String)> = if whole_batch.is_ok() {
            targets.iter().collect()
        } else {
            targets
                .iter()
                .filter(|(id, ..)| {
                    with_commit_retry(|| -> crate::search::Result<()> {
                        search_index.delete_message(*id)?;
                        search_index.commit()
                    })
                    .is_ok()
                })
                .collect()
        };
        summary.skipped += targets.len() - removable.len();

        for (id, account_id, uidl) in &removable {
            let _ = maildir::remove(maildir_base, *account_id, uidl);
            conn.execute("DELETE FROM message_labels WHERE message_id = ?1", [id])?;
            conn.execute("DELETE FROM messages WHERE id = ?1", [id])?;
        }
        summary.removed += removable.len();
        summary.groups += chunk.len();
    }

    Ok(summary)
}

fn merge_state_into_keeper(conn: &Connection, group: &DuplicateGroup) -> rusqlite::Result<()> {
    let placeholders = group.remove_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let params: Vec<&dyn rusqlite::ToSql> =
        group.remove_ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();

    let (any_read, any_flagged): (i64, i64) = conn.query_row(
        &format!(
            "SELECT COALESCE(MAX(is_read), 0), COALESCE(MAX(is_flagged), 0)
             FROM messages WHERE id IN ({placeholders})"
        ),
        params.as_slice(),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    conn.execute(
        "UPDATE messages SET is_read = MAX(is_read, ?2), is_flagged = MAX(is_flagged, ?3)
         WHERE id = ?1",
        rusqlite::params![group.keep_id, any_read, any_flagged],
    )?;

    let mut all_params: Vec<&dyn rusqlite::ToSql> = vec![&group.keep_id];
    all_params.extend(params);
    conn.execute(
        &format!(
            "INSERT OR IGNORE INTO message_labels (message_id, label_id)
             SELECT ?1, label_id FROM message_labels WHERE message_id IN ({})",
            (2..=group.remove_ids.len() + 1)
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        all_params.as_slice(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::{self, NewMessage};
    use crate::db::open_in_memory;

    fn account(conn: &Connection, name: &str, uidl_supported: bool) -> i64 {
        let id = accounts::insert(
            conn,
            &NewAccount {
                name,
                host: "pop.example.com",
                port: 110,
                username: "u",
                use_tls: false,
            },
        )
        .unwrap();
        accounts::set_uidl_supported(conn, id, uidl_supported).unwrap();
        id
    }

    fn insert(conn: &Connection, account_id: i64, uidl: &str, mid: Option<&str>, subject: &str) -> i64 {
        messages::insert_new(
            conn,
            &NewMessage {
                account_id,
                uidl,
                message_id_header: mid,
                subject: Some(subject),
                from_name: None,
                from_addr: Some("a@example.com"),
                to_addr: Some("b@example.com"),
                date_header: 1000,
                size_bytes: 10,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn finds_and_removes_duplicates_only_for_fallback_accounts() {
        let conn = open_in_memory().unwrap();
        let search_index = SearchIndex::create_in_ram().unwrap();
        let dir = tempfile::tempdir().unwrap();

        let fallback = account(&conn, "fallback", false);
        let normal = account(&conn, "normal", true);

        let keep = insert(&conn, fallback, "h1", Some("m1"), "s");
        let dup1 = insert(&conn, fallback, "h2", Some("m1"), "s");
        let dup2 = insert(&conn, fallback, "h3", Some("m1"), "s");
        let other = insert(&conn, fallback, "h4", Some("m2"), "s");
        // Message-IDなし: メタデータが同じものだけを重複とみなす。
        let n1 = insert(&conn, fallback, "h5", None, "no mid");
        let n2 = insert(&conn, fallback, "h6", None, "no mid");
        let n3 = insert(&conn, fallback, "h7", None, "different");
        // UIDL対応アカウントの同一Message-IDは対象外。
        insert(&conn, normal, "u1", Some("m1"), "s");
        insert(&conn, normal, "u2", Some("m1"), "s");

        messages::set_read(&conn, dup2, true).unwrap();
        messages::set_flagged(&conn, dup1, true).unwrap();

        let groups = find_duplicates(&conn).unwrap();
        assert_eq!(groups.len(), 2);
        assert!(groups.contains(&DuplicateGroup {
            account_id: fallback,
            keep_id: keep,
            remove_ids: vec![dup1, dup2],
        }));
        assert!(groups.contains(&DuplicateGroup {
            account_id: fallback,
            keep_id: n1,
            remove_ids: vec![n2],
        }));

        let summary = remove_duplicates(&conn, dir.path(), &search_index, &groups).unwrap();
        assert_eq!(summary, DedupeSummary { groups: 2, removed: 3, skipped: 0 });

        let remaining: Vec<i64> = messages::list_recent(&conn, None, None, 100)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert!(remaining.contains(&keep) && remaining.contains(&other));
        assert!(remaining.contains(&n1) && remaining.contains(&n3));
        assert!(!remaining.contains(&dup1) && !remaining.contains(&dup2) && !remaining.contains(&n2));
        // normalアカウントの2件は無傷。
        assert_eq!(remaining.len(), 4 + 2);

        // 既読/スターが残す側に引き継がれている。
        let kept = messages::get(&conn, keep).unwrap().unwrap();
        assert!(kept.is_read && kept.is_flagged);

        assert!(find_duplicates(&conn).unwrap().is_empty());
    }
}
