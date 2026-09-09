//! アプリケーション状態。`ui.rs`が読んで描画し、`keymap.rs`がキー入力に応じて変更する。

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ferro_core::color_rules::ColorRule;
use ferro_core::db::accounts::Account;
use ferro_core::db::labels::LabelWithCount;
use ferro_core::db::messages::{self, Folder, FolderCounts, Message};
use ferro_core::db::Connection;
use ferro_core::mail::attachments::AttachmentInfo;
use ferro_core::search::SearchIndex;
use ferro_core::settings::Settings;
use ferro_core::{color_rules, mail, maildir, message_actions, paths, settings};
use tui_input::Input;

use crate::background::{self, BackgroundEvent};

/// サイドバーで選べる項目。フォルダかラベルのどちらか一方だけを選ぶ
/// （GUI版のSidebar.svelteと同じ設計）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarSelection {
    Folder(Folder),
    Label(i64),
}

pub const FOLDERS: [(Folder, &str); 5] = [
    (Folder::Inbox, "受信箱"),
    (Folder::Starred, "スター付き"),
    (Folder::Snoozed, "スヌーズ"),
    (Folder::Archive, "アーカイブ"),
    (Folder::Trash, "ゴミ箱"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    List,
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Messages,
    Settings,
}

/// メッセージ詳細（GUI版`MessageDetailView`相当）。
pub struct MessageDetail {
    pub message: Message,
    pub body: Option<String>,
    pub attachments: Vec<AttachmentInfo>,
    pub labels: Vec<ferro_core::db::labels::Label>,
}

const PAGE_SIZE: u32 = 200;
/// 選択行が読み込み済みバッファの末尾からこの件数以内に近づいたら次ページを読む
/// （GUI版`MessageList.svelte`の`PREFETCH_DISTANCE`と同じ考え方）。
const PREFETCH_ROWS: usize = 20;

pub struct App {
    pub read_conn: Arc<Mutex<Connection>>,
    pub write_conn: Arc<Mutex<Connection>>,
    pub search_index: Arc<SearchIndex>,
    pub bg_rx: Receiver<BackgroundEvent>,
    /// バックグラウンドスレッドと同じchannelへ送るための複製。手動同期
    /// （`sync_now`）の進捗・結果も同じ`status`表示に流し込むために使う。
    pub bg_tx: Sender<BackgroundEvent>,

    pub should_quit: bool,
    pub screen: Screen,
    pub focus: Pane,
    pub show_help: bool,

    pub folder_counts: FolderCounts,
    pub labels: Vec<LabelWithCount>,
    pub selection: SidebarSelection,
    pub sidebar_cursor: usize,

    pub items: Vec<Message>,
    pub list_cursor: usize,
    pub list_exhausted: bool,
    pub list_loading: bool,

    pub detail: Option<MessageDetail>,
    /// 本文を表示してから`mark_read_delay`秒後に既読化するためのタイマー。
    pub read_delay_deadline: Option<Instant>,

    pub search_mode: bool,
    pub search_input: Input,
    pub search_results: Option<Vec<Message>>,

    pub color_rules: Vec<ColorRule>,
    pub settings: Settings,
    pub accounts: Vec<Account>,

    /// スヌーズ確認待ち（`z`の後に1/3/7のいずれかを待つ状態）。
    pub awaiting_snooze_choice: bool,

    pub status: Option<String>,
}

impl App {
    pub fn new(
        read_conn: Arc<Mutex<Connection>>,
        write_conn: Arc<Mutex<Connection>>,
        search_index: Arc<SearchIndex>,
        bg_rx: Receiver<BackgroundEvent>,
        bg_tx: Sender<BackgroundEvent>,
    ) -> anyhow::Result<Self> {
        let mut app = App {
            read_conn,
            write_conn,
            search_index,
            bg_rx,
            bg_tx,
            should_quit: false,
            screen: Screen::Messages,
            focus: Pane::List,
            show_help: false,
            folder_counts: FolderCounts::default(),
            labels: Vec::new(),
            selection: SidebarSelection::Folder(Folder::Inbox),
            sidebar_cursor: 0,
            items: Vec::new(),
            list_cursor: 0,
            list_exhausted: false,
            list_loading: false,
            detail: None,
            read_delay_deadline: None,
            search_mode: false,
            search_input: Input::default(),
            search_results: None,
            color_rules: Vec::new(),
            settings: Settings::default(),
            accounts: Vec::new(),
            awaiting_snooze_choice: false,
            status: None,
        };
        app.refresh_all()?;
        app.load_more()?;
        app.load_selected_detail()?;
        Ok(app)
    }

    fn now_unix() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is before the Unix epoch")
            .as_secs() as i64
    }

    /// フォルダ件数・ラベル・設定・アカウント・色分けルールを読み直す。
    pub fn refresh_all(&mut self) -> anyhow::Result<()> {
        let conn = self.read_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        self.folder_counts = messages::folder_counts(&conn, None, Self::now_unix())?;
        self.labels = ferro_core::db::labels::list_with_counts(&conn)?;
        self.accounts = ferro_core::db::accounts::list(&conn)?;
        drop(conn);
        self.settings = settings::load(&paths::settings_config_path())?;
        self.color_rules = color_rules::load(&paths::color_rules_config_path())?.rules;
        Ok(())
    }

    /// 現在のフォルダ/ラベル選択に応じて、一覧の次ページを読み込む
    /// （キーセットページネーション。`before`カーソルは`items`最後の`date_header`）。
    pub fn load_more(&mut self) -> anyhow::Result<()> {
        if self.list_loading || self.list_exhausted || self.search_results.is_some() {
            return Ok(());
        }
        self.list_loading = true;
        let before = self.items.last().map(|m| m.date_header);
        let conn = self.read_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let page = match self.selection {
            SidebarSelection::Folder(folder) => {
                messages::list_by_folder(&conn, folder, None, before, PAGE_SIZE, Self::now_unix())?
            }
            SidebarSelection::Label(label_id) => messages::list_by_label(&conn, label_id, before, PAGE_SIZE)?,
        };
        drop(conn);
        self.list_loading = false;
        if page.is_empty() {
            self.list_exhausted = true;
        } else {
            self.items.extend(page);
        }
        Ok(())
    }

    /// 選択中フォルダ/ラベルを切り替え、一覧を最初から読み直す。
    pub fn select_sidebar(&mut self, selection: SidebarSelection) -> anyhow::Result<()> {
        self.selection = selection;
        self.items.clear();
        self.list_cursor = 0;
        self.list_exhausted = false;
        self.detail = None;
        self.search_mode = false;
        self.search_results = None;
        self.load_more()?;
        self.load_selected_detail()?;
        Ok(())
    }

    /// 一覧のカーソルを動かす。末尾付近に近づいたら次ページを先読みする。
    pub fn move_list_cursor(&mut self, delta: i64) -> anyhow::Result<()> {
        let len = self.current_list().len();
        if len == 0 {
            return Ok(());
        }
        let new_cursor = (self.list_cursor as i64 + delta).clamp(0, len as i64 - 1) as usize;
        self.list_cursor = new_cursor;
        if self.search_results.is_none() && len.saturating_sub(new_cursor) <= PREFETCH_ROWS {
            self.load_more()?;
        }
        self.load_selected_detail()?;
        Ok(())
    }

    pub fn current_list(&self) -> &[Message] {
        self.search_results.as_deref().unwrap_or(&self.items)
    }

    /// 選択中メッセージの詳細を読み込む（GUI版`get_message_detail`と同じ組み合わせ:
    /// DB行 + Maildirの生データ + 本文プレーンテキスト抽出 + 添付一覧 + ラベル）。
    pub fn load_selected_detail(&mut self) -> anyhow::Result<()> {
        self.read_delay_deadline = None;
        let Some(message) = self.current_list().get(self.list_cursor).cloned() else {
            self.detail = None;
            return Ok(());
        };
        let conn = self.read_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let raw = maildir::load(&paths::maildir_dir(), message.account_id, &message.uidl).ok();
        let labels = ferro_core::db::labels::for_message(&conn, message.id)?;
        drop(conn);

        let (body, attachments) = match &raw {
            Some(raw) => (
                mail::parse::extract_plain_text_body(raw),
                mail::attachments::list_attachments(raw),
            ),
            None => (None, Vec::new()),
        };

        if !message.is_read {
            if self.settings.mark_read_delay {
                self.read_delay_deadline = Some(Instant::now() + Duration::from_secs(2));
            } else {
                self.set_read(message.id, true)?;
            }
        }

        self.detail = Some(MessageDetail { message, body, attachments, labels });
        Ok(())
    }

    /// `load_selected_detail`が仕掛けた既読化タイマーが満了していれば実行する。
    /// メインループが毎ティック呼ぶ（`mark_read_delay`設定時、本文を2秒表示したら
    /// 既読にするGUI版`MessageDetail.svelte`と同じ体験）。
    pub fn tick(&mut self) -> anyhow::Result<()> {
        if let Some(deadline) = self.read_delay_deadline {
            if Instant::now() >= deadline {
                self.read_delay_deadline = None;
                if let Some(detail) = &self.detail {
                    let id = detail.message.id;
                    self.set_read(id, true)?;
                }
            }
        }
        self.drain_background_events();
        Ok(())
    }

    fn drain_background_events(&mut self) {
        while let Ok(event) = self.bg_rx.try_recv() {
            match event {
                BackgroundEvent::SyncProgress { account_id, fetched, total } => {
                    self.status = Some(format!("account #{account_id}: syncing… {fetched}/{total}"));
                }
                BackgroundEvent::SyncFinished { account_id, result } => {
                    self.status = Some(match result {
                        Ok(summary) => format!(
                            "account #{account_id}: fetched {}, {} remaining",
                            summary.fetched, summary.remaining
                        ),
                        Err(e) => format!("account #{account_id}: error: {e}"),
                    });
                    let _ = self.refresh_all();
                }
                BackgroundEvent::RetentionPurged { count } => {
                    self.status = Some(format!("保持期間切れメッセージを{count}件削除しました"));
                    let _ = self.refresh_all();
                }
                BackgroundEvent::ManualPurgeFinished { result } => {
                    self.status = Some(match result {
                        Ok(0) => "保持期間が無期限のため、何も削除されませんでした".to_string(),
                        Ok(count) => format!("{count}件を削除しました"),
                        Err(e) => format!("整理に失敗しました: {e}"),
                    });
                    let _ = self.refresh_all();
                }
                BackgroundEvent::ReindexFinished { result } => {
                    self.status = Some(match result {
                        Ok(count) => format!("{count}件を再構築しました"),
                        Err(e) => format!("再構築に失敗しました: {e}"),
                    });
                }
            }
        }
    }

    /// 現在選択中のメッセージに対する操作をまとめたヘルパー群。
    /// いずれも即座にDBへ反映し、一覧側の表示も更新する。
    fn selected_message_id(&self) -> Option<i64> {
        self.current_list().get(self.list_cursor).map(|m| m.id)
    }

    fn update_selected_in_place(&mut self, f: impl Fn(&mut Message)) {
        let id = match self.selected_message_id() {
            Some(id) => id,
            None => return,
        };
        if let Some(m) = self.items.iter_mut().find(|m| m.id == id) {
            f(m);
        }
        if let Some(results) = &mut self.search_results {
            if let Some(m) = results.iter_mut().find(|m| m.id == id) {
                f(m);
            }
        }
        if let Some(detail) = &mut self.detail {
            if detail.message.id == id {
                f(&mut detail.message);
            }
        }
    }

    pub fn set_read(&mut self, id: i64, is_read: bool) -> anyhow::Result<()> {
        let conn = self.write_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        messages::set_read(&conn, id, is_read)?;
        drop(conn);
        self.update_selected_in_place(|m| m.is_read = is_read);
        Ok(())
    }

    pub fn toggle_flagged(&mut self) -> anyhow::Result<()> {
        let Some(id) = self.selected_message_id() else { return Ok(()) };
        let current = self.current_list().get(self.list_cursor).map(|m| m.is_flagged).unwrap_or(false);
        let conn = self.write_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        messages::set_flagged(&conn, id, !current)?;
        drop(conn);
        self.update_selected_in_place(|m| m.is_flagged = !current);
        self.refresh_all()?;
        Ok(())
    }

    pub fn toggle_archived(&mut self) -> anyhow::Result<()> {
        let Some(id) = self.selected_message_id() else { return Ok(()) };
        let current = self.current_list().get(self.list_cursor).map(|m| m.is_archived).unwrap_or(false);
        let conn = self.write_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        messages::set_archived(&conn, id, !current)?;
        drop(conn);
        self.update_selected_in_place(|m| m.is_archived = !current);
        self.refresh_all()?;
        Ok(())
    }

    pub fn toggle_deleted(&mut self) -> anyhow::Result<()> {
        let Some(id) = self.selected_message_id() else { return Ok(()) };
        let current = self.current_list().get(self.list_cursor).map(|m| m.is_deleted).unwrap_or(false);
        let conn = self.write_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        message_actions::set_deleted(&conn, &self.search_index, id, !current)?;
        drop(conn);
        self.update_selected_in_place(|m| m.is_deleted = !current);
        self.refresh_all()?;
        Ok(())
    }

    /// `preset`は"1d"/"3d"/"1w"のいずれか。期限の時刻計算はここ(Rust側)で行う
    /// （GUI版のTauriコマンドと同じ設計）。
    pub fn snooze(&mut self, preset: &str) -> anyhow::Result<()> {
        let Some(id) = self.selected_message_id() else { return Ok(()) };
        let until = match preset {
            "1d" => Some(Self::now_unix() + 24 * 60 * 60),
            "3d" => Some(Self::now_unix() + 3 * 24 * 60 * 60),
            "1w" => Some(Self::now_unix() + 7 * 24 * 60 * 60),
            _ => None,
        };
        let conn = self.write_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        messages::set_snoozed(&conn, id, until)?;
        drop(conn);
        self.refresh_all()?;
        Ok(())
    }

    /// 全アカウントを手動同期する（バックグラウンドスレッドと同じ関数を呼ぶ）。
    /// 呼び出し元(メインの描画/入力ループ)を塞がないよう専用スレッドで実行する
    /// （src-tauriが`sync_account`/`reindex_all`を`spawn_blocking`へ逃がしたのと
    /// 同じ理由。進捗・結果は`bg_tx`を複製して同じchannelへ流す）。
    pub fn sync_now(&mut self) {
        self.status = Some("同期を開始しました…".to_string());
        let write_conn = Arc::clone(&self.write_conn);
        let search_index = Arc::clone(&self.search_index);
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            background::sync_all_accounts_once(&write_conn, &search_index, &tx);
        });
    }

    /// 設定画面の「今すぐ整理する」相当。呼び出し元を塞がないよう別スレッドで
    /// 実行し、結果は`bg_tx`経由で`status`に反映する（`sync_now`と同じ理由）。
    pub fn run_purge_now(&mut self) {
        self.status = Some("整理中…".to_string());
        let write_conn = Arc::clone(&self.write_conn);
        let search_index = Arc::clone(&self.search_index);
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let result = background::purge_expired_now(&write_conn, &search_index);
            let _ = tx.send(BackgroundEvent::ManualPurgeFinished { result });
        });
    }

    /// 設定画面の「検索インデックス再構築」相当。
    pub fn run_reindex_now(&mut self) {
        self.status = Some("検索インデックスを再構築中…".to_string());
        let write_conn = Arc::clone(&self.write_conn);
        let search_index = Arc::clone(&self.search_index);
        let tx = self.bg_tx.clone();
        std::thread::spawn(move || {
            let result = background::reindex_all_now(&write_conn, &search_index);
            let _ = tx.send(BackgroundEvent::ReindexFinished { result });
        });
    }

    pub fn start_search(&mut self) {
        self.search_mode = true;
        self.focus = Pane::List;
        self.search_input = Input::default();
    }

    pub fn cancel_search(&mut self) {
        self.search_mode = false;
        self.search_results = None;
        self.list_cursor = 0;
    }

    pub fn run_search(&mut self) -> anyhow::Result<()> {
        let query = self.search_input.value().to_string();
        if query.trim().is_empty() {
            self.search_results = None;
            return Ok(());
        }
        let ids = self.search_index.search(&query, 200)?;
        let conn = self.read_conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut results = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(m) = messages::get(&conn, id)? {
                results.push(m);
            }
        }
        self.search_results = Some(results);
        self.list_cursor = 0;
        Ok(())
    }
}
