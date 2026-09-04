use std::sync::Mutex;

use ferro_core::account_setup;
use ferro_core::credentials;
use ferro_core::db::accounts::{Account, NewAccount};
use ferro_core::db::messages::Message;
use ferro_core::db::{self, Connection};
use ferro_core::sync::SyncSummary;
use ferro_core::{paths, sync};
use serde::Serialize;
use tauri::State;

/// CLIとGUIが同じDBファイル(`ferro_core::paths::db_path`)を共有するため、
/// スキーマやクエリロジックはすべてferro-core側にあり、ここは薄いコマンド層のみ。
struct AppState {
    conn: Mutex<Connection>,
}

#[derive(Serialize)]
struct AccountView {
    id: i64,
    name: String,
    host: String,
    port: u16,
    username: String,
    use_tls: bool,
}

impl From<Account> for AccountView {
    fn from(a: Account) -> Self {
        AccountView {
            id: a.id,
            name: a.name,
            host: a.host,
            port: a.port,
            username: a.username,
            use_tls: a.use_tls,
        }
    }
}

#[derive(Serialize)]
struct MessageView {
    id: i64,
    account_id: i64,
    subject: Option<String>,
    from_name: Option<String>,
    from_addr: Option<String>,
    date_header: i64,
    is_read: bool,
    is_flagged: bool,
}

impl From<Message> for MessageView {
    fn from(m: Message) -> Self {
        MessageView {
            id: m.id,
            account_id: m.account_id,
            subject: m.subject,
            from_name: m.from_name,
            from_addr: m.from_addr,
            date_header: m.date_header,
            is_read: m.is_read,
            is_flagged: m.is_flagged,
        }
    }
}

#[derive(Serialize)]
struct SyncSummaryView {
    fetched: u32,
    remaining: u32,
    ended_early: bool,
}

impl From<SyncSummary> for SyncSummaryView {
    fn from(s: SyncSummary) -> Self {
        SyncSummaryView {
            fetched: s.fetched,
            remaining: s.remaining,
            ended_early: s.ended_early,
        }
    }
}

#[tauri::command]
fn list_accounts(state: State<AppState>) -> Result<Vec<AccountView>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::accounts::list(&conn)
        .map(|accounts| accounts.into_iter().map(AccountView::from).collect())
        .map_err(|e| e.to_string())
}

/// 新着順（date_header降順）のキーセットページネーション一覧。
/// `account_id`を省略すると全アカウント横断、`before`で次ページを取得する。
#[tauri::command]
fn list_messages(
    state: State<AppState>,
    account_id: Option<i64>,
    before: Option<i64>,
    limit: u32,
) -> Result<Vec<MessageView>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::messages::list_recent(&conn, account_id, before, limit)
        .map(|messages| messages.into_iter().map(MessageView::from).collect())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn add_account(
    state: State<AppState>,
    name: String,
    host: String,
    port: u16,
    username: String,
    use_tls: bool,
    password: String,
) -> Result<AccountView, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let new_account = NewAccount {
        name: &name,
        host: &host,
        port,
        username: &username,
        use_tls,
    };
    account_setup::create(&conn, &new_account, &password)
        .map(AccountView::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_account(state: State<AppState>, account_id: i64) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    account_setup::remove(&conn, account_id).map_err(|e| e.to_string())
}

/// アカウントを同期する。ネットワークI/Oを伴うため呼び出し中は他のコマンドが
/// 同じDB接続を待つことになるが、同期中に一覧を触るユースケースは薄いため
/// 現状はこの単純なMutexで十分と判断している（バックグラウンドキュー化は将来課題）。
#[tauri::command]
fn sync_account(
    state: State<AppState>,
    account_id: i64,
    limit: Option<u32>,
    allow_plaintext: bool,
) -> Result<SyncSummaryView, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let account = db::accounts::get(&conn, account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("account #{account_id} not found"))?;
    let password = credentials::get_password(account_id).map_err(|e| {
        format!(
            "failed to read the password from the OS keyring: {e}\n\
             see CLAUDE.md's keyring troubleshooting notes."
        )
    })?;

    sync::sync_account_with_limit(
        &conn,
        &paths::maildir_dir(),
        &account,
        &password,
        allow_plaintext,
        limit,
    )
    .map(SyncSummaryView::from)
    .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::fs::create_dir_all(paths::app_data_dir()).expect("failed to create app data directory");
    let conn = db::open(&paths::db_path()).expect("failed to open ferro database");

    tauri::Builder::default()
        .manage(AppState {
            conn: Mutex::new(conn),
        })
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            list_messages,
            add_account,
            remove_account,
            sync_account
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Ferro desktop app");
}
