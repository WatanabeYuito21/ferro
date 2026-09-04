use std::sync::Mutex;

use ferro_core::db::accounts::Account;
use ferro_core::db::messages::Message;
use ferro_core::db::{self, Connection};
use ferro_core::paths;
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::fs::create_dir_all(paths::app_data_dir()).expect("failed to create app data directory");
    let conn = db::open(&paths::db_path()).expect("failed to open ferro database");

    tauri::Builder::default()
        .manage(AppState {
            conn: Mutex::new(conn),
        })
        .invoke_handler(tauri::generate_handler![list_accounts, list_messages])
        .run(tauri::generate_context!())
        .expect("error while running the Ferro desktop app");
}
