//! バックグラウンドスレッド（同期・検索インデックスのキャッチアップ・メール保持期間の
//! クリーンアップ）。`src-tauri/src/lib.rs`の対応する関数（`spawn_background_sync`/
//! `spawn_initial_search_catchup`/`spawn_retention_cleanup`）の移植。Tauriの
//! `AppHandle::emit`の代わりに`std::sync::mpsc::Sender<BackgroundEvent>`で
//! メインの描画/入力ループへ通知する。
//!
//! GUI版と同じ理由（同期・検索キャッチアップ・保持期間クリーンアップの重い処理が
//! DBの書き込みロックを長時間握ると、UI用の読み取り接続まで巻き込んで描画/入力が
//! 止まる）で、書き込み系はここでまとめて`write_conn`だけを使う。UIの描画時の
//! 読み取り(`app.rs`)は別の`read_conn`を使うことで競合を避ける。

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ferro_core::db::{self, Connection};
use ferro_core::search::SearchIndex;
use ferro_core::sync::SyncSummary;
use ferro_core::{credentials, paths, reindex, sync};

/// バックグラウンドスレッドからメインループへの通知。
pub enum BackgroundEvent {
    SyncProgress { account_id: i64, fetched: u32, total: u32 },
    SyncFinished { account_id: i64, result: Result<SyncSummary, String> },
    RetentionPurged { count: usize },
    ManualPurgeFinished { result: Result<usize, String> },
    ReindexFinished { result: Result<usize, String> },
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_secs() as i64
}

/// 保持期間チェックの間隔（GUI版`spawn_retention_cleanup`と同じ6時間）。
const RETENTION_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// 検索インデックス初回キャッチアップの1バッチごとの待機（GUI版と同じ）。
const CATCHUP_STEP_DELAY: Duration = Duration::from_millis(200);

/// 同期・検索キャッチアップ・保持期間クリーンアップをまとめて行う唯一の
/// バックグラウンドスレッドを起動する。GUI版は3本のスレッドに分かれているが、
/// TUIでは管理を簡単にするため1本にまとめ、内部でそれぞれの実行間隔を
/// 個別に管理する。
pub fn spawn(write_conn: Arc<Mutex<Connection>>, search_index: Arc<SearchIndex>, tx: Sender<BackgroundEvent>) {
    std::thread::spawn(move || {
        // 起動直後: 検索インデックスの未投入分を一気に片付ける
        // (`spawn_initial_search_catchup`相当)。
        drain_search_catchup(&write_conn, &search_index);

        // 起動直後にも一度保持期間クリーンアップを走らせておく
        // （`Instant::now() - RETENTION_CHECK_INTERVAL`にすることで、
        // 最初のループで即座に条件を満たすようにする）。
        let mut retention_last_run = Instant::now()
            .checked_sub(RETENTION_CHECK_INTERVAL)
            .unwrap_or_else(Instant::now);

        loop {
            let interval = current_sync_interval(&write_conn);
            std::thread::sleep(interval);

            sync_all_accounts_once(&write_conn, &search_index, &tx);
            step_search_catchup(&write_conn, &search_index);

            if retention_last_run.elapsed() >= RETENTION_CHECK_INTERVAL {
                run_retention_cleanup_once(&write_conn, &search_index, &tx);
                retention_last_run = Instant::now();
            }
        }
    });
}

fn current_sync_interval(write_conn: &Arc<Mutex<Connection>>) -> Duration {
    let minutes = write_conn
        .lock()
        .ok()
        .and_then(|conn| db::settings::get(&conn).ok())
        .map(|s| s.sync_interval_minutes)
        .filter(|m| *m > 0)
        .unwrap_or(5);
    Duration::from_secs(minutes as u64 * 60)
}

/// 全アカウントを1回同期する（`run_background_sync_once`相当）。`use_tls=false`の
/// アカウントも対象にする（平文専用という選択自体がアカウント作成時の明示的
/// opt-inのため。今セッションで直した仕様をそのまま踏襲）。
///
/// バックグラウンドスレッドの定期実行だけでなく、ユーザーが手動で同期を
/// 実行した時（`App::sync_now`）にも同じ関数・同じ`Sender`（複製したもの）を使う。
pub fn sync_all_accounts_once(
    write_conn: &Arc<Mutex<Connection>>,
    search_index: &Arc<SearchIndex>,
    tx: &Sender<BackgroundEvent>,
) {
    let accounts = {
        let Ok(conn) = write_conn.lock() else { return };
        match db::accounts::list(&conn) {
            Ok(accounts) => accounts,
            Err(_) => return,
        }
    };

    for account in accounts {
        let account_id = account.id;
        let result = (|| -> Result<SyncSummary, String> {
            let password = credentials::get_password(account_id).map_err(|e| {
                format!("failed to read the password from the OS keyring: {e}")
            })?;
            let conn = write_conn.lock().map_err(|e| e.to_string())?;
            sync::sync_account_with_limit(
                &conn,
                &paths::maildir_dir(),
                &account,
                &password,
                !account.use_tls,
                Some(200),
                search_index,
                |fetched, total| {
                    let _ = tx.send(BackgroundEvent::SyncProgress { account_id, fetched, total });
                },
            )
            .map_err(|e| e.to_string())
        })();
        let _ = tx.send(BackgroundEvent::SyncFinished { account_id, result });
    }
}

/// 起動直後、検索インデックス未投入分をラップ(周回)しながら一気に片付ける
/// （`spawn_initial_search_catchup`相当。詳細はそちらのドキュメント参照）。
fn drain_search_catchup(write_conn: &Arc<Mutex<Connection>>, search_index: &Arc<SearchIndex>) {
    const MAX_ITERATIONS: u32 = 5000;
    let mut after_id = 0i64;
    let mut indexed_this_lap = 0usize;
    for _ in 0..MAX_ITERATIONS {
        let result = {
            let Ok(conn) = write_conn.lock() else { return };
            reindex::catch_up_unindexed(&conn, &paths::maildir_dir(), search_index, after_id)
        };
        match result {
            Ok((indexed, Some(next))) => {
                indexed_this_lap += indexed;
                after_id = next;
                std::thread::sleep(CATCHUP_STEP_DELAY);
            }
            Ok((indexed, None)) => {
                indexed_this_lap += indexed;
                if indexed_this_lap == 0 {
                    return; // 1周して何も投入できなかった＝完全に追いついた
                }
                after_id = 0;
                indexed_this_lap = 0;
                std::thread::sleep(CATCHUP_STEP_DELAY);
            }
            Err(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }
}

/// 同期サイクルのたびに少量だけ拾い直す（`step_search_catchup`相当）。
fn step_search_catchup(write_conn: &Arc<Mutex<Connection>>, search_index: &Arc<SearchIndex>) {
    let Ok(conn) = write_conn.lock() else { return };
    let _ = reindex::catch_up_unindexed(&conn, &paths::maildir_dir(), search_index, 0);
}

/// メール保持期間が切れたメッセージを完全に削除する（`run_retention_cleanup_once`相当）。
fn run_retention_cleanup_once(
    write_conn: &Arc<Mutex<Connection>>,
    search_index: &Arc<SearchIndex>,
    tx: &Sender<BackgroundEvent>,
) {
    let retention_days = {
        let Ok(conn) = write_conn.lock() else { return };
        match db::settings::get(&conn) {
            Ok(settings) => settings.retention_days,
            Err(_) => return,
        }
    };
    if retention_days <= 0 {
        return;
    }
    let cutoff = now_unix() - retention_days * 24 * 60 * 60;

    let mut total = 0usize;
    loop {
        let purged = {
            let Ok(conn) = write_conn.lock() else { return };
            match ferro_core::message_actions::purge_expired_batch(
                &conn,
                &paths::maildir_dir(),
                search_index,
                cutoff,
                500,
            ) {
                Ok(n) => n,
                Err(_) => return,
            }
        };
        if purged == 0 {
            break;
        }
        total += purged;
    }
    if total > 0 {
        let _ = tx.send(BackgroundEvent::RetentionPurged { count: total });
    }
}

/// 保持日数が切れたメッセージを即座に削除する（設定画面の「今すぐ整理する」用）。
/// バックグラウンドスレッドとは独立して、メインスレッドから直接呼べるようにする。
pub fn purge_expired_now(write_conn: &Arc<Mutex<Connection>>, search_index: &Arc<SearchIndex>) -> Result<usize, String> {
    let retention_days = {
        let conn = write_conn.lock().map_err(|e| e.to_string())?;
        db::settings::get(&conn).map_err(|e| e.to_string())?.retention_days
    };
    if retention_days <= 0 {
        return Ok(0);
    }
    let cutoff = now_unix() - retention_days * 24 * 60 * 60;

    let mut total = 0usize;
    loop {
        let purged = {
            let conn = write_conn.lock().map_err(|e| e.to_string())?;
            ferro_core::message_actions::purge_expired_batch(&conn, &paths::maildir_dir(), search_index, cutoff, 500)
                .map_err(|e| e.to_string())?
        };
        if purged == 0 {
            break;
        }
        total += purged;
    }
    Ok(total)
}

/// 検索インデックスを全件作り直す（設定画面の「検索インデックス再構築」用）。
pub fn reindex_all_now(write_conn: &Arc<Mutex<Connection>>, search_index: &Arc<SearchIndex>) -> Result<usize, String> {
    let conn = write_conn.lock().map_err(|e| e.to_string())?;
    reindex::reindex_all(&conn, &paths::maildir_dir(), search_index).map_err(|e| e.to_string())
}
