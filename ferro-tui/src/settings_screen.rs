//! 設定画面（`,`で開く）。項目をカーソルで選んで、その場で変更・保存する。
//!
//! 変更した設定は`settings.toml`/`color_rules.toml`/`accounts.toml`へすぐ保存する
//! （GUIの設定画面と同じファイルなので、どちらで変えても同じ結果になる）。
//! 数値・文字列・ルール/アカウントの追加は、画面中央に出す入力フォーム（`Form`）で行い、
//! アカウントの削除だけは確認（`Confirm`）を挟む（そのアカウントのメールも全て消えるため）。
//!
//! ここでのエラーは`?`で呼び出し元へ返さず、ステータス行に出す（`keymap::handle_key`の
//! エラーはTUIごと終了させてしまうため）。

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ferro_core::account_config::AccountConfig;
use ferro_core::color_rules::ColorRule;
use ferro_core::settings::{self, Settings};
use ferro_core::{account_setup, color_rules, paths};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::app::{App, Screen};
use crate::colors::parse_hex_color;

/// 設定画面の1行。
enum Item {
    Header(&'static str),
    Toggle {
        label: &'static str,
        get: fn(&Settings) -> bool,
        set: fn(&mut Settings, bool),
    },
    /// ←/→（Enter）で選択肢を順に切り替える。`options`は(保存する値, 表示名)。
    Choice {
        label: &'static str,
        options: &'static [(&'static str, &'static str)],
        get: fn(&Settings) -> String,
        set: fn(&mut Settings, &str),
    },
    /// Enterで入力フォーム、←/→で±1。`zero`は0のときの表示（「無期限」等）。
    Number {
        label: &'static str,
        unit: &'static str,
        zero: &'static str,
        get: fn(&Settings) -> i64,
        set: fn(&mut Settings, i64),
    },
    /// Enterで入力フォーム。`set`は不正な値ならエラーメッセージを返す。
    Text {
        label: &'static str,
        get: fn(&Settings) -> String,
        set: fn(&mut Settings, &str) -> Result<(), String>,
    },
    ColorRule(usize),
    AddColorRule,
    Account(usize),
    AddAccount,
    Reindex,
    Purge,
}

const THEMES: &[(&str, &str)] = &[("light", "ライト"), ("dark", "ダーク"), ("system", "システムに合わせる")];
const FONT_SIZES: &[(&str, &str)] = &[("small", "小"), ("medium", "中"), ("large", "大")];
const SYNC_INTERVALS: &[(&str, &str)] =
    &[("1", "1分"), ("5", "5分"), ("10", "10分"), ("15", "15分"), ("30", "30分"), ("60", "60分")];

fn items(app: &App) -> Vec<Item> {
    let mut items = vec![
        Item::Header("同期・保持期間"),
        Item::Choice {
            label: "バックグラウンド自動同期の間隔",
            options: SYNC_INTERVALS,
            get: |s| s.sync_interval_minutes.to_string(),
            set: |s, v| s.sync_interval_minutes = v.parse().unwrap_or(5),
        },
        Item::Number {
            label: "メールの保持日数",
            unit: "日",
            zero: "無期限",
            get: |s| s.retention_days,
            set: |s, v| s.retention_days = v,
        },
        Item::Number {
            label: "サーバー側メールの削除",
            unit: "日経過後",
            zero: "しない",
            get: |s| s.server_delete_days,
            set: |s, v| s.server_delete_days = v,
        },
        Item::Purge,
        Item::Header("表示・検索"),
        Item::Toggle {
            label: "既読にするまでの時間（2秒表示したら既読）",
            get: |s| s.mark_read_delay,
            set: |s, v| s.mark_read_delay = v,
        },
        Item::Reindex,
        Item::Header("GUIの設定（このファイルを共有しているので、ここからも変えられます）"),
        Item::Toggle { label: "一覧のプレビュー行", get: |s| s.show_preview_line, set: |s, v| s.show_preview_line = v },
        Item::Toggle {
            label: "差出人アイコンを表示",
            get: |s| s.show_sender_avatar,
            set: |s, v| s.show_sender_avatar = v,
        },
        Item::Choice { label: "テーマ", options: THEMES, get: |s| s.theme.clone(), set: |s, v| s.theme = v.to_string() },
        Item::Text {
            label: "アクセントカラー（#RRGGBB）",
            get: |s| s.accent_color.clone(),
            set: |s, v| {
                parse_hex_color(v).ok_or("#RRGGBBの形で入力してください")?;
                s.accent_color = v.to_string();
                Ok(())
            },
        },
        Item::Text {
            label: "フォント",
            get: |s| s.font_family.clone(),
            set: |s, v| {
                if v.trim().is_empty() {
                    return Err("フォント名を入力してください".into());
                }
                s.font_family = v.trim().to_string();
                Ok(())
            },
        },
        Item::Choice {
            label: "文字サイズ",
            options: FONT_SIZES,
            get: |s| s.font_size.clone(),
            set: |s, v| s.font_size = v.to_string(),
        },
        Item::Toggle {
            label: "1日1回、更新を自動確認",
            get: |s| s.auto_update_check,
            set: |s, v| s.auto_update_check = v,
        },
        Item::Toggle { label: "新着メールを通知", get: |s| s.notify_new_mail, set: |s, v| s.notify_new_mail = v },
        Item::Text {
            label: "通知するラベル（カンマ区切り。空なら全て）",
            get: |s| s.notify_labels.join(", "),
            set: |s, v| {
                s.notify_labels =
                    v.split([',', '、']).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect();
                Ok(())
            },
        },
        Item::Toggle { label: "閉じたらトレイに常駐", get: |s| s.close_to_tray, set: |s, v| s.close_to_tray = v },
        Item::Header("色分けルール（d:削除）"),
    ];
    items.extend((0..app.color_rules.len()).map(Item::ColorRule));
    items.push(Item::AddColorRule);
    items.push(Item::Header("アカウント（d:削除）"));
    items.extend((0..app.accounts.len()).map(Item::Account));
    items.push(Item::AddAccount);
    items
}

/// 入力フォームの1欄。`boolean`があればSpaceで切り替える欄（文字入力はしない）。
pub struct FormField {
    label: &'static str,
    input: Input,
    secret: bool,
    boolean: Option<bool>,
}

impl FormField {
    fn text(label: &'static str, value: &str) -> Self {
        FormField { label, input: Input::new(value.to_string()), secret: false, boolean: None }
    }
    fn secret(label: &'static str) -> Self {
        FormField { label, input: Input::default(), secret: true, boolean: None }
    }
    fn boolean(label: &'static str, value: bool) -> Self {
        FormField { label, input: Input::default(), secret: false, boolean: Some(value) }
    }
    fn value(&self) -> String {
        self.input.value().trim().to_string()
    }
}

enum FormKind {
    /// `items()`の何番目の項目を編集しているか（フォームを開いている間、項目の並びは変わらない）。
    EditItem(usize),
    AddColorRule,
    AddAccount,
}

pub struct Form {
    title: &'static str,
    fields: Vec<FormField>,
    focus: usize,
    kind: FormKind,
    error: Option<String>,
}

/// アカウント削除の確認待ち。
pub struct Confirm {
    message: String,
    account_id: i64,
}

fn is_selectable(item: &Item) -> bool {
    !matches!(item, Item::Header(_))
}

/// カーソルを、選べる行（見出し以外）に合わせる。
fn clamp_cursor(app: &mut App, items: &[Item]) {
    let last = items.len().saturating_sub(1);
    app.settings_cursor = app.settings_cursor.min(last);
    if !items.get(app.settings_cursor).is_some_and(is_selectable) {
        move_cursor(app, items, 1);
    }
}

fn move_cursor(app: &mut App, items: &[Item], delta: i64) {
    let mut index = app.settings_cursor as i64;
    loop {
        index += delta;
        if index < 0 || index >= items.len() as i64 {
            return;
        }
        if is_selectable(&items[index as usize]) {
            app.settings_cursor = index as usize;
            return;
        }
    }
}

/// 設定を変えて`settings.toml`へ保存する。
fn update_settings(app: &mut App, change: impl FnOnce(&mut Settings) -> Result<(), String>) -> Result<(), String> {
    let mut next = app.settings.clone();
    change(&mut next)?;
    settings::save(&paths::settings_config_path(), &next).map_err(|e| e.to_string())?;
    app.settings = next;
    app.status = Some("設定を保存しました".to_string());
    Ok(())
}

fn report(app: &mut App, result: Result<(), String>) {
    if let Err(e) = result {
        app.status = Some(format!("エラー: {e}"));
    }
}

/// 設定画面でのキー入力。フォーム・確認が出ていればそちらを優先する。
pub fn handle_key(app: &mut App, key: KeyEvent) {
    if app.settings_confirm.is_some() {
        handle_confirm_key(app, key);
        return;
    }
    if app.settings_form.is_some() {
        handle_form_key(app, key);
        return;
    }

    let items = items(app);
    clamp_cursor(app, &items);
    let cursor = app.settings_cursor;
    match key.code {
        KeyCode::Esc => app.screen = Screen::Messages,
        KeyCode::Char('j') | KeyCode::Down => move_cursor(app, &items, 1),
        KeyCode::Char('k') | KeyCode::Up => move_cursor(app, &items, -1),
        KeyCode::Char('R') => app.run_reindex_now(),
        KeyCode::Char('P') => app.run_purge_now(),
        KeyCode::Enter | KeyCode::Char(' ') => activate(app, &items, cursor),
        KeyCode::Left | KeyCode::Char('h') => step(app, &items[cursor], -1),
        KeyCode::Right | KeyCode::Char('l') => step(app, &items[cursor], 1),
        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => delete(app, &items[cursor]),
        _ => {}
    }
}

fn activate(app: &mut App, items: &[Item], index: usize) {
    match &items[index] {
        Item::Toggle { get, set, .. } => {
            let value = !get(&app.settings);
            let result = update_settings(app, |s| {
                set(s, value);
                Ok(())
            });
            report(app, result);
        }
        Item::Choice { .. } => step(app, &items[index], 1),
        Item::Number { label, get, .. } => {
            app.settings_form = Some(Form {
                title: label,
                fields: vec![FormField::text("日数（0で無効）", &get(&app.settings).to_string())],
                focus: 0,
                kind: FormKind::EditItem(index),
                error: None,
            });
        }
        Item::Text { label, get, .. } => {
            app.settings_form = Some(Form {
                title: label,
                fields: vec![FormField::text("値", &get(&app.settings))],
                focus: 0,
                kind: FormKind::EditItem(index),
                error: None,
            });
        }
        Item::AddColorRule => {
            app.settings_form = Some(Form {
                title: "色分けルールを追加",
                fields: vec![FormField::text("含む文字列", ""), FormField::text("色（#RRGGBB）", "#d9534f")],
                focus: 0,
                kind: FormKind::AddColorRule,
                error: None,
            });
        }
        Item::AddAccount => {
            app.settings_form = Some(Form {
                title: "アカウントを追加",
                fields: vec![
                    FormField::text("名前", ""),
                    FormField::text("ホスト", ""),
                    FormField::text("ポート", "995"),
                    FormField::text("ユーザー名", ""),
                    FormField::secret("パスワード"),
                    FormField::boolean("TLSを使う（Spaceで切替）", true),
                ],
                focus: 0,
                kind: FormKind::AddAccount,
                error: None,
            });
        }
        Item::Reindex => app.run_reindex_now(),
        Item::Purge => app.run_purge_now(),
        Item::Header(_) | Item::ColorRule(_) | Item::Account(_) => {}
    }
}

/// ←/→: 選択肢を切り替える／数値を1ずつ増減する。
fn step(app: &mut App, item: &Item, delta: i64) {
    let result = match item {
        Item::Choice { options, get, set, .. } => {
            let current = get(&app.settings);
            let index = options.iter().position(|(value, _)| *value == current).unwrap_or(0) as i64;
            let next = options[(index + delta).rem_euclid(options.len() as i64) as usize].0;
            update_settings(app, |s| {
                set(s, next);
                Ok(())
            })
        }
        Item::Number { get, set, .. } => {
            let next = (get(&app.settings) + delta).max(0);
            update_settings(app, |s| {
                set(s, next);
                Ok(())
            })
        }
        _ => Ok(()),
    };
    report(app, result);
}

fn delete(app: &mut App, item: &Item) {
    match *item {
        Item::ColorRule(index) => {
            let result = color_rules::remove(&paths::color_rules_config_path(), index)
                .map(|file| {
                    app.color_rules = file.rules;
                    app.status = Some("色分けルールを削除しました".to_string());
                })
                .map_err(|e| e.to_string());
            report(app, result);
        }
        Item::Account(index) => {
            let Some(account) = app.accounts.get(index) else { return };
            app.settings_confirm = Some(Confirm {
                message: format!(
                    "アカウント「{}」を削除します。このアカウントのメールもDB・Maildir・検索インデックスから\
                     全て消えます（サーバー上のメールは残ります）。よろしいですか？ (y/N)",
                    account.name
                ),
                account_id: account.id,
            });
        }
        _ => {}
    }
}

fn handle_confirm_key(app: &mut App, key: KeyEvent) {
    let Some(confirm) = app.settings_confirm.take() else { return };
    if key.code != KeyCode::Char('y') {
        app.status = Some("削除を取りやめました".to_string());
        return;
    }
    let result = (|| -> Result<(), String> {
        let conn = app.write_conn.lock().map_err(|e| e.to_string())?;
        account_setup::remove(
            &conn,
            &paths::maildir_dir(),
            &app.search_index,
            &paths::accounts_config_path(),
            confirm.account_id,
        )
        .map_err(|e| e.to_string())
    })();
    match result {
        Ok(()) => {
            app.status = Some("アカウントを削除しました".to_string());
            let _ = app.refresh_all();
            let _ = app.select_sidebar(app.selection);
        }
        Err(e) => app.status = Some(format!("エラー: {e}")),
    }
}

fn handle_form_key(app: &mut App, key: KeyEvent) {
    let Some(form) = app.settings_form.as_mut() else { return };
    let last = form.fields.len() - 1;
    match key.code {
        KeyCode::Esc => {
            app.settings_form = None;
            return;
        }
        KeyCode::Tab | KeyCode::Down => form.focus = (form.focus + 1).min(last),
        KeyCode::BackTab | KeyCode::Up => form.focus = form.focus.saturating_sub(1),
        KeyCode::Enter if form.focus < last => form.focus += 1,
        KeyCode::Enter => submit_form(app),
        KeyCode::Char(' ') if form.fields[form.focus].boolean.is_some() => {
            let field = &mut form.fields[form.focus];
            field.boolean = field.boolean.map(|b| !b);
        }
        _ if form.fields[form.focus].boolean.is_none() && !key.modifiers.contains(KeyModifiers::CONTROL) => {
            form.fields[form.focus].input.handle_event(&Event::Key(key));
        }
        _ => {}
    }
}

fn submit_form(app: &mut App) {
    let Some(form) = app.settings_form.take() else { return };
    let values: Vec<String> = form.fields.iter().map(FormField::value).collect();
    let result = match form.kind {
        FormKind::EditItem(index) => submit_item(app, index, &values[0]),
        FormKind::AddColorRule => submit_color_rule(app, &values[0], &values[1]),
        FormKind::AddAccount => submit_account(app, &values, form.fields[5].boolean.unwrap_or(true)),
    };
    // 失敗したら入力をそのまま残してフォームに戻す（打ち直さなくて済むように）。
    if let Err(e) = result {
        app.settings_form = Some(Form { error: Some(e), ..form });
    }
}

fn submit_item(app: &mut App, index: usize, value: &str) -> Result<(), String> {
    let items = items(app);
    match &items[index] {
        Item::Number { set, .. } => {
            let number: i64 = value.parse().map_err(|_| "0以上の整数を入力してください".to_string())?;
            if number < 0 {
                return Err("0以上の整数を入力してください".into());
            }
            update_settings(app, |s| {
                set(s, number);
                Ok(())
            })
        }
        Item::Text { set, .. } => update_settings(app, |s| set(s, value)),
        _ => Ok(()),
    }
}

fn submit_color_rule(app: &mut App, pattern: &str, color: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Err("含む文字列を入力してください".into());
    }
    parse_hex_color(color).ok_or("色は#RRGGBBの形で入力してください")?;
    let file = color_rules::add(
        &paths::color_rules_config_path(),
        ColorRule { pattern: pattern.to_string(), color: color.to_string() },
    )
    .map_err(|e| e.to_string())?;
    app.color_rules = file.rules;
    app.status = Some("色分けルールを追加しました".to_string());
    Ok(())
}

fn submit_account(app: &mut App, values: &[String], use_tls: bool) -> Result<(), String> {
    let [name, host, port, username, password, _] = values else {
        return Err("入力欄の数が想定と違います".into());
    };
    if name.is_empty() || host.is_empty() || username.is_empty() || password.is_empty() {
        return Err("名前・ホスト・ユーザー名・パスワードを入力してください".into());
    }
    let port: u16 = port.parse().map_err(|_| "ポートは1〜65535の数字で入力してください".to_string())?;
    let config = AccountConfig {
        name: name.clone(),
        host: host.clone(),
        port,
        username: username.clone(),
        use_tls,
    };
    {
        let conn = app.write_conn.lock().map_err(|e| e.to_string())?;
        account_setup::add(&conn, &paths::accounts_config_path(), &config, password).map_err(|e| e.to_string())?;
    }
    let _ = app.refresh_all();
    app.status = Some(format!("アカウント「{name}」を追加しました（S で同期できます）"));
    Ok(())
}

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let items = items(app);
    let block = Block::default().borders(Borders::ALL).title(format!(
        "設定 v{}（j/k:移動 Enter/Space:変更 ←→:切替 d:削除 R:再構築 P:整理 Esc:戻る）",
        env!("CARGO_PKG_VERSION")
    ));
    let inner_width = area.width.saturating_sub(4) as usize;
    let list_items: Vec<ListItem> = items.iter().map(|item| ListItem::new(item_line(app, item, inner_width))).collect();
    let mut state = ListState::default();
    state.select(Some(app.settings_cursor.min(items.len().saturating_sub(1))));
    let list = List::new(list_items).block(block).highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut state);

    if let Some(form) = &app.settings_form {
        render_form(f, form, area);
    }
    if let Some(confirm) = &app.settings_confirm {
        let popup = popup_rect(area, 70, 7);
        f.render_widget(Clear, popup);
        let block = Block::default().borders(Borders::ALL).title("確認").border_style(Style::default().fg(Color::Red));
        f.render_widget(Paragraph::new(confirm.message.as_str()).block(block).wrap(Wrap { trim: false }), popup);
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "ON" } else { "OFF" }
}

fn item_line<'a>(app: &'a App, item: &'a Item, width: usize) -> Line<'a> {
    let row = |label: &str, value: String| -> Line<'a> {
        let label = format!("  {label}");
        let used = unicode_width(&label) + unicode_width(&value);
        let gap = width.saturating_sub(used).max(2);
        Line::from(vec![Span::raw(label), Span::raw(" ".repeat(gap)), Span::styled(value, Style::default().fg(Color::Cyan))])
    };
    match item {
        Item::Header(title) => Line::styled(*title, Style::default().add_modifier(Modifier::BOLD)),
        Item::Toggle { label, get, .. } => row(label, on_off(get(&app.settings)).to_string()),
        Item::Choice { label, options, get, .. } => {
            let current = get(&app.settings);
            let shown = options.iter().find(|(value, _)| *value == current).map_or(current.clone(), |(_, name)| name.to_string());
            row(label, format!("‹ {shown} ›"))
        }
        Item::Number { label, unit, zero, get, .. } => {
            let value = get(&app.settings);
            row(label, if value > 0 { format!("{value}{unit}") } else { zero.to_string() })
        }
        Item::Text { label, get, .. } => {
            let value = get(&app.settings);
            row(label, if value.is_empty() { "（なし）".to_string() } else { value })
        }
        Item::ColorRule(index) => {
            let rule = &app.color_rules[*index];
            let color = parse_hex_color(&rule.color).unwrap_or(Color::White);
            Line::from(vec![
                Span::raw("  "),
                Span::styled("● ", Style::default().fg(color)),
                Span::raw(rule.pattern.as_str()),
                Span::styled(format!("  {}", rule.color), Style::default().add_modifier(Modifier::DIM)),
            ])
        }
        Item::AddColorRule => Line::styled("  ＋ 色分けルールを追加", Style::default().fg(Color::Green)),
        Item::Account(index) => {
            let account = &app.accounts[*index];
            let status = app.sync_statuses.get(&account.id);
            let failing = status.is_some_and(|s| s.last_error.is_some());
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            Line::from(vec![
                Span::raw(format!(
                    "  {} — {}@{}:{} ({}{})",
                    account.name,
                    account.username,
                    account.host,
                    account.port,
                    if account.use_tls { "TLS" } else { "平文" },
                    if account.uidl_supported == Some(false) { "、UIDL非対応" } else { "" }
                )),
                Span::styled(
                    format!("  {}", ferro_core::db::sync_status::describe(status, now)),
                    if failing {
                        Style::default().fg(Color::Red)
                    } else {
                        Style::default().add_modifier(Modifier::DIM)
                    },
                ),
            ])
        }
        Item::AddAccount => Line::styled("  ＋ アカウントを追加", Style::default().fg(Color::Green)),
        Item::Reindex => row("検索インデックスを再構築", "Enter / R".to_string()),
        Item::Purge => row("保持日数を過ぎたメールを今すぐ整理", "Enter / P".to_string()),
    }
}

fn render_form(f: &mut Frame, form: &Form, area: Rect) {
    let height = form.fields.len() as u16 + 5 + u16::from(form.error.is_some());
    let popup = popup_rect(area, 70, height);
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("{}（Enter:次へ/確定 Tab:移動 Esc:取消）", form.title))
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let label_width = form.fields.iter().map(|field| unicode_width(field.label)).max().unwrap_or(0) + 2;
    let mut lines = Vec::new();
    let mut cursor = None;
    for (i, field) in form.fields.iter().enumerate() {
        let focused = i == form.focus;
        let value = match field.boolean {
            Some(value) => if value { "[x]" } else { "[ ]" }.to_string(),
            None if field.secret => "*".repeat(field.input.value().chars().count()),
            None => field.input.value().to_string(),
        };
        let label = format!("{}{}", field.label, " ".repeat(label_width - unicode_width(field.label)));
        let style = if focused { Style::default().add_modifier(Modifier::REVERSED) } else { Style::default() };
        if focused && field.boolean.is_none() {
            let shown_before_cursor: usize = if field.secret {
                field.input.cursor()
            } else {
                field.input.visual_cursor()
            };
            cursor = Some(Position::new(
                inner.x + (label_width + shown_before_cursor) as u16,
                inner.y + i as u16,
            ));
        }
        lines.push(Line::from(vec![Span::raw(label), Span::styled(value, style)]));
    }
    lines.push(Line::from(""));
    if let Some(error) = &form.error {
        lines.push(Line::styled(error.as_str(), Style::default().fg(Color::Red)));
    }
    f.render_widget(Paragraph::new(lines), inner);
    if let Some(position) = cursor {
        f.set_cursor_position(position);
    }
}

/// 画面中央に、幅`percent_x`%・高さ`height`行の領域を取る。
fn popup_rect(area: Rect, percent_x: u16, height: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(1), Constraint::Length(height.min(area.height)), Constraint::Fill(1)])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

/// 端末上の表示幅（全角は2）。
fn unicode_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

#[cfg(test)]
mod tests {
    //! 利用者の本物の設定ファイルを書き換えないよう、保存を伴う操作（値の確定）は
    //! ここでは試さない。描画・カーソル移動・フォームの開閉だけを確かめる。
    use std::sync::{Arc, Mutex, mpsc};

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ferro_core::db;
    use ferro_core::search::SearchIndex;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn test_app(dir: &std::path::Path) -> App {
        let conn = || Arc::new(Mutex::new(db::open(std::path::Path::new(":memory:")).unwrap()));
        let index = Arc::new(SearchIndex::open_or_create(dir).unwrap());
        let (tx, rx) = mpsc::channel();
        let mut app = App::new(conn(), conn(), index, rx, tx).unwrap();
        app.screen = Screen::Settings;
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn screen_text(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(140, 50)).unwrap();
        terminal.draw(|f| render(f, app, f.area())).unwrap();
        // 全角文字は2セルを使い、2セル目は空白になるので、空白を除いて比べる。
        terminal.backend().buffer().content().iter().map(|cell| cell.symbol()).filter(|s| *s != " ").collect()
    }

    #[test]
    fn cursor_skips_headers_and_number_items_open_a_form() {
        let dir = std::env::temp_dir().join(format!("ferro-tui-settings-test-{}", std::process::id()));
        let mut app = test_app(&dir);
        let before = app.settings.clone();

        // 先頭は見出しなので、最初のキー操作で最初の項目（同期の間隔）に合う。
        handle_key(&mut app, key(KeyCode::Char('j')));
        handle_key(&mut app, key(KeyCode::Char('k')));
        let list = items(&app);
        assert!(matches!(list[app.settings_cursor], Item::Choice { .. }));

        // 保持日数でEnterすると入力フォームが開き、Escで何も保存せず閉じる。
        handle_key(&mut app, key(KeyCode::Down));
        assert!(matches!(list[app.settings_cursor], Item::Number { .. }));
        handle_key(&mut app, key(KeyCode::Enter));
        assert!(app.settings_form.is_some());
        assert!(screen_text(&app).contains("メールの保持日数"));
        handle_key(&mut app, key(KeyCode::Char('9')));
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.settings_form.is_none());
        assert_eq!(app.settings, before);

        // 末尾の「アカウントを追加」まで進んでも範囲外に出ない。
        for _ in 0..200 {
            handle_key(&mut app, key(KeyCode::Down));
        }
        assert!(matches!(items(&app)[app.settings_cursor], Item::AddAccount));
        handle_key(&mut app, key(KeyCode::Enter));
        let text = screen_text(&app);
        assert!(text.contains("アカウントを追加"));
        assert!(text.contains("[x]"), "the TLS checkbox should be drawn");
        handle_key(&mut app, key(KeyCode::Esc));
        assert!(app.settings_form.is_none());

        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
