use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ferro_core::account_config::{self, AccountConfig};
use ferro_core::account_setup;
use ferro_core::color_rules::{self, ColorRule};
use ferro_core::credentials;
use ferro_core::db::accounts::Account;
use ferro_core::db::labels::Label;
use ferro_core::db::messages::{Folder, Message};
use ferro_core::db::settings::Settings;
use ferro_core::db::{self, Connection};
use ferro_core::mail::attachments::{self, AttachmentInfo};
use ferro_core::mail::parse::extract_plain_text_body;
use ferro_core::search::SearchIndex;
use ferro_core::sync::SyncSummary;
use ferro_core::{maildir, paths, reindex, sync};
use serde::{Deserialize, Serialize};
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, State};

/// 現在時刻(Unixエポック秒)。フォルダの受信箱判定(スヌーズ期限切れ)・
/// スヌーズのプリセット計算に使う。`ferro_core::db::now_unix`は
/// `pub(crate)`でクレート外から使えないためここに小さく複製する。
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before the Unix epoch")
        .as_secs() as i64
}

/// 1アカウントあたり1回の自動同期で取得する上限（バックログが多いアカウントでも
/// 1サイクルで固まらないように）。残りは次のサイクルで拾われる。
const BACKGROUND_SYNC_LIMIT: u32 = 200;

/// CLIとGUIが同じDBファイル(`ferro_core::paths::db_path`)を共有するため、
/// スキーマやクエリロジックはすべてferro-core側にあり、ここは薄いコマンド層のみ。
/// `search_index`はTantivyの`IndexWriter`を自前のMutexで直列化しているので
/// (`ferro_core::search::SearchIndex`参照)、ここで別途Mutexに包む必要はない。
///
/// DB接続は書き込み用/読み取り用の2本を持つ（同じファイルを指す別々の
/// `Connection`。`db::open`がWALモードを有効化済みなので、書き込み側が
/// トランザクションを持っていても読み取り側は最後にcommitされた状態を
/// ブロックされずに読める）。Sync（手動・5分間隔の自動同期どちらも）は
/// ネットワークI/Oを含み数十秒〜数分かかることがあり、以前は単一の
/// `Mutex<Connection>`を共有していたため、その間`list_messages`等の
/// 読み取り系コマンドまで巻き込まれて全部待たされ、アプリごと「応答なし」に
/// 見える原因になっていた（実際に踏んだ罠）。書き込み・変更系
/// （sync/add_account/remove_account/reload_accounts_config/set_*）は
/// `write_conn`、それ以外の純粋な読み取り系（list_*/get_message_detail/
/// save_attachment/search_messages/reindex_all）は`read_conn`を使う。
struct AppState {
    write_conn: Mutex<Connection>,
    read_conn: Mutex<Connection>,
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

#[derive(Serialize, Clone)]
struct LabelView {
    id: i64,
    name: String,
    color: String,
}

impl From<Label> for LabelView {
    fn from(l: Label) -> Self {
        LabelView {
            id: l.id,
            name: l.name,
            color: l.color,
        }
    }
}

#[derive(Serialize)]
struct LabelWithCountView {
    id: i64,
    name: String,
    color: String,
    count: i64,
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
    is_archived: bool,
    snoozed_until: Option<i64>,
    attachment_count: i64,
    preview: Option<String>,
    labels: Vec<LabelView>,
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
            is_archived: m.is_archived,
            snoozed_until: m.snoozed_until,
            attachment_count: m.attachment_count,
            preview: m.preview,
            // ラベルは一覧側でまとめて取得して詰める(`attach_labels`参照。
            // メッセージ1件ごとにラベルを引くとN+1になるため)。
            labels: Vec::new(),
        }
    }
}

/// `messages`をまとめて`MessageView`に変換し、ラベルを1回のクエリで詰める
/// (`db::labels::for_messages`でN+1を避ける)。一覧・検索・ラベル別一覧の
/// 各コマンドで共通に使う。
fn attach_labels(conn: &Connection, messages: Vec<Message>) -> Result<Vec<MessageView>, String> {
    let ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
    let mut label_map: HashMap<i64, Vec<Label>> =
        db::labels::for_messages(conn, &ids).map_err(|e| e.to_string())?;

    Ok(messages
        .into_iter()
        .map(|m| {
            let labels = label_map.remove(&m.id).unwrap_or_default();
            let mut view = MessageView::from(m);
            view.labels = labels.into_iter().map(LabelView::from).collect();
            view
        })
        .collect())
}

/// Sync進捗（"30/1000"のような表示用）。1バッチ完了するごとに発火する。
#[derive(Serialize, Clone)]
struct SyncProgressEvent {
    account_id: i64,
    fetched: u32,
    total: u32,
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
    is_archived: bool,
    labels: Vec<LabelView>,
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
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let (message, raw) = load_message_and_raw(&conn, message_id)?;
    let labels = db::labels::for_message(&conn, message_id).map_err(|e| e.to_string())?;

    Ok(MessageDetailView {
        id: message.id,
        subject: message.subject,
        from_name: message.from_name,
        from_addr: message.from_addr,
        to_addr: message.to_addr,
        date_header: message.date_header,
        is_read: message.is_read,
        is_flagged: message.is_flagged,
        is_archived: message.is_archived,
        labels: labels.into_iter().map(LabelView::from).collect(),
        body: extract_plain_text_body(&raw),
        attachments: attachments::list_attachments(&raw)
            .into_iter()
            .map(AttachmentView::from)
            .collect(),
    })
}

/// 検索インデックスの初回再構築（`spawn_initial_search_catchup`）やバックグラウンド
/// 同期は`write_conn`を長時間（Tantivyのcommitリトライ込みで最大約8.4秒/バッチ）
/// 保持しうる。メッセージ状態を変える系のコマンドも同じ`write_conn`を取り合うため、
/// 素の(非async)`#[tauri::command]`のままだとIPCディスパッチスレッドがロック待ちで
/// 塞がり、アプリ全体が「応答なし」になる（`sync_account`と同じ理由で実際に踏んだ。
/// 大量の未投入分を一気に片付ける初回キャッチアップ中に特に起きやすい）。
/// そのため`sync_account`/`reindex_all`と同様に`spawn_blocking`へ逃がす。
#[tauri::command]
async fn set_read(app: AppHandle, message_id: i64, is_read: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        db::messages::set_read(&conn, message_id, is_read).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_flagged(app: AppHandle, message_id: i64, is_flagged: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        db::messages::set_flagged(&conn, message_id, is_flagged).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 論理削除する/復元する（POP3サーバー側のDELEとは独立したローカルの削除フラグ）。
/// 削除時は検索インデックスからも取り除く（`ferro_core::message_actions::set_deleted`参照）。
#[tauri::command]
async fn set_deleted(app: AppHandle, message_id: i64, is_deleted: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        ferro_core::message_actions::set_deleted(&conn, &state.search_index, message_id, is_deleted)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_archived(app: AppHandle, message_id: i64, is_archived: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        db::messages::set_archived(&conn, message_id, is_archived).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `preset`は"1d"/"3d"/"1w"のいずれか。Noneを渡すとスヌーズ解除（受信箱へ即座に戻す）。
/// 期限の時刻計算はここ(Rust側)で行い、フロントはUnix時刻を扱わない。
#[tauri::command]
async fn set_snoozed(app: AppHandle, message_id: i64, preset: Option<String>) -> Result<(), String> {
    let until = match preset.as_deref() {
        None => None,
        Some("1d") => Some(now_unix() + 24 * 60 * 60),
        Some("3d") => Some(now_unix() + 3 * 24 * 60 * 60),
        Some("1w") => Some(now_unix() + 7 * 24 * 60 * 60),
        Some(other) => return Err(format!("unknown snooze preset: {other}")),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        db::messages::set_snoozed(&conn, message_id, until).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let (_message, raw) = load_message_and_raw(&conn, message_id)?;
    let bytes = attachments::extract_attachment_bytes(&raw, attachment_index)
        .ok_or_else(|| format!("attachment #{attachment_index} not found"))?;
    std::fs::write(&destination_path, bytes).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_accounts(state: State<AppState>) -> Result<Vec<AccountView>, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    db::accounts::list(&conn)
        .map(|accounts| accounts.into_iter().map(AccountView::from).collect())
        .map_err(|e| e.to_string())
}

fn parse_folder(folder: &str) -> Result<Folder, String> {
    match folder {
        "inbox" => Ok(Folder::Inbox),
        "starred" => Ok(Folder::Starred),
        "snoozed" => Ok(Folder::Snoozed),
        "archive" => Ok(Folder::Archive),
        "trash" => Ok(Folder::Trash),
        other => Err(format!("unknown folder: {other}")),
    }
}

/// フォルダ別（受信箱/スター付き/スヌーズ/アーカイブ/ゴミ箱）の新着順
/// キーセットページネーション一覧。`account_id`を省略すると全アカウント横断、
/// `before`で次ページを取得する。
#[tauri::command]
fn list_messages_by_folder(
    state: State<AppState>,
    folder: String,
    account_id: Option<i64>,
    before: Option<i64>,
    limit: u32,
) -> Result<Vec<MessageView>, String> {
    let folder = parse_folder(&folder)?;
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let messages = db::messages::list_by_folder(&conn, folder, account_id, before, limit, now_unix())
        .map_err(|e| e.to_string())?;
    attach_labels(&conn, messages)
}

/// 指定ラベルが付いたメッセージの新着順キーセットページネーション一覧。
#[tauri::command]
fn list_messages_by_label(
    state: State<AppState>,
    label_id: i64,
    before: Option<i64>,
    limit: u32,
) -> Result<Vec<MessageView>, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let messages =
        db::messages::list_by_label(&conn, label_id, before, limit).map_err(|e| e.to_string())?;
    attach_labels(&conn, messages)
}

#[derive(Serialize)]
struct FolderCountsView {
    inbox: i64,
    starred: i64,
    snoozed: i64,
    archive: i64,
    trash: i64,
}

/// サイドバーのフォルダ件数表示用。
#[tauri::command]
fn get_folder_counts(
    state: State<AppState>,
    account_id: Option<i64>,
) -> Result<FolderCountsView, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    db::messages::folder_counts(&conn, account_id, now_unix())
        .map(|c| FolderCountsView {
            inbox: c.inbox,
            starred: c.starred,
            snoozed: c.snoozed,
            archive: c.archive,
            trash: c.trash,
        })
        .map_err(|e| e.to_string())
}

/// サイドバー表示用のラベル一覧（付与件数付き）。
#[tauri::command]
fn list_labels(state: State<AppState>) -> Result<Vec<LabelWithCountView>, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    db::labels::list_with_counts(&conn)
        .map(|labels| {
            labels
                .into_iter()
                .map(|lc| LabelWithCountView {
                    id: lc.label.id,
                    name: lc.label.name,
                    color: lc.label.color,
                    count: lc.count,
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn create_label(app: AppHandle, name: String, color: String) -> Result<LabelView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        let id = db::labels::insert(&conn, &name, &color).map_err(|e| e.to_string())?;
        Ok(LabelView { id, name, color })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// ラベル自体と、全メッセージへの付与関係も合わせて削除する
/// (`db::labels::delete`参照。メッセージ本体・Maildir・検索インデックスは触れない)。
#[tauri::command]
async fn delete_label(app: AppHandle, label_id: i64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        db::labels::delete(&conn, label_id).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn set_message_label(
    app: AppHandle,
    message_id: i64,
    label_id: i64,
    assigned: bool,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        db::labels::set_on_message(&conn, message_id, label_id, assigned).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Deserialize)]
struct SettingsView {
    show_preview_line: bool,
    show_sender_avatar: bool,
    mark_read_delay: bool,
    sync_interval_minutes: i64,
}

impl From<Settings> for SettingsView {
    fn from(s: Settings) -> Self {
        SettingsView {
            show_preview_line: s.show_preview_line,
            show_sender_avatar: s.show_sender_avatar,
            mark_read_delay: s.mark_read_delay,
            sync_interval_minutes: s.sync_interval_minutes,
        }
    }
}

impl From<SettingsView> for Settings {
    fn from(s: SettingsView) -> Self {
        Settings {
            show_preview_line: s.show_preview_line,
            show_sender_avatar: s.show_sender_avatar,
            mark_read_delay: s.mark_read_delay,
            sync_interval_minutes: s.sync_interval_minutes,
        }
    }
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Result<SettingsView, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    db::settings::get(&conn).map(SettingsView::from).map_err(|e| e.to_string())
}

#[tauri::command]
async fn update_settings(app: AppHandle, settings: SettingsView) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        db::settings::set(&conn, &settings.into()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// アカウントを追加する: `accounts.toml`にエントリを追記してからDBに反映し、
/// パスワードをOS keyringに保存する（`account_setup::add`参照）。
#[tauri::command]
async fn add_account(
    app: AppHandle,
    name: String,
    host: String,
    port: u16,
    username: String,
    use_tls: bool,
    password: String,
) -> Result<AccountView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
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
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn remove_account(app: AppHandle, account_id: i64) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        account_setup::remove(
            &conn,
            &paths::maildir_dir(),
            &state.search_index,
            &paths::accounts_config_path(),
            account_id,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `accounts.toml`を読み直してDBに反映する。アプリ起動中に手でファイルを
/// 編集した場合に使う（他は起動時に一度だけ自動で反映する）。
#[tauri::command]
async fn reload_accounts_config(app: AppHandle) -> Result<Vec<AccountView>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        account_config::load_and_reconcile(&conn, &paths::accounts_config_path())
            .map(|accounts| accounts.into_iter().map(AccountView::from).collect())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// フロント側で「設定ファイルはここにあります」と案内表示するためのパス。
#[tauri::command]
fn accounts_config_path() -> String {
    paths::accounts_config_path().display().to_string()
}

/// 色分けルール(`color_rules.toml`)を読み込む。実際のマッチ判定はフロント側
/// （`colorRules.js`）が行うので、ここは読み込んだ内容をそのまま返すだけ。
/// GUI起動時の初回読み込みと、手編集後の「再読み込み」ボタンの両方で使う。
#[tauri::command]
fn get_color_rules() -> Result<Vec<ColorRule>, String> {
    color_rules::load(&paths::color_rules_config_path())
        .map(|file| file.rules)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn add_color_rule(pattern: String, color: String) -> Result<Vec<ColorRule>, String> {
    color_rules::add(&paths::color_rules_config_path(), ColorRule { pattern, color })
        .map(|file| file.rules)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_color_rule(index: usize) -> Result<Vec<ColorRule>, String> {
    color_rules::remove(&paths::color_rules_config_path(), index)
        .map(|file| file.rules)
        .map_err(|e| e.to_string())
}

/// フロント側で「設定ファイルはここにあります」と案内表示するためのパス
/// （`accounts_config_path`と同じ理由）。
#[tauri::command]
fn color_rules_config_path() -> String {
    paths::color_rules_config_path().display().to_string()
}

/// アカウントを同期する。ネットワークI/Oを伴い数十秒〜数分（クラッシュ時の
/// リトライを含めるとさらに長く）かかることがある。`#[tauri::command]`の
/// 呼び出し元スレッドで直接この重い処理を行うと、実機でWindowsから
/// プロセスが「応答なし」判定される（`Get-Process .Responding`がfalseになる）
/// ことを実際に確認した。`spawn_background_sync`（5分毎の自動同期）は元々
/// 生の`std::thread::spawn`でTauriのコマンド呼び出しを経由せず動いており
/// この問題を踏まないため、手動SyncもTauriのIPC実行コンテキストから完全に
/// 切り離す（`async fn`にして`spawn_blocking`で別スレッドに逃がし、
/// `State`もそのスレッドの中で`app.state()`から取り直す）。
#[tauri::command]
async fn sync_account(
    app: AppHandle,
    account_id: i64,
    limit: Option<u32>,
    allow_plaintext: bool,
) -> Result<SyncSummaryView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
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
            |fetched, total| {
                let _ = app.emit("sync-progress", SyncProgressEvent { account_id, fetched, total });
            },
        )
        .map(SyncSummaryView::from)
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 全文検索インデックスを検索し、ヒットしたメッセージをスコア順に返す。
#[tauri::command]
fn search_messages(state: State<AppState>, query: String, limit: usize) -> Result<Vec<MessageView>, String> {
    let ids = state.search_index.search(&query, limit).map_err(|e| e.to_string())?;
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let mut messages = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(m) = db::messages::get(&conn, id).map_err(|e| e.to_string())? {
            messages.push(m);
        }
    }
    attach_labels(&conn, messages)
}

/// 全メッセージから検索インデックスを作り直す（DB/Maildirから再構築可能な派生キャッシュ）。
/// `sync_account`と同じ理由（大量メッセージ＋クラッシュ時リトライで長時間化しうる）で
/// `spawn_blocking`に逃がし、呼び出し元スレッドをブロックしないようにする。
#[tauri::command]
async fn reindex_all(app: AppHandle) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        // 全メッセージをDBから読むだけ(SQLite側は変更しない)なのでread_connでよい。
        let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
        reindex::reindex_all(&conn, &paths::maildir_dir(), &state.search_index).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
struct BackgroundSyncEvent {
    account_id: i64,
    #[serde(flatten)]
    summary: Option<SyncSummaryView>,
    error: Option<String>,
}

/// 設定画面の「Sync間隔」を読む。ロック取得や読み取りに失敗した場合・
/// 0以下が保存されていた場合はデフォルト(5分)にフォールバックする
/// （バックグラウンドスレッドを止めないため、ここではエラーを伝播しない）。
fn current_sync_interval(app_handle: &AppHandle) -> Duration {
    let state = app_handle.state::<AppState>();
    let minutes = state
        .read_conn
        .lock()
        .ok()
        .and_then(|conn| db::settings::get(&conn).ok())
        .map(|s| s.sync_interval_minutes)
        .filter(|m| *m > 0)
        .unwrap_or(5);
    Duration::from_secs((minutes as u64) * 60)
}

/// バックグラウンド定期同期のループ本体。専用のOSスレッドで動かす
/// （ferro-coreの同期処理は同期(ブロッキング)関数なので、tokioワーカースレッドを
/// 塞がないよう素のstd::threadを使う）。
///
/// `allow_plaintext`は`!account.use_tls`で決める: 平文専用アカウント自体が
/// アカウント作成時にユーザーが明示的に選んだ設定（CLAUDE.md参照）なので、
/// それを自動同期でも尊重する（以前は自動実行では常に`allow_plaintext=false`にして
/// 手動Syncのみに絞っていたが、ユーザーの要望により平文アカウントも自動同期の対象にした）。
///
/// 間隔は`current_sync_interval`で毎サイクル読み直す。設定変更は次のサイクルから
/// 反映される（実行中のsleepを割り込んで即座反映するような仕組みは持たない。
/// 「Sync間隔」はそこまで即時性が求められる設定ではないため、単純さを優先した）。
fn spawn_background_sync(app_handle: AppHandle) {
    std::thread::spawn(move || {
        // 検索インデックス未投入分を拾い直すためのカーソル（このスレッドの
        // 生存期間だけ持てば十分。プロセス再起動後は0から再開すればよい）。
        // `step_search_catchup`のドキュメント参照。
        let mut catchup_after_id = 0i64;
        loop {
            std::thread::sleep(current_sync_interval(&app_handle));
            run_background_sync_once(&app_handle);
            step_search_catchup(&app_handle, &mut catchup_after_id);
        }
    });
}

/// `catch_up_unindexed`を1回だけ呼び、カーソルを進める。
///
/// 戻り値の`next_after_id`が`Some`ならそこへ、`None`（未投入分の末尾まで
/// 到達）なら次回0から周回し直すようカーソルを更新する。呼び出し・DBロック
/// 失敗時はカーソルを据え置き、次回同じ位置からやり直す。
fn step_search_catchup(app_handle: &AppHandle, after_id: &mut i64) -> usize {
    let state = app_handle.state::<AppState>();
    let Ok(conn) = state.write_conn.lock() else {
        return 0;
    };
    match reindex::catch_up_unindexed(&conn, &paths::maildir_dir(), &state.search_index, *after_id)
    {
        Ok((indexed, Some(next))) => {
            *after_id = next;
            indexed
        }
        Ok((indexed, None)) => {
            *after_id = 0;
            indexed
        }
        Err(_) => 0,
    }
}

/// 起動直後に一度だけ、検索インデックス未投入(`fts_indexed_at IS NULL`)の
/// メッセージをまとめて片付ける専用スレッド。
///
/// `fts_indexed_at`による投入管理はこのバージョンで初めて実装したため、
/// それ以前に同期済みだった大量の既存メッセージは（Tantivyへの投入自体は
/// 当時成功していたものが大半のはずだが、記録が無いという理由だけで）
/// 全て「未投入」扱いになる。`run_background_sync_once`内の定期呼び出しは
/// 1サイクルあたり500件ずつしか処理しないため、数万件規模の既存データだと
/// 追いつくまでに何時間もかかってしまう（実際に検索で見つかるはずのメッセージが
/// 見つからない、という形で踏んだ）。起動時にこのスレッドで一気に片付けておく。
///
/// `after_id`カーソルで先頭から末尾まで1周し、1周の間に1件も投入できなければ
/// （＝残っている未投入メッセージ全てが今は投入不能、または既に無い）そこで
/// 終了する。特定のバッチが繰り返し失敗しても`after_id`が前進し続けるため、
/// 以前のように「同じ集合に永久に足止めされて後続に一生手が届かない」ことはない
/// （`db::messages::list_unindexed`のドキュメント参照）。
fn spawn_initial_search_catchup(app_handle: AppHandle) {
    std::thread::spawn(move || {
        // 安全弁。周回してもなお進展が無くなり次第即座に終了するため、
        // これは「間違って無限ループしない」ための上限に過ぎない。
        const MAX_ITERATIONS: u32 = 5000;
        let mut after_id = 0i64;
        let mut indexed_this_lap = 0usize;
        for _ in 0..MAX_ITERATIONS {
            let state = app_handle.state::<AppState>();
            let result = {
                let Ok(conn) = state.write_conn.lock() else {
                    return;
                };
                reindex::catch_up_unindexed(&conn, &paths::maildir_dir(), &state.search_index, after_id)
            };
            match result {
                Ok((indexed, Some(next))) => {
                    indexed_this_lap += indexed;
                    after_id = next;
                    std::thread::sleep(Duration::from_millis(200));
                }
                Ok((indexed, None)) => {
                    indexed_this_lap += indexed;
                    if indexed_this_lap == 0 {
                        return; // 1周して何も投入できなかった＝完全に追いついた
                    }
                    // まだ何か投入できたなら、取りこぼしが無いか最初からもう一周確認する
                    // （最初の周で失敗したバッチが、後続処理を挟んだ2周目には
                    // 成功することがある。CLAUDE.md記載のクラッシュは間欠的なため）。
                    after_id = 0;
                    indexed_this_lap = 0;
                    std::thread::sleep(Duration::from_millis(200));
                }
                // Tantivy側のcrashは`catch_up_unindexed`内で1件ずつのフォールバックまで
                // 吸収済みなので、ここに来るのはSQLite側の失敗(ロック競合等)のみ。
                // after_idは据え置いたまま少し待って次回同じ位置からやり直す。
                Err(_) => std::thread::sleep(Duration::from_secs(2)),
            }
        }
    });
}

fn run_background_sync_once(app_handle: &AppHandle) {
    let state = app_handle.state::<AppState>();

    let accounts = {
        let Ok(conn) = state.read_conn.lock() else {
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
                let Ok(conn) = state.write_conn.lock() else {
                    return;
                };
                let account_id = account.id;
                let result = sync::sync_account_with_limit(
                    &conn,
                    &paths::maildir_dir(),
                    &account,
                    &password,
                    !account.use_tls,
                    Some(BACKGROUND_SYNC_LIMIT),
                    &state.search_index,
                    |fetched, total| {
                        let _ = app_handle.emit(
                            "sync-progress",
                            SyncProgressEvent { account_id, fetched, total },
                        );
                    },
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
    // sync中にTantivyのIndexWriterクラッシュ(`with_commit_retry`のドキュメント参照)で
    // 検索インデックスへの投入が漏れたメッセージの拾い直しは、呼び出し元
    // (`spawn_background_sync`)がサイクルのたびに`step_search_catchup`で行う。
}

/// SettingsとMessages一覧はOSネイティブのメニューバーから切り替える別画面として
/// 扱う（同じウィンドウ内でフロント側の表示を切り替えるだけで、別ウィンドウは
/// 開かない）。Account管理はSettings画面内の「アカウント」セクションに統合されている
/// （`SettingsView.svelte`参照）ので、専用のメニュー項目は無い。メニュー項目の
/// クリックは`navigate`イベントとしてフロントへ通知し、`App.svelte`側で画面を切り替える。
const NAV_MESSAGES_ID: &str = "nav-messages";
const NAV_SETTINGS_ID: &str = "nav-settings";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    std::fs::create_dir_all(paths::app_data_dir()).expect("failed to create app data directory");
    // 書き込み用・読み取り用で別々の接続を開く(`AppState`のドキュメント参照)。
    // 同じファイルを指す独立した`Connection`で、WALモードなので競合しない。
    let write_conn = db::open(&paths::db_path()).expect("failed to open ferro database");
    let read_conn =
        db::open(&paths::db_path()).expect("failed to open ferro database (read connection)");
    let search_index =
        SearchIndex::open_or_create(&paths::search_index_dir()).expect("failed to open search index");

    // 検索インデックスの文書数が、DBが「投入済み」と認識している件数より少ない場合、
    // 自己修復不能な不整合（`db::messages::reset_all_fts_indexed`のドキュメント参照。
    // スキーマ変更での自動再構築や`search_index`ディレクトリの手動削除の直後に起きうる。
    // 実際にこれで検索が壊れたまま治らなくなる不具合を踏んだ）なので、全件を
    // 再投入対象に戻す。単純に`num_docs() == 0`だけを見ると、インデックスが
    // 消えた後に新着メールだけは正常に投入され続けて`num_docs()`が0でなくなり、
    // 大半を占める既存メールの欠落を二度と検知できなくなる（実際に踏んだ）ため、
    // DB側の件数との比較にしている。
    match db::messages::count_indexed(&write_conn) {
        Ok(indexed_count) if indexed_count > search_index.num_docs() as i64 => {
            match db::messages::reset_all_fts_indexed(&write_conn) {
                Ok(0) => {}
                Ok(n) => eprintln!(
                    "warning: search index has fewer documents ({}) than the DB expected \
                     ({indexed_count}); resetting {n} message(s) so the background catch-up \
                     rebuilds the index",
                    search_index.num_docs()
                ),
                Err(e) => eprintln!("warning: failed to reset fts_indexed_at: {e}"),
            }
        }
        Ok(_) => {}
        Err(e) => eprintln!("warning: failed to check indexed message count: {e}"),
    }

    // accounts.tomlが正の情報源。存在すれば起動時にDBへ反映する（無ければ空扱い）。
    // 手編集ファイルの構文ミス等でここが失敗してもGUI自体は起動させる
    // （直近の反映結果＝DBの内容のまま起動し、修正後は`reload_accounts_config`
    // か再起動で反映される）。
    if let Err(e) = account_config::load_and_reconcile(&write_conn, &paths::accounts_config_path()) {
        eprintln!("warning: failed to load/reconcile accounts.toml: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            write_conn: Mutex::new(write_conn),
            read_conn: Mutex::new(read_conn),
            search_index,
        })
        .setup(|app| {
            spawn_background_sync(app.handle().clone());
            spawn_initial_search_catchup(app.handle().clone());

            let messages_item = MenuItemBuilder::with_id(NAV_MESSAGES_ID, "Messages").build(app)?;
            let settings_item = MenuItemBuilder::with_id(NAV_SETTINGS_ID, "Settings…").build(app)?;
            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&messages_item)
                .item(&settings_item)
                .build()?;
            let menu = MenuBuilder::new(app).item(&view_menu).build()?;
            app.set_menu(menu)?;

            Ok(())
        })
        .on_menu_event(|app_handle, event| {
            let target = match event.id().as_ref() {
                NAV_MESSAGES_ID => Some("messages"),
                NAV_SETTINGS_ID => Some("settings"),
                _ => None,
            };
            if let Some(target) = target {
                let _ = app_handle.emit("navigate", target);
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            list_messages_by_folder,
            list_messages_by_label,
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
            set_archived,
            set_snoozed,
            get_folder_counts,
            list_labels,
            create_label,
            delete_label,
            set_message_label,
            get_settings,
            update_settings,
            reload_accounts_config,
            accounts_config_path,
            get_color_rules,
            add_color_rule,
            remove_color_rule,
            color_rules_config_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Ferro desktop app");
}
