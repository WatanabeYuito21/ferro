//! アカウントごとの直近の同期結果（migration 0011）。`sync::sync_account_with_limit`が
//! 毎回記録し、GUIの設定画面・TUIの設定画面・`ferro account list`が表示する。

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncStatus {
    pub account_id: i64,
    pub last_attempt_at: i64,
    pub last_success_at: Option<i64>,
    /// 直近の同期が失敗していればそのエラー。
    pub last_error: Option<String>,
    /// 直近に成功した同期で取得した件数・残り件数・途中で打ち切ったか。
    pub last_fetched: u32,
    pub last_remaining: u32,
    pub last_ended_early: bool,
    pub consecutive_failures: u32,
}

const COLUMNS: &str = "account_id, last_attempt_at, last_success_at, last_error, last_fetched, \
                       last_remaining, last_ended_early, consecutive_failures";

fn from_row(row: &Row) -> rusqlite::Result<SyncStatus> {
    Ok(SyncStatus {
        account_id: row.get(0)?,
        last_attempt_at: row.get(1)?,
        last_success_at: row.get(2)?,
        last_error: row.get(3)?,
        last_fetched: row.get(4)?,
        last_remaining: row.get(5)?,
        last_ended_early: row.get(6)?,
        consecutive_failures: row.get(7)?,
    })
}

pub fn record_success(
    conn: &Connection,
    account_id: i64,
    at: i64,
    fetched: u32,
    remaining: u32,
    ended_early: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO account_sync_status (account_id, last_attempt_at, last_success_at, last_error,
                                          last_fetched, last_remaining, last_ended_early, consecutive_failures)
         VALUES (?1, ?2, ?2, NULL, ?3, ?4, ?5, 0)
         ON CONFLICT(account_id) DO UPDATE SET
             last_attempt_at = excluded.last_attempt_at,
             last_success_at = excluded.last_success_at,
             last_error = NULL,
             last_fetched = excluded.last_fetched,
             last_remaining = excluded.last_remaining,
             last_ended_early = excluded.last_ended_early,
             consecutive_failures = 0",
        params![account_id, at, fetched, remaining, ended_early],
    )?;
    Ok(())
}

/// 失敗を記録する。直近に成功した同期の結果（`last_success_at`/`last_fetched`等）は残す。
pub fn record_failure(conn: &Connection, account_id: i64, at: i64, error: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO account_sync_status (account_id, last_attempt_at, last_error, consecutive_failures)
         VALUES (?1, ?2, ?3, 1)
         ON CONFLICT(account_id) DO UPDATE SET
             last_attempt_at = excluded.last_attempt_at,
             last_error = excluded.last_error,
             consecutive_failures = consecutive_failures + 1",
        params![account_id, at, error],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, account_id: i64) -> rusqlite::Result<Option<SyncStatus>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM account_sync_status WHERE account_id = ?1"),
        params![account_id],
        from_row,
    )
    .optional()
}

/// 全アカウント分（アカウント一覧に並べる用）。
pub fn list(conn: &Connection) -> rusqlite::Result<HashMap<i64, SyncStatus>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM account_sync_status"))?;
    let rows = stmt.query_map([], from_row)?;
    rows.map(|row| row.map(|status| (status.account_id, status))).collect()
}

/// `now`から見た相対時刻（「5分前」等）。CLI/TUIはタイムゾーンを持たないため（README参照）、
/// 絶対時刻ではなくこちらで出す。
pub fn ago(now: i64, at: i64) -> String {
    let elapsed = (now - at).max(0);
    match elapsed {
        0..60 => "たった今".to_string(),
        60..3600 => format!("{}分前", elapsed / 60),
        3600..86400 => format!("{}時間前", elapsed / 3600),
        _ => format!("{}日前", elapsed / 86400),
    }
}

/// CLI/TUIのアカウント一覧に出す1行の説明（GUIは各項目を自前で整形する）。
pub fn describe(status: Option<&SyncStatus>, now: i64) -> String {
    let Some(status) = status else {
        return "まだ同期していません".to_string();
    };
    let last_success = status.last_success_at.map(|at| ago(now, at));
    match &status.last_error {
        Some(error) => format!(
            "失敗: {}（{}回連続、最後の成功: {}）: {error}",
            ago(now, status.last_attempt_at),
            status.consecutive_failures,
            last_success.as_deref().unwrap_or("なし"),
        ),
        None => {
            let mut text = format!(
                "成功: {}（{}件取得、残り{}件）",
                last_success.as_deref().unwrap_or("-"),
                status.last_fetched,
                status.last_remaining
            );
            if status.last_ended_early {
                text.push_str("、切断が続いたため途中で打ち切り");
            }
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::open_in_memory;

    fn make_account(conn: &Connection, name: &str) -> i64 {
        accounts::insert(
            conn,
            &NewAccount { name, host: "pop.example.com", port: 995, username: "u", use_tls: true },
        )
        .unwrap()
    }

    #[test]
    fn failures_count_up_and_keep_the_last_success() {
        let conn = open_in_memory().unwrap();
        let id = make_account(&conn, "a");
        assert_eq!(get(&conn, id).unwrap(), None);

        record_failure(&conn, id, 100, "connection closed").unwrap();
        let status = get(&conn, id).unwrap().unwrap();
        assert_eq!((status.last_success_at, status.consecutive_failures), (None, 1));

        record_success(&conn, id, 200, 12, 3, true).unwrap();
        record_failure(&conn, id, 300, "timeout").unwrap();
        record_failure(&conn, id, 400, "timeout again").unwrap();
        let status = get(&conn, id).unwrap().unwrap();
        assert_eq!(
            status,
            SyncStatus {
                account_id: id,
                last_attempt_at: 400,
                last_success_at: Some(200),
                last_error: Some("timeout again".to_string()),
                last_fetched: 12,
                last_remaining: 3,
                last_ended_early: true,
                consecutive_failures: 2,
            }
        );

        record_success(&conn, id, 500, 0, 0, false).unwrap();
        let status = get(&conn, id).unwrap().unwrap();
        assert_eq!((status.last_error, status.consecutive_failures, status.last_success_at), (None, 0, Some(500)));
    }

    #[test]
    fn rows_go_away_with_the_account() {
        let conn = open_in_memory().unwrap();
        let a = make_account(&conn, "a");
        let b = make_account(&conn, "b");
        record_success(&conn, a, 1, 0, 0, false).unwrap();
        record_success(&conn, b, 1, 0, 0, false).unwrap();
        accounts::delete(&conn, a).unwrap();
        assert_eq!(list(&conn).unwrap().keys().copied().collect::<Vec<_>>(), vec![b]);
    }

    #[test]
    fn describe_reads_naturally() {
        assert_eq!(describe(None, 0), "まだ同期していません");
        let mut status = SyncStatus {
            account_id: 1,
            last_attempt_at: 1000,
            last_success_at: Some(1000),
            last_error: None,
            last_fetched: 5,
            last_remaining: 0,
            last_ended_early: false,
            consecutive_failures: 0,
        };
        assert_eq!(describe(Some(&status), 1000 + 300), "成功: 5分前（5件取得、残り0件）");
        status.last_error = Some("connection closed by server".to_string());
        status.consecutive_failures = 3;
        status.last_attempt_at = 1000 + 7200;
        assert_eq!(
            describe(Some(&status), 1000 + 7200 + 10),
            "失敗: たった今（3回連続、最後の成功: 2時間前）: connection closed by server"
        );
    }
}
