mod badge;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ferro_core::account_config::{self, AccountConfig};
use ferro_core::account_setup;
use ferro_core::color_rules::{self, ColorRule};
use ferro_core::credentials;
use ferro_core::logging;
use ferro_core::db::accounts::Account;
use ferro_core::db::labels::Label;
use ferro_core::db::messages::{Folder, Message};
use ferro_core::db::sync_status::SyncStatus;
use ferro_core::db::{self, Connection};
use ferro_core::label_rules::{self, LabelRule, RuleActions};
use ferro_core::mail::attachments::{self, AttachmentInfo};
use ferro_core::mail::{export, source};
use ferro_core::mail::parse::extract_plain_text_body;
use ferro_core::message_filter::MessageFilter;
use ferro_core::search::SearchIndex;
use ferro_core::settings::Settings;
use ferro_core::sync::SyncSummary;
use ferro_core::saved_searches::{self, SavedSearch};
use ferro_core::{maildir, paths, reindex, settings, sync};
use serde::{Deserialize, Serialize};
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::image::Image;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_notification::NotificationExt;

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
/// save_attachment/search_messages）は`read_conn`を使う。
///
/// `read_conn`を使う読み取り系コマンドは`#[tauri::command(async)]`にしてある。
/// Tauriは`async`でない普通のコマンドを**メインスレッド**で実行するため、
/// ロック待ちが起きるとその間ウィンドウのイベントループごと止まり、アプリ全体が
/// 「応答なし」になる（検索インデックス再構築が`read_conn`を数分間握りっぱなしに
/// していた間、一覧取得等がメインスレッドで待たされて実際に固まった）。
/// 長時間かかる処理は`read_conn`/`write_conn`を長く握らないこと（`reindex_all`参照）。
struct AppState {
    write_conn: Mutex<Connection>,
    read_conn: Mutex<Connection>,
    search_index: SearchIndex,
    /// 実行中（`write_conn`のロック待ちを含む）の手動Syncの件数。0より大きい間、
    /// バックグラウンド同期は新しいアカウントの同期を始めずに見送る
    /// （`ManualSyncGuard`/`run_background_sync_once`参照）。
    manual_syncs: AtomicUsize,
}

/// 手動Syncの実行中だけ`AppState::manual_syncs`を1つ増やすRAIIガード。
/// 手動Syncは無制限(`limit: None`)で全件取るため、同じ`write_conn`ロックを
/// 待つバックグラウンド同期（上限`BACKGROUND_SYNC_LIMIT`）が直後に走ると、
/// 手動Syncの結果表示が「fetched 0」等の自動同期の結果で上書きされてしまう。
struct ManualSyncGuard<'a>(&'a AtomicUsize);

impl<'a> ManualSyncGuard<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for ManualSyncGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Serialize)]
struct AccountView {
    id: i64,
    name: String,
    host: String,
    port: u16,
    username: String,
    use_tls: bool,
    /// UIDLに対応しているか（`None`はまだ判定していない）。
    uidl_supported: Option<bool>,
    /// 直近の同期結果（`db::sync_status`。まだ同期していなければ`None`）。
    sync_status: Option<SyncStatusView>,
}

#[derive(Serialize)]
struct SyncStatusView {
    last_attempt_at: i64,
    last_success_at: Option<i64>,
    last_error: Option<String>,
    last_fetched: u32,
    last_remaining: u32,
    last_ended_early: bool,
    consecutive_failures: u32,
}

impl From<SyncStatus> for SyncStatusView {
    fn from(s: SyncStatus) -> Self {
        SyncStatusView {
            last_attempt_at: s.last_attempt_at,
            last_success_at: s.last_success_at,
            last_error: s.last_error,
            last_fetched: s.last_fetched,
            last_remaining: s.last_remaining,
            last_ended_early: s.last_ended_early,
            consecutive_failures: s.consecutive_failures,
        }
    }
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
            uidl_supported: a.uidl_supported,
            sync_status: None,
        }
    }
}

/// アカウント一覧に直近の同期結果を付けて返す（`list_accounts`/`reload_accounts_config`）。
fn account_views(conn: &Connection, accounts: Vec<Account>) -> Result<Vec<AccountView>, String> {
    let mut statuses = db::sync_status::list(conn).map_err(|e| e.to_string())?;
    Ok(accounts
        .into_iter()
        .map(|account| {
            let status = statuses.remove(&account.id);
            let mut view = AccountView::from(account);
            view.sync_status = status.map(SyncStatusView::from);
            view
        })
        .collect())
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
    is_deleted: bool,
    is_archived: bool,
    snoozed_until: Option<i64>,
    attachment_count: i64,
    preview: Option<String>,
    labels: Vec<LabelView>,
    /// このメッセージが属するスレッドの件数（削除済みを除く。スレッド未判定なら1）。
    thread_size: i64,
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
            is_deleted: m.is_deleted,
            is_archived: m.is_archived,
            snoozed_until: m.snoozed_until,
            attachment_count: m.attachment_count,
            preview: m.preview,
            // ラベルは一覧側でまとめて取得して詰める(`attach_labels`参照。
            // メッセージ1件ごとにラベルを引くとN+1になるため)。スレッドの件数も同じ。
            labels: Vec::new(),
            thread_size: 1,
        }
    }
}

/// `messages`をまとめて`MessageView`に変換し、ラベルとスレッドの件数をそれぞれ
/// 1回のクエリで詰める(`db::labels::for_messages`/`db::messages::thread_info`で
/// N+1を避ける)。一覧・検索・ラベル別一覧の各コマンドで共通に使う。
fn attach_labels(conn: &Connection, messages: Vec<Message>) -> Result<Vec<MessageView>, String> {
    let ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
    let mut label_map: HashMap<i64, Vec<Label>> =
        db::labels::for_messages(conn, &ids).map_err(|e| e.to_string())?;
    let thread_map = db::messages::thread_info(conn, &ids).map_err(|e| e.to_string())?;

    Ok(messages
        .into_iter()
        .map(|m| {
            let labels = label_map.remove(&m.id).unwrap_or_default();
            let thread_size = thread_map.get(&m.id).map_or(1, |&(_, size)| size);
            let mut view = MessageView::from(m);
            view.labels = labels.into_iter().map(LabelView::from).collect();
            view.thread_size = thread_size;
            view
        })
        .collect())
}

/// 本文表示の「このスレッドのメッセージ」欄の1行。
#[derive(Serialize)]
struct ThreadItemView {
    id: i64,
    subject: Option<String>,
    from_name: Option<String>,
    from_addr: Option<String>,
    date_header: i64,
    is_read: bool,
    preview: Option<String>,
}

/// 本文表示で一度に出すスレッドのメッセージ数の上限。
const THREAD_DISPLAY_LIMIT: u32 = 200;

/// Sync進捗（"30/1000"のような表示用）。1バッチ完了するごとに発火する。
/// 検索インデックス再構築の進捗（`reindex::ReindexProgress`）。開始直後と、
/// 1000件のページを処理するたびに発火する。
#[derive(Serialize, Clone)]
struct ReindexProgressEvent {
    processed: usize,
    total: usize,
}

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
    /// 同じスレッドのメッセージ（自分を含み古い順）。スレッドが自分だけなら空。
    thread: Vec<ThreadItemView>,
    /// 「.emlとして保存」の保存ダイアログで既定にするファイル名。
    eml_filename: String,
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
#[tauri::command(async)]
fn get_message_detail(state: State<AppState>, message_id: i64) -> Result<MessageDetailView, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let (message, raw) = load_message_and_raw(&conn, message_id)?;
    let labels = db::labels::for_message(&conn, message_id).map_err(|e| e.to_string())?;
    let eml_filename = export::suggested_eml_filename(message.subject.as_deref(), message.id);
    let thread = match db::messages::thread_info(&conn, &[message_id]).map_err(|e| e.to_string())?.get(&message_id) {
        Some(&(thread_id, size)) if size > 1 => db::messages::list_thread(&conn, thread_id, THREAD_DISPLAY_LIMIT)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|m| ThreadItemView {
                id: m.id,
                subject: m.subject,
                from_name: m.from_name,
                from_addr: m.from_addr,
                date_header: m.date_header,
                is_read: m.is_read,
                preview: m.preview,
            })
            .collect(),
        _ => Vec::new(),
    };

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
        eml_filename,
        thread,
    })
}

#[derive(Serialize)]
struct SourceView {
    text: String,
    truncated: bool,
    total_bytes: usize,
}

/// 本文表示の「ヘッダー」「ソース」タブ用に、生のヘッダー部（`headers_only`）か
/// メール全体（大きければ先頭`source::SOURCE_DISPLAY_LIMIT`バイトまで）を返す。
#[tauri::command(async)]
fn get_message_source(state: State<AppState>, message_id: i64, headers_only: bool) -> Result<SourceView, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let (_message, raw) = load_message_and_raw(&conn, message_id)?;
    drop(conn);
    let source = source::source_text(&raw, headers_only);
    Ok(SourceView { text: source.text, truncated: source.truncated, total_bytes: source.total_bytes })
}

/// メッセージを`.eml`（Maildirに保存している生のバイト列そのまま）として、
/// フロント側の保存ダイアログで選んだ`destination_path`に書き出す。
#[tauri::command(async)]
fn save_message_eml(state: State<AppState>, message_id: i64, destination_path: String) -> Result<(), String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let (_message, raw) = load_message_and_raw(&conn, message_id)?;
    drop(conn);
    std::fs::write(&destination_path, raw).map_err(|e| e.to_string())
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

/// スヌーズのプリセット（"1d"/"3d"/"1w"）から期限のUnix時刻を求める。Noneは解除。
/// 期限の時刻計算はここ(Rust側)で行い、フロントはUnix時刻を扱わない。
fn snooze_until(preset: Option<&str>) -> Result<Option<i64>, String> {
    match preset {
        None => Ok(None),
        Some("1d") => Ok(Some(now_unix() + 24 * 60 * 60)),
        Some("3d") => Ok(Some(now_unix() + 3 * 24 * 60 * 60)),
        Some("1w") => Ok(Some(now_unix() + 7 * 24 * 60 * 60)),
        Some(other) => Err(format!("unknown snooze preset: {other}")),
    }
}

/// `preset`は"1d"/"3d"/"1w"のいずれか。Noneを渡すとスヌーズ解除（受信箱へ即座に戻す）。
#[tauri::command]
async fn set_snoozed(app: AppHandle, message_id: i64, preset: Option<String>) -> Result<(), String> {
    let until = snooze_until(preset.as_deref())?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        db::messages::set_snoozed(&conn, message_id, until).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 一括操作の種類（フロントからは`{ kind: "read", value: true }`のような形で渡す）。
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BulkActionInput {
    Read { value: bool },
    Flag { value: bool },
    Archive { value: bool },
    Snooze { preset: Option<String> },
    Delete,
    Label { label_id: i64, assigned: bool },
}

/// 複数選択したメッセージへの一括操作（`message_actions::apply_bulk`参照）。
#[tauri::command]
async fn bulk_update(app: AppHandle, message_ids: Vec<i64>, action: BulkActionInput) -> Result<(), String> {
    use ferro_core::message_actions::{apply_bulk, BulkAction};
    let action = match action {
        BulkActionInput::Read { value } => BulkAction::SetRead(value),
        BulkActionInput::Flag { value } => BulkAction::SetFlagged(value),
        BulkActionInput::Archive { value } => BulkAction::SetArchived(value),
        BulkActionInput::Snooze { preset } => BulkAction::Snooze(snooze_until(preset.as_deref())?),
        BulkActionInput::Delete => BulkAction::Delete,
        BulkActionInput::Label { label_id, assigned } => BulkAction::SetLabel { label_id, assigned },
    };
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        apply_bulk(&conn, &state.search_index, &message_ids, action).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 表示中のフォルダ（`folder`）かラベル（`label_id`。こちらが優先）の未読をすべて既読にする
/// （「すべて既読にする」）。既読にした件数を返す。検索結果のときはフロントが`bulk_update`を使う。
#[tauri::command]
async fn mark_all_read(app: AppHandle, folder: Option<String>, label_id: Option<i64>) -> Result<usize, String> {
    let folder = folder.as_deref().map(parse_folder).transpose()?;
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        match (label_id, folder) {
            (Some(label_id), _) => db::messages::mark_label_read(&conn, label_id),
            (None, Some(folder)) => db::messages::mark_folder_read(&conn, folder, now_unix()),
            (None, None) => return Err("folder or label_id is required".to_string()),
        }
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `get_message_detail`が返した添付の`index`を指定して、フロント側が
/// (`@tauri-apps/plugin-dialog`の保存ダイアログで選んだ)`destination_path`に書き出す。
#[tauri::command(async)]
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

#[tauri::command(async)]
fn list_accounts(state: State<AppState>) -> Result<Vec<AccountView>, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let accounts = db::accounts::list(&conn).map_err(|e| e.to_string())?;
    account_views(&conn, accounts)
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
    theme: String,
    accent_color: String,
    font_family: String,
    font_size: String,
    retention_days: i64,
    server_delete_days: i64,
    auto_update_check: bool,
    notify_new_mail: bool,
    notify_labels: Vec<String>,
    close_to_tray: bool,
    show_unread_badge: bool,
    open_on_select: bool,
}

impl From<Settings> for SettingsView {
    fn from(s: Settings) -> Self {
        SettingsView {
            show_preview_line: s.show_preview_line,
            show_sender_avatar: s.show_sender_avatar,
            mark_read_delay: s.mark_read_delay,
            sync_interval_minutes: s.sync_interval_minutes,
            theme: s.theme,
            accent_color: s.accent_color,
            font_family: s.font_family,
            font_size: s.font_size,
            retention_days: s.retention_days,
            server_delete_days: s.server_delete_days,
            auto_update_check: s.auto_update_check,
            notify_new_mail: s.notify_new_mail,
            notify_labels: s.notify_labels,
            close_to_tray: s.close_to_tray,
            show_unread_badge: s.show_unread_badge,
            open_on_select: s.open_on_select,
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
            theme: s.theme,
            accent_color: s.accent_color,
            font_family: s.font_family,
            font_size: s.font_size,
            retention_days: s.retention_days,
            server_delete_days: s.server_delete_days,
            auto_update_check: s.auto_update_check,
            notify_new_mail: s.notify_new_mail,
            notify_labels: s.notify_labels,
            close_to_tray: s.close_to_tray,
            show_unread_badge: s.show_unread_badge,
            open_on_select: s.open_on_select,
        }
    }
}

#[tauri::command]
fn get_settings() -> Result<SettingsView, String> {
    settings::load(&paths::settings_config_path())
        .map(SettingsView::from)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn update_settings(app: AppHandle, settings: SettingsView) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        ferro_core::settings::save(&paths::settings_config_path(), &settings.into()).map_err(|e| e.to_string())?;
        // 「未読数を表示」の切り替えをすぐ反映する。
        update_unread_badge(&app);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 受信箱の未読数を、トレイアイコン（右下のバッジとツールチップ）とタスクバー（Windowsは
/// オーバーレイアイコン、それ以外はバッジ）に出す。`show_unread_badge`がオフなら消す。
/// フロントが一覧の件数を取り直すたび（`refresh_unread_badge`）と、バックグラウンド同期の後に呼ぶ。
fn update_unread_badge(app_handle: &AppHandle) {
    let enabled = settings::load_or_default(&paths::settings_config_path()).show_unread_badge;
    let count = if enabled {
        let state = app_handle.state::<AppState>();
        let Ok(conn) = state.read_conn.lock() else {
            return;
        };
        db::messages::count_unread_inbox(&conn, now_unix()).unwrap_or(0).max(0) as u64
    } else {
        0
    };

    if let Some(tray) = app_handle.tray_by_id(TRAY_ID) {
        if let Some(base) = app_handle.default_window_icon() {
            let icon = if count > 0 {
                let mut rgba = base.rgba().to_vec();
                badge::draw_on(&mut rgba, base.width(), base.height(), count);
                Image::new_owned(rgba, base.width(), base.height())
            } else {
                base.clone().to_owned()
            };
            let _ = tray.set_icon(Some(icon));
        }
        let tooltip = if count > 0 { format!("Ferro — 受信箱の未読 {count}件") } else { "Ferro".to_string() };
        let _ = tray.set_tooltip(Some(tooltip));
    }
    if let Some(window) = app_handle.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        let _ = window.set_overlay_icon(
            (count > 0).then(|| Image::new_owned(badge::standalone(count, BADGE_OVERLAY_SIZE), BADGE_OVERLAY_SIZE, BADGE_OVERLAY_SIZE)),
        );
        #[cfg(not(target_os = "windows"))]
        let _ = window.set_badge_count((count > 0).then_some(count as i64));
    }
}

/// タスクバーのオーバーレイアイコンの大きさ（OSが16×16に縮めて表示する）。
#[cfg(target_os = "windows")]
const BADGE_OVERLAY_SIZE: u32 = 32;

#[tauri::command(async)]
fn refresh_unread_badge(app: AppHandle) {
    update_unread_badge(&app);
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
        let accounts =
            account_config::load_and_reconcile(&conn, &paths::accounts_config_path()).map_err(|e| e.to_string())?;
        account_views(&conn, accounts)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// フロント側で「設定ファイルはここにあります」と案内表示するためのパス。
#[tauri::command]
fn accounts_config_path() -> String {
    paths::accounts_config_path().display().to_string()
}

/// 診断用ログ（`ferro_core::logging`）のファイルパス。設定画面の
/// 「ログファイルを開く」ボタンで使う（`accounts_config_path`と同じ理由）。
#[tauri::command]
fn log_file_path() -> String {
    paths::log_path().display().to_string()
}

/// 設定画面にアプリのバージョンを表示するために使う。Tauri組み込みの
/// `app`プラグイン（`getVersion()`）を使うと権限設定が別途要るため、
/// 他の`*_path`コマンドと同じ「自前コマンドでビルド時定数を返すだけ」
/// パターンに揃えた。
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
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

/// 受信時の自動ラベル付けルール(`label_rules.toml`)を読み込む
/// （`get_color_rules`と同じ理由。実際の判定は`ferro_core::label_rules`が行う）。
#[tauri::command]
fn get_label_rules() -> Result<Vec<LabelRule>, String> {
    label_rules::load(&paths::label_rules_config_path())
        .map(|file| file.rules)
        .map_err(|e| e.to_string())
}

/// 受信ルールを追加する。`label_name`が空ならラベルは付けず、動作（既読にする等）だけを行う。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn add_label_rule(
    label_name: String,
    from_contains: Option<String>,
    subject_contains: Option<String>,
    body_contains: Option<String>,
    has_attachment: Option<bool>,
    mark_read: Option<bool>,
    archive: Option<bool>,
    star: Option<bool>,
    skip_notification: Option<bool>,
) -> Result<Vec<LabelRule>, String> {
    let filter = MessageFilter { from_contains, subject_contains, body_contains, has_attachment };
    if filter.is_empty() {
        return Err("From/件名/本文/添付有無のいずれか1つ以上を指定してください".to_string());
    }
    let actions = RuleActions {
        mark_read: mark_read.unwrap_or(false),
        archive: archive.unwrap_or(false),
        star: star.unwrap_or(false),
        skip_notification: skip_notification.unwrap_or(false),
    };
    label_rules::add(&paths::label_rules_config_path(), LabelRule { label_name, filter, actions })
        .map(|file| file.rules)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_label_rule(index: usize) -> Result<Vec<LabelRule>, String> {
    label_rules::remove(&paths::label_rules_config_path(), index)
        .map(|file| file.rules)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn label_rules_config_path() -> String {
    paths::label_rules_config_path().display().to_string()
}

/// 検索語の強調（`search::highlight`）に使う語。`field`は"any" | "from" | "subject" | "body"。
#[derive(Serialize)]
struct HighlightTermView {
    text: String,
    field: &'static str,
}

#[tauri::command]
fn highlight_terms(query: String) -> Vec<HighlightTermView> {
    use ferro_core::search::query::TextField;
    ferro_core::search::highlight::terms(&query)
        .into_iter()
        .map(|term| HighlightTermView {
            text: term.text,
            field: match term.field {
                TextField::Any => "any",
                TextField::From => "from",
                TextField::Subject => "subject",
                TextField::Body => "body",
            },
        })
        .collect()
}

#[derive(Serialize)]
struct OptimizeReportView {
    db_before: u64,
    db_after: u64,
    mail_before: u64,
    mail_after: u64,
    index_before: u64,
    index_after: u64,
    freed: u64,
}

/// DBと検索インデックスを最適化する（設定画面の「今すぐ最適化する」）。同期と同じ
/// `write_conn`を握るので、その間の同期・メールの操作は待たされる（数秒〜数十秒）。
#[tauri::command]
async fn optimize_storage(app: AppHandle) -> Result<OptimizeReportView, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        let report = ferro_core::maintenance::optimize(
            &conn,
            &paths::db_path(),
            &paths::maildir_dir(),
            &state.search_index,
            &paths::search_index_dir(),
        )
        .map_err(|e| e.to_string())?;
        Ok(OptimizeReportView {
            db_before: report.db_before,
            db_after: report.db_after,
            mail_before: report.mail_before,
            mail_after: report.mail_after,
            index_before: report.index_before,
            index_after: report.index_after,
            freed: report.freed(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
struct SavedSearchView {
    name: String,
    query: String,
    /// 未読の件数（`saved_searches::UNREAD_COUNT_CAP`まで）。
    unread: usize,
}

/// 保存した検索の一覧を、それぞれの未読数を付けて返す（サイドバー用）。未読数を数えられ
/// なかった検索（書式の誤り等）は0にする（一覧自体は出したいため）。
#[tauri::command(async)]
fn list_saved_searches(state: State<AppState>, utc_offset_seconds: Option<i64>) -> Result<Vec<SavedSearchView>, String> {
    let file = saved_searches::load(&paths::saved_searches_config_path()).map_err(|e| e.to_string())?;
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    Ok(file
        .searches
        .into_iter()
        .map(|search| {
            let unread = saved_searches::unread_count(
                &conn,
                &state.search_index,
                &search.query,
                utc_offset_seconds.unwrap_or(0),
            )
            .unwrap_or(0);
            SavedSearchView { name: search.name, query: search.query, unread }
        })
        .collect())
}

/// 検索を保存する（同じ名前があれば検索条件を置き換える）。
#[tauri::command]
fn add_saved_search(name: String, query: String) -> Result<(), String> {
    saved_searches::add(&paths::saved_searches_config_path(), SavedSearch { name, query })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn remove_saved_search(index: usize) -> Result<(), String> {
    saved_searches::remove(&paths::saved_searches_config_path(), index)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 既存の全メッセージへ遡って受信ルールを適用する（「今すぐ適用」ボタン）。戻り値は変わったメッセージの件数。
/// `purge_expired_messages`と同じ理由で`spawn_blocking`に逃がす（1000万件規模だと
/// 数秒〜数十秒かかりうる重い処理のため）。
#[tauri::command]
async fn apply_label_rules_now(app: AppHandle) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
        let rules = label_rules::load(&paths::label_rules_config_path()).map_err(|e| e.to_string())?;
        label_rules::apply_all(&conn, &paths::maildir_dir(), &rules).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// OSにインストールされているフォントのファミリー名一覧（重複除去・アルファベット順）。
/// 設定画面の「フォント」プルダウンをこの一覧から選ばせるために使う
/// （`ferro_core::db::settings::Settings::font_family`は固定候補ではなく任意の
/// フォント名を受け付ける。存在しない名前が入っていてもフロント側の
/// フォールバックチェーンで安全に無視されるだけなので、ここで検証はしない）。
#[tauri::command]
fn list_installed_fonts() -> Result<Vec<String>, String> {
    use font_kit::source::SystemSource;
    let mut families = SystemSource::new().all_families().map_err(|e| e.to_string())?;
    families.sort();
    families.dedup();
    Ok(families)
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
        let _manual_guard = ManualSyncGuard::new(&state.manual_syncs);
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

        let label_rules = label_rules::load_or_default(&paths::label_rules_config_path());
        sync::sync_account_with_limit(
            &conn,
            &paths::maildir_dir(),
            &account,
            &password,
            allow_plaintext,
            limit,
            &state.search_index,
            &label_rules,
            settings::load_or_default(&paths::settings_config_path()).server_delete_cutoff(),
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

/// 検索欄の文字列で検索し（`from:`/`has:attachment`/`before:`等の詳細検索の書式も
/// 受け付ける。`ferro_core::search::query`参照）、受信日時の新しい順に返す。
/// `utc_offset_seconds`は利用者のタイムゾーン（`after:`/`before:`の日付の区切り）。
#[tauri::command(async)]
fn search_messages(
    state: State<AppState>,
    query: String,
    limit: usize,
    utc_offset_seconds: Option<i64>,
) -> Result<Vec<MessageView>, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let ids = ferro_core::message_search::search_query(
        &conn,
        &state.search_index,
        &query,
        utc_offset_seconds.unwrap_or(0),
        limit,
    )
    .map_err(|e| e.to_string())?;
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
///
/// DB接続は`read_conn`/`write_conn`を借りず、この処理専用に新しく開く。全件の再構築は
/// メールボックスの規模次第で数分かかり、その間共有の接続を握り続けると、一覧表示・
/// フォルダ件数・本文表示などがロック待ちで止まってアプリ全体が固まる（以前は
/// `read_conn`を握りっぱなしにしていて実際に固まった）。WALモードなので別接続の
/// 書き込み（`mark_indexed`/`update_preview`）と`write_conn`側の同期は共存できる。
#[tauri::command]
async fn reindex_all(app: AppHandle) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let conn = db::open(&paths::db_path()).map_err(|e| e.to_string())?;
        // 二重起動の防止と、再構築中の拾い直し（起動時/定期の`catch_up_unindexed`）の
        // 停止は`reindex_all_with_progress`側（`SearchIndex::begin_rebuild`）が行う。
        logging::log_line("reindex: started");
        let result =
            reindex::reindex_all_with_progress(&conn, &paths::maildir_dir(), &state.search_index, |p| {
                let _ = app.emit(
                    "reindex-progress",
                    ReindexProgressEvent { processed: p.processed, total: p.total },
                );
            });
        match &result {
            Ok(count) => logging::log_line(&format!("reindex: finished, indexed={count}")),
            Err(e) => logging::log_line(&format!("reindex: failed: {e}")),
        }
        result.map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
struct IndexStatusView {
    total: i64,
    indexed: i64,
    index_docs: u64,
    rebuilding: bool,
}

/// 設定画面の「インデックスの状態」表示用（`reindex::index_status`参照）。
/// 全件のCOUNTを取るので、画面を開いた時と利用者が押した時にだけ呼ばれる。
#[tauri::command(async)]
fn get_search_index_status(state: State<AppState>) -> Result<IndexStatusView, String> {
    let conn = state.read_conn.lock().map_err(|e| e.to_string())?;
    let status = reindex::index_status(&conn, &state.search_index).map_err(|e| e.to_string())?;
    Ok(IndexStatusView {
        total: status.total,
        indexed: status.indexed,
        index_docs: status.index_docs,
        rebuilding: status.rebuilding,
    })
}

/// メール保持期間（`settings::Settings::retention_days`）が切れたメッセージを
/// まとめて完全に削除する（設定画面の「今すぐ整理する」ボタン用）。
/// `spawn_retention_cleanup`が定期的に同じ処理をバックグラウンドで行うが、
/// 保持日数を変更した直後にすぐ効果を見たい場合のためにも手動実行できるようにする。
/// 保持日数が0(無期限)の場合は何もせず0を返す。
#[tauri::command]
async fn purge_expired_messages(app: AppHandle) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        // 設定ファイルが一時的に読めない/壊れていても、安全な既定値(0=無期限＝
        // 何も削除しない)にフォールバックする（`load_or_default`のドキュメント参照）。
        let retention_days = settings::load_or_default(&paths::settings_config_path()).retention_days;
        if retention_days <= 0 {
            return Ok(0);
        }
        let cutoff = now_unix() - retention_days * 24 * 60 * 60;

        let mut total = 0usize;
        loop {
            let purged = {
                let conn = state.write_conn.lock().map_err(|e| e.to_string())?;
                ferro_core::message_actions::purge_expired_batch(
                    &conn,
                    &paths::maildir_dir(),
                    &state.search_index,
                    cutoff,
                    500,
                )
                .map_err(|e| e.to_string())?
            };
            if purged == 0 {
                break;
            }
            total += purged;
        }
        Ok(total)
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

/// 設定画面の「Sync間隔」を読む。設定ファイルの読み取りに失敗した場合・
/// 0以下が保存されていた場合はデフォルト(5分)にフォールバックする
/// （バックグラウンドスレッドを止めないため、ここではエラーを伝播しない。
/// `settings::load_or_default`参照）。
fn current_sync_interval() -> Duration {
    let minutes = settings::load_or_default(&paths::settings_config_path()).sync_interval_minutes;
    let minutes = if minutes > 0 { minutes } else { 5 };
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
            std::thread::sleep(current_sync_interval());
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
                // 吸収済みなので、ここに来るのはSQLite側の失敗(ロック競合等)か、
                // writerが作り直しても使えないままの場合（`ferro.log`に記録される）。
                // after_idは据え置いたまま少し待って次回同じ位置からやり直す。
                Err(_) => std::thread::sleep(Duration::from_secs(2)),
            }
        }
    });
}

/// スレッド表示（`ferro_core::threads`）を入れる前に受信していたメッセージの
/// スレッドを、起動後にバックグラウンドで少しずつ埋める。新しく受信したメッセージは
/// sync時に判定済みなので、埋め終われば以後は起動直後に「残り無し」を確かめるだけ
/// （`idx_messages_thread_pending`が空になるので全件は舐めない）。
///
/// `reindex_all`と同じ理由で`write_conn`は借りず専用の接続で動かす（Maildirを読み直す
/// ため1バッチ数秒かかりうるので、その間同期や既読化を待たせない）。
/// v0.0.39までの1通1ファイルの保存を、生メールの保存先DB（`maildir`の`messages.db`）へ移す
/// （`maildir::migrate_all_files`）。移し終わるまでも読み込みはファイルから行えるので、
/// 何もしない起動と同じように使える。
fn spawn_storage_migration() {
    std::thread::spawn(|| {
        let _ = ferro_core::maildir::migrate_all_files(&paths::maildir_dir());
    });
}

fn spawn_thread_backfill() {
    std::thread::spawn(|| {
        let Ok(conn) = db::open(&paths::db_path()) else {
            return;
        };
        let mut after_id = 0i64;
        let mut total = 0usize;
        loop {
            match ferro_core::threads::backfill_batch(&conn, &paths::maildir_dir(), after_id, 500) {
                Ok((count, Some(next))) => {
                    total += count;
                    after_id = next;
                    std::thread::sleep(Duration::from_millis(100));
                }
                Ok((_, None)) => break,
                Err(e) => {
                    logging::log_line(&format!("threads: backfill failed after id={after_id}: {e}"));
                    return;
                }
            }
        }
        if total > 0 {
            logging::log_line(&format!("threads: backfilled {total} message(s)"));
        }
    });
}

/// メール保持期間（`settings::Settings::retention_days`）のバックグラウンド
/// 掃除スレッド。起動直後に一度、以後は`RETENTION_CHECK_INTERVAL`ごとに実行する
/// （保持日数は同期間隔ほど頻繁に変える設定ではないと想定されるため、短い間隔は
/// 不要。すぐに効果を見たい場合は設定画面の「今すぐ整理する」ボタン
/// (`purge_expired_messages`コマンド)を使う）。
const RETENTION_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

fn spawn_retention_cleanup(app_handle: AppHandle) {
    std::thread::spawn(move || {
        loop {
            run_retention_cleanup_once(&app_handle);
            std::thread::sleep(RETENTION_CHECK_INTERVAL);
        }
    });
}

fn run_retention_cleanup_once(app_handle: &AppHandle) {
    let state = app_handle.state::<AppState>();
    // 設定ファイルが一時的に読めない/壊れていても、安全な既定値(0=無期限＝
    // 何も削除しない)にフォールバックする（`load_or_default`のドキュメント参照）。
    let retention_days = settings::load_or_default(&paths::settings_config_path()).retention_days;
    // 0(無期限)がデフォルト。既存ユーザーが明示的に設定しない限り何も削除しない。
    if retention_days <= 0 {
        return;
    }
    let cutoff = now_unix() - retention_days * 24 * 60 * 60;

    let mut total = 0usize;
    loop {
        let purged = {
            let Ok(conn) = state.write_conn.lock() else {
                return;
            };
            match ferro_core::message_actions::purge_expired_batch(
                &conn,
                &paths::maildir_dir(),
                &state.search_index,
                cutoff,
                500,
            ) {
                Ok(n) => n,
                // ベストエフォート。次回の定期実行で再挑戦する。
                Err(_) => return,
            }
        };
        if purged == 0 {
            break;
        }
        total += purged;
    }
    if total > 0 {
        let _ = app_handle.emit("retention-purge", RetentionPurgeEvent { purged: total });
    }
}

#[derive(Serialize, Clone)]
struct RetentionPurgeEvent {
    purged: usize,
}

fn run_background_sync_once(app_handle: &AppHandle) {
    let state = app_handle.state::<AppState>();

    let (accounts, latest_id) = {
        let Ok(conn) = state.read_conn.lock() else {
            return;
        };
        match (db::accounts::list(&conn), ferro_core::new_mail::latest_id(&conn)) {
            (Ok(accounts), Ok(latest_id)) => (accounts, latest_id),
            _ => return,
        }
    };
    let label_rules = label_rules::load_or_default(&paths::label_rules_config_path());
    let server_delete_before =
        settings::load_or_default(&paths::settings_config_path()).server_delete_cutoff();

    for account in accounts {
        let event = match credentials::get_password(account.id) {
            Err(e) => {
                let error = format!("failed to read password from OS keyring: {e}");
                // 同期そのものに入れなかった失敗も、アカウント一覧の同期状態に残す。
                if let Ok(conn) = state.write_conn.lock() {
                    let _ = db::sync_status::record_failure(&conn, account.id, now_unix(), &error);
                }
                BackgroundSyncEvent { account_id: account.id, summary: None, error: Some(error) }
            }
            Ok(password) => {
                // 手動Syncが走っている（またはロック待ち）なら、そのアカウント以降の
                // 自動同期は次のサイクルに回す。手動Syncが全件取り終えた直後に
                // 上限付きの自動同期が走って結果表示を上書きするのを避ける。
                if state.manual_syncs.load(Ordering::SeqCst) > 0 {
                    break;
                }
                let Ok(conn) = state.write_conn.lock() else {
                    break;
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
                    &label_rules,
                    server_delete_before,
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
    notify_new_mail(app_handle, latest_id);
    update_unread_badge(app_handle);
    // sync中にTantivyのIndexWriterクラッシュ(`with_commit_retry`のドキュメント参照)で
    // 検索インデックスへの投入が漏れたメッセージの拾い直しは、呼び出し元
    // (`spawn_background_sync`)がサイクルのたびに`step_search_catchup`で行う。
}

/// メインウィンドウを見ている最中（表示されていてフォーカスがある）か。
fn main_window_is_in_use(app_handle: &AppHandle) -> bool {
    app_handle.get_webview_window("main").is_some_and(|window| {
        window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
    })
}

/// バックグラウンド同期で`after_id`より後に受信したメッセージがあれば、デスクトップ通知を
/// 出す（`settings::Settings::notify_new_mail`/`notify_labels`、`ferro_core::new_mail`参照）。
/// ウィンドウを見ている間は、画面上の「新着メッセージがあります」で足りるので出さない。
fn notify_new_mail(app_handle: &AppHandle, after_id: i64) {
    let settings = settings::load_or_default(&paths::settings_config_path());
    if !settings.notify_new_mail || main_window_is_in_use(app_handle) {
        return;
    }
    let state = app_handle.state::<AppState>();
    let new_mail = {
        let Ok(conn) = state.read_conn.lock() else {
            return;
        };
        match ferro_core::new_mail::since(&conn, after_id, &settings.notify_labels, 3) {
            Ok(new_mail) => new_mail,
            Err(_) => return,
        }
    };
    if new_mail.count == 0 {
        return;
    }
    let (title, body) = ferro_core::new_mail::notification_text(&new_mail);
    if let Err(e) = app_handle.notification().builder().title(title).body(body).show() {
        logging::log_line(&format!("notify: failed to show a notification: {e}"));
    }
}

/// メインウィンドウを表示して前面に出す（トレイアイコンのクリック・メニューから）。
fn show_main_window(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

const TRAY_SHOW_ID: &str = "tray-show";
const TRAY_SYNC_ID: &str = "tray-sync";
const TRAY_QUIT_ID: &str = "tray-quit";

/// システムトレイのアイコン。左クリックでウィンドウを表示し、右クリックのメニューから
/// 「今すぐ同期」「終了」もできる。`close_to_tray`がオンのときはウィンドウを閉じても
/// 終了せずここに残る（`run`の`on_window_event`参照）ので、終了はこのメニューから行う。
const TRAY_ID: &str = "main";

fn build_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id(TRAY_SHOW_ID, "Ferroを開く").build(app)?;
    let sync = MenuItemBuilder::with_id(TRAY_SYNC_ID, "今すぐ同期").build(app)?;
    let quit = MenuItemBuilder::with_id(TRAY_QUIT_ID, "終了").build(app)?;
    let menu = MenuBuilder::new(app).item(&show).item(&sync).separator().item(&quit).build()?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Ferro")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app_handle, event| match event.id().as_ref() {
            TRAY_SHOW_ID => show_main_window(app_handle),
            TRAY_SYNC_ID => {
                let app_handle = app_handle.clone();
                std::thread::spawn(move || run_background_sync_once(&app_handle));
            }
            TRAY_QUIT_ID => app_handle.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// SettingsとMessages一覧はOSネイティブのメニューバーから切り替える別画面として
/// 扱う（同じウィンドウ内でフロント側の表示を切り替えるだけで、別ウィンドウは
/// 開かない）。Account管理はSettings画面内の「アカウント」セクションに統合されている
/// （`SettingsView.svelte`参照）ので、専用のメニュー項目は無い。メニュー項目の
/// クリックは`navigate`イベントとしてフロントへ通知し、`App.svelte`側で画面を切り替える。
const NAV_MESSAGES_ID: &str = "nav-messages";
const NAV_SETTINGS_ID: &str = "nav-settings";
/// 更新確認自体はフロント側(`tauri-plugin-updater`のJS API)で行うため、
/// ここでは`check-for-update`イベントを投げるだけ（`frontend/src/lib/updater.svelte.js`参照）。
const CHECK_FOR_UPDATE_ID: &str = "check-for-update";

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

    // 設定はv0.0.6までSQLiteの`settings`テーブルに保存していたが、
    // accounts.toml/color_rules.tomlと同じくファイルベースに移行した
    // （`settings::migrate_from_db_once`参照）。設定ファイルが既に存在する
    // 場合は何もしないので、既存ユーザーの初回起動時にだけ一度実行される。
    if let Err(e) = settings::migrate_from_db_once(&write_conn, &paths::settings_config_path()) {
        eprintln!("warning: failed to migrate settings from the old database table: {e}");
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .manage(AppState {
            write_conn: Mutex::new(write_conn),
            read_conn: Mutex::new(read_conn),
            search_index,
            manual_syncs: AtomicUsize::new(0),
        })
        .setup(|app| {
            spawn_background_sync(app.handle().clone());
            spawn_initial_search_catchup(app.handle().clone());
            spawn_retention_cleanup(app.handle().clone());
            spawn_thread_backfill();
            spawn_storage_migration();
            build_tray(app)?;
            let app_handle = app.handle().clone();
            std::thread::spawn(move || update_unread_badge(&app_handle));

            let messages_item = MenuItemBuilder::with_id(NAV_MESSAGES_ID, "Messages").build(app)?;
            let settings_item = MenuItemBuilder::with_id(NAV_SETTINGS_ID, "Settings…").build(app)?;
            let check_update_item =
                MenuItemBuilder::with_id(CHECK_FOR_UPDATE_ID, "Check for Updates…").build(app)?;
            let view_menu = SubmenuBuilder::new(app, "View")
                .item(&messages_item)
                .item(&settings_item)
                .separator()
                .item(&check_update_item)
                .build()?;
            let menu = MenuBuilder::new(app).item(&view_menu).build()?;
            app.set_menu(menu)?;

            Ok(())
        })
        // 「閉じたらトレイに常駐」がオンなら、閉じるボタンでは終了せずウィンドウを隠すだけにする
        // （バックグラウンド同期と新着通知はそのまま続く）。設定は閉じる時点で読み直す。
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == "main"
                && settings::load_or_default(&paths::settings_config_path()).close_to_tray
            {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .on_menu_event(|app_handle, event| {
            if event.id().as_ref() == CHECK_FOR_UPDATE_ID {
                let _ = app_handle.emit("check-for-update", ());
                return;
            }
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
            get_search_index_status,
            purge_expired_messages,
            get_message_detail,
            save_attachment,
            save_message_eml,
            get_message_source,
            list_saved_searches,
            optimize_storage,
            refresh_unread_badge,
            highlight_terms,
            add_saved_search,
            remove_saved_search,
            set_read,
            set_flagged,
            set_deleted,
            set_archived,
            set_snoozed,
            bulk_update,
            mark_all_read,
            get_folder_counts,
            list_labels,
            create_label,
            delete_label,
            set_message_label,
            get_settings,
            update_settings,
            reload_accounts_config,
            accounts_config_path,
            log_file_path,
            app_version,
            get_color_rules,
            add_color_rule,
            remove_color_rule,
            color_rules_config_path,
            get_label_rules,
            add_label_rule,
            remove_label_rule,
            label_rules_config_path,
            apply_label_rules_now,
            list_installed_fonts
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Ferro desktop app");
}
