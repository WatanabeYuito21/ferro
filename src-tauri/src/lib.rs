use std::sync::Mutex;
use std::time::Duration;

use ferro_core::account_config::{self, AccountConfig};
use ferro_core::account_setup;
use ferro_core::credentials;
use ferro_core::db::accounts::Account;
use ferro_core::db::messages::Message;
use ferro_core::db::{self, Connection};
use ferro_core::mail::attachments::{self, AttachmentInfo};
use ferro_core::mail::parse::extract_plain_text_body;
use ferro_core::search::SearchIndex;
use ferro_core::sync::SyncSummary;
use ferro_core::{maildir, paths, reindex, sync};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

/// 自動バックグラウンド同期の間隔。設定画面はまだ無いのでハードコードしている。
const BACKGROUND_SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// 1アカウントあたり1回の自動同期で取得する上限（バックログが多いアカウントでも
/// 1サイクルで固まらないように）。残りは次のサイクルで拾われる。
const BACKGROUND_SYNC_LIMIT: u32 = 200;

/// CLIとGUIが同じDBファイル(`ferro_core::paths::db_path`)を共有するため、
/// スキーマやクエリロジックはすべてferro-core側にあり、ここは薄いコマンド層のみ。
/// `search_index`はTantivyの`IndexWriter`を自前のMutexで直列化しているので
/// (`ferro_core::search::SearchIndex`参照)、ここで別途Mutexに包む必要はない。
struct AppState {
    conn: Mutex<Connection>,
    search_index: SearchIndex,
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

#[derive(Serialize, Clone)]
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

#[derive(Serialize)]
struct AttachmentView {
    index: usize,
    filename: Option<String>,
    content_type: Option<String>,
    size: usize,
}

impl From<AttachmentInfo> for AttachmentView {
    fn from(a: AttachmentInfo) -> Self {
        AttachmentView {
            index: a.index,
            filename: a.filename,
            content_type: a.content_type,
            size: a.size,
        }
    }
}

#[derive(Serialize)]
struct MessageDetailView {
    id: i64,
    subject: Option<String>,
    from_name: Option<String>,
    from_addr: Option<String>,
    to_addr: Option<String>,
    date_header: i64,
    is_read: bool,
    is_flagged: bool,
    body: Option<String>,
    attachments: Vec<AttachmentView>,
}

/// メッセージのDB行とMaildir上の生バイト列を両方取ってくる。
/// メッセージ詳細表示・添付保存の両方で必要になる共通の下ごしらえ。
fn load_message_and_raw(conn: &Connection, message_id: i64) -> Result<(Message, Vec<u8>), String> {
    let message = db::messages::get(conn, message_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("message #{message_id} not found"))?;
    let raw = maildir::load(&paths::maildir_dir(), message.account_id, &message.uidl)
        .map_err(|e| e.to_string())?;
    Ok((message, raw))
}

/// 本文（プレーンテキストのみ。HTML本文はタグを剥がしたテキストに変換済み）と
/// 添付ファイルの一覧を返す。添付の中身はここでは返さず`save_attachment`で個別に取得する。
#[tauri::command]
fn get_message_detail(state: State<AppState>, message_id: i64) -> Result<MessageDetailView, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let (message, raw) = load_message_and_raw(&conn, message_id)?;

    Ok(MessageDetailView {
        id: message.id,
        subject: message.subject,
        from_name: message.from_name,
        from_addr: message.from_addr,
        to_addr: message.to_addr,
        date_header: message.date_header,
        is_read: message.is_read,
        is_flagged: message.is_flagged,
        body: extract_plain_text_body(&raw),
        attachments: attachments::list_attachments(&raw)
            .into_iter()
            .map(AttachmentView::from)
            .collect(),
    })
}

#[tauri::command]
fn set_read(state: State<AppState>, message_id: i64, is_read: bool) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::messages::set_read(&conn, message_id, is_read).map_err(|e| e.to_string())
}

#[tauri::command]
fn set_flagged(state: State<AppState>, message_id: i64, is_flagged: bool) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    db::messages::set_flagged(&conn, message_id, is_flagged).map_err(|e| e.to_string())
}

/// 論理削除する/復元する（POP3サーバー側のDELEとは独立したローカルの削除フラグ）。
/// 削除時は検索インデックスからも取り除く（`ferro_core::message_actions::set_deleted`参照）。
#[tauri::command]
fn set_deleted(state: State<AppState>, message_id: i64, is_deleted: bool) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    ferro_core::message_actions::set_deleted(&conn, &state.search_index, message_id, is_deleted)
        .map_err(|e| e.to_string())
}

/// `get_message_detail`が返した添付の`index`を指定して、フロント側が
/// (`@tauri-apps/plugin-dialog`の保存ダイアログで選んだ)`destination_path`に書き出す。
#[tauri::command]
fn save_attachment(
    state: State<AppState>,
    message_id: i64,
    attachment_index: usize,
    destination_path: String,
) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let (_message, raw) = load_message_and_raw(&conn, message_id)?;
    let bytes = attachments::extract_attachment_bytes(&raw, attachment_index)
        .ok_or_else(|| format!("attachment #{attachment_index} not found"))?;
    std::fs::write(&destination_path, bytes).map_err(|e| e.to_string())
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

/// アカウントを追加する: `accounts.toml`にエントリを追記してからDBに反映し、
/// パスワードをOS keyringに保存する（`account_setup::add`参照）。
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
    let new_account = AccountConfig {
        name,
        host,
        port,
        username,
        use_tls,
    };
    account_setup::add(&conn, &paths::accounts_config_path(), &new_account, &password)
        .map(AccountView::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_account(state: State<AppState>, account_id: i64) -> Result<(), String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    account_setup::remove(
        &conn,
        &paths::maildir_dir(),
        &state.search_index,
        &paths::accounts_config_path(),
        account_id,
    )
    .map_err(|e| e.to_string())
}

/// `accounts.toml`を読み直してDBに反映する。アプリ起動中に手でファイルを
/// 編集した場合に使う（他は起動時に一度だけ自動で反映する）。
#[tauri::command]
fn reload_accounts_config(state: State<AppState>) -> Result<Vec<AccountView>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    account_config::load_and_reconcile(&conn, &paths::accounts_config_path())
        .map(|accounts| accounts.into_iter().map(AccountView::from).collect())
        .map_err(|e| e.to_string())
}

/// フロント側で「設定ファイルはここにあります」と案内表示するためのパス。
#[tauri::command]
fn accounts_config_path() -> String {
    paths::accounts_config_path().display().to_string()
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
        &state.search_index,
    )
    .map(SyncSummaryView::from)
    .map_err(|e| e.to_string())
}

/// 全文検索インデックスを検索し、ヒットしたメッセージをスコア順に返す。
#[tauri::command]
fn search_messages(state: State<AppState>, query: String, limit: usize) -> Result<Vec<MessageView>, String> {
    let ids = state.search_index.search(&query, limit).map_err(|e| e.to_string())?;
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    ids.into_iter()
        .filter_map(|id| db::messages::get(&conn, id).transpose())
        .map(|result| result.map(MessageView::from).map_err(|e| e.to_string()))
        .collect()
}

/// 全メッセージから検索インデックスを作り直す（DB/Maildirから再構築可能な派生キャッシュ）。
#[tauri::command]
fn reindex_all(state: State<AppState>) -> Result<usize, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    reindex::reindex_all(&conn, &paths::maildir_dir(), &state.search_index).map_err(|e| e.to_string())
}

#[derive(Serialize, Clone)]
struct BackgroundSyncEvent {
    account_id: i64,
    #[serde(flatten)]
    summary: Option<SyncSummaryView>,
    error: Option<String>,
}

/// バックグラウンド定期同期のループ本体。専用のOSスレッドで動かす
/// （ferro-coreの同期処理は同期(ブロッキング)関数なので、tokioワーカースレッドを
/// 塞がないよう素のstd::threadを使う）。
///
/// `allow_plaintext`は常にfalseで呼ぶ: ユーザーが手動でSyncボタンを押した
/// わけではない自動実行で、平文接続の暗黙的な選択はしない
/// （`use_tls=false`のアカウントは自動同期の対象外になり、手動同期のみ届く）。
fn spawn_background_sync(app_handle: AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(BACKGROUND_SYNC_INTERVAL);
            run_background_sync_once(&app_handle);
        }
    });
}

fn run_background_sync_once(app_handle: &AppHandle) {
    let state = app_handle.state::<AppState>();

    let accounts = {
        let Ok(conn) = state.conn.lock() else {
            return;
        };
        match db::accounts::list(&conn) {
            Ok(accounts) => accounts,
            Err(_) => return,
        }
    };

    for account in accounts {
        let event = match credentials::get_password(account.id) {
            Err(e) => BackgroundSyncEvent {
                account_id: account.id,
                summary: None,
                error: Some(format!("failed to read password from OS keyring: {e}")),
            },
            Ok(password) => {
                let Ok(conn) = state.conn.lock() else {
                    return;
                };
                let result = sync::sync_account_with_limit(
                    &conn,
                    &paths::maildir_dir(),
                    &account,
                    &password,
                    false,
                    Some(BACKGROUND_SYNC_LIMIT),
                    &state.search_index,
                );
                match result {
                    Ok(summary) => BackgroundSyncEvent {
                        account_id: account.id,
                        summary: Some(SyncSummaryView::from(summary)),
                        error: None,
                    },
                    Err(e) => BackgroundSyncEvent {
                        account_id: account.id,
                        summary: None,
                        error: Some(e.to_string()),
                    },
                }
            }
        };
        let _ = app_handle.emit("background-sync", event);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::fs::create_dir_all(paths::app_data_dir()).expect("failed to create app data directory");
    let conn = db::open(&paths::db_path()).expect("failed to open ferro database");
    let search_index =
        SearchIndex::open_or_create(&paths::search_index_dir()).expect("failed to open search index");

    // accounts.tomlが正の情報源。存在すれば起動時にDBへ反映する（無ければ空扱い）。
    // 手編集ファイルの構文ミス等でここが失敗してもGUI自体は起動させる
    // （直近の反映結果＝DBの内容のまま起動し、修正後は`reload_accounts_config`
    // か再起動で反映される）。
    if let Err(e) = account_config::load_and_reconcile(&conn, &paths::accounts_config_path()) {
        eprintln!("warning: failed to load/reconcile accounts.toml: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            conn: Mutex::new(conn),
            search_index,
        })
        .setup(|app| {
            spawn_background_sync(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            list_messages,
            add_account,
            remove_account,
            sync_account,
            search_messages,
            reindex_all,
            get_message_detail,
            save_attachment,
            set_read,
            set_flagged,
            set_deleted,
            reload_accounts_config,
            accounts_config_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Ferro desktop app");
}
