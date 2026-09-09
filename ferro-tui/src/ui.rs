//! 画面描画。`App`の状態を読むだけで、変更はしない。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::{App, Pane, Screen, FOLDERS};
use crate::colors::{match_color, truncate_to_width};

const FOCUS_BORDER: Color = Color::Yellow;
const NORMAL_BORDER: Color = Color::DarkGray;

fn pane_border_style(active: bool) -> Style {
    Style::default().fg(if active { FOCUS_BORDER } else { NORMAL_BORDER })
}

pub fn render(f: &mut Frame, app: &App) {
    let size = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(size);

    match app.screen {
        Screen::Messages => render_messages_screen(f, app, rows[0]),
        Screen::Settings => render_settings_screen(f, app, rows[0]),
    }
    render_status_bar(f, app, rows[1]);

    if app.show_help {
        render_help_overlay(f, size);
    }
}

fn render_messages_screen(f: &mut Frame, app: &App, area: Rect) {
    let (search_area, body_area) = if app.search_mode {
        let split = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .split(area);
        (Some(split[0]), split[1])
    } else {
        (None, area)
    };

    if let Some(search_area) = search_area {
        render_search_bar(f, app, search_area);
    }

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(26), Constraint::Percentage(45), Constraint::Min(24)])
        .split(body_area);

    render_sidebar(f, app, cols[0]);
    render_list(f, app, cols[1]);
    render_detail(f, app, cols[2]);
}

fn render_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("検索 (Enter:確定 Esc:キャンセル)");
    let text = app.search_input.value();
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn render_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let active = matches!(app.focus, Pane::Sidebar) && app.screen == Screen::Messages;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(pane_border_style(active))
        .title("フォルダ");

    let mut items: Vec<ListItem> = FOLDERS
        .iter()
        .map(|(folder, label)| {
            let count = match folder {
                ferro_core::db::messages::Folder::Inbox => app.folder_counts.inbox,
                ferro_core::db::messages::Folder::Starred => app.folder_counts.starred,
                ferro_core::db::messages::Folder::Snoozed => app.folder_counts.snoozed,
                ferro_core::db::messages::Folder::Archive => app.folder_counts.archive,
                ferro_core::db::messages::Folder::Trash => app.folder_counts.trash,
            };
            let selected = app.selection == crate::app::SidebarSelection::Folder(*folder);
            let marker = if selected { "▶ " } else { "  " };
            ListItem::new(Line::from(format!("{marker}{label} ({count})")))
        })
        .collect();

    if !app.labels.is_empty() {
        items.push(ListItem::new(Line::from("―― ラベル ――")));
        for label_with_count in &app.labels {
            let selected = app.selection == crate::app::SidebarSelection::Label(label_with_count.label.id);
            let marker = if selected { "▶ " } else { "  " };
            let dot_color = crate::colors::parse_hex_color(&label_with_count.label.color).unwrap_or(Color::White);
            items.push(ListItem::new(Line::from(vec![
                Span::raw(marker),
                Span::styled("● ", Style::default().fg(dot_color)),
                Span::raw(format!("{} ({})", label_with_count.label.name, label_with_count.count)),
            ])));
        }
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    if active {
        state.select(Some(cursor_row_for_sidebar(app)));
    }
    f.render_stateful_widget(list, area, &mut state);
}

/// サイドバーの表示行番号（ラベル区切り行がある分だけラベルのインデックスとずれる）。
fn cursor_row_for_sidebar(app: &App) -> usize {
    if app.sidebar_cursor < FOLDERS.len() {
        app.sidebar_cursor
    } else {
        FOLDERS.len() + 1 + (app.sidebar_cursor - FOLDERS.len())
    }
}

fn render_list(f: &mut Frame, app: &App, area: Rect) {
    let active = matches!(app.focus, Pane::List) && app.screen == Screen::Messages;
    let title = if app.search_mode { "検索結果" } else { "メッセージ一覧" };
    let block = Block::default().borders(Borders::ALL).border_style(pane_border_style(active)).title(title);

    let list_items = app.current_list();
    let inner_width = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = list_items
        .iter()
        .map(|m| {
            let color = match_color(
                &app.color_rules,
                m.subject.as_deref(),
                m.from_name.as_deref(),
                m.from_addr.as_deref(),
                m.preview.as_deref(),
            );
            let from = m.from_name.as_deref().or(m.from_addr.as_deref()).unwrap_or("(unknown)");
            let subject = m.subject.as_deref().unwrap_or("(no subject)");
            let date = format_date(m.date_header);
            let flag = if m.is_flagged { "★" } else { " " };
            let line_text = format!("{flag} {date}  {from}  {subject}");
            let line_text = truncate_to_width(&line_text, inner_width);

            let mut style = Style::default();
            if let Some(color) = color {
                style = style.fg(color);
            }
            if !m.is_read {
                style = style.add_modifier(Modifier::BOLD);
            }
            ListItem::new(Line::styled(line_text, style))
        })
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default();
    if !list_items.is_empty() {
        state.select(Some(app.list_cursor));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let active = matches!(app.focus, Pane::Detail) && app.screen == Screen::Messages;
    let block = Block::default().borders(Borders::ALL).border_style(pane_border_style(active)).title("本文");

    let Some(detail) = &app.detail else {
        f.render_widget(Paragraph::new("メッセージを選択してください").block(block), area);
        return;
    };

    let mut lines = Vec::new();
    lines.push(Line::styled(
        detail.message.subject.clone().unwrap_or_else(|| "(no subject)".to_string()),
        Style::default().add_modifier(Modifier::BOLD),
    ));
    let from = detail.message.from_name.as_deref().or(detail.message.from_addr.as_deref()).unwrap_or("(unknown)");
    lines.push(Line::from(format!("From: {from}")));
    if let Some(to) = &detail.message.to_addr {
        lines.push(Line::from(format!("To: {to}")));
    }
    lines.push(Line::from(format!("Date: {}", format_date(detail.message.date_header))));

    if !detail.labels.is_empty() {
        let mut spans = vec![Span::raw("Labels: ")];
        for (i, label) in detail.labels.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(", "));
            }
            let color = crate::colors::parse_hex_color(&label.color).unwrap_or(Color::White);
            spans.push(Span::styled(label.name.clone(), Style::default().fg(color)));
        }
        lines.push(Line::from(spans));
    }

    let mut status_parts = Vec::new();
    if detail.message.is_flagged {
        status_parts.push("★スター付き");
    }
    if detail.message.is_archived {
        status_parts.push("アーカイブ済み");
    }
    if detail.message.snoozed_until.is_some() {
        status_parts.push("スヌーズ中");
    }
    if detail.message.is_deleted {
        status_parts.push("ゴミ箱");
    }
    if !status_parts.is_empty() {
        lines.push(Line::from(status_parts.join(" / ")));
    }

    if !detail.attachments.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::styled(
            format!("添付ファイル ({})", detail.attachments.len()),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        for attachment in &detail.attachments {
            let name = attachment.filename.as_deref().unwrap_or("(unnamed)");
            lines.push(Line::from(format!("  - {name} ({} bytes)", attachment.size)));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from("―――――――――――――――――――――"));
    for body_line in detail.body.as_deref().unwrap_or("(本文なし)").lines() {
        lines.push(Line::from(body_line.to_string()));
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_settings_screen(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("設定（R:検索インデックス再構築 P:今すぐ整理する Esc:戻る。数値/ルール編集は今後対応予定）");
    let mut lines = vec![
        Line::styled(
            format!("Ferro v{}", env!("CARGO_PKG_VERSION")),
            Style::default().add_modifier(Modifier::DIM),
        ),
        Line::from(""),
        Line::styled("表示", Style::default().add_modifier(Modifier::BOLD)),
        Line::from(format!(
            "  一覧のプレビュー行: {}",
            if app.settings.show_preview_line { "ON" } else { "OFF" }
        )),
        Line::from(format!(
            "  既読にするまでの時間: {}",
            if app.settings.mark_read_delay { "ON" } else { "OFF" }
        )),
        Line::from(""),
        Line::styled("同期・保持期間", Style::default().add_modifier(Modifier::BOLD)),
        Line::from(format!("  バックグラウンド自動同期の間隔: {}分", app.settings.sync_interval_minutes)),
        Line::from(format!(
            "  メールの保持日数: {}",
            if app.settings.retention_days > 0 {
                format!("{}日", app.settings.retention_days)
            } else {
                "無期限".to_string()
            }
        )),
        Line::from(""),
        Line::styled("色分けルール", Style::default().add_modifier(Modifier::BOLD)),
    ];
    if app.color_rules.is_empty() {
        lines.push(Line::from("  （なし）"));
    } else {
        for rule in &app.color_rules {
            let color = crate::colors::parse_hex_color(&rule.color).unwrap_or(Color::White);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("● ", Style::default().fg(color)),
                Span::raw(&rule.pattern),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("アカウント", Style::default().add_modifier(Modifier::BOLD)));
    if app.accounts.is_empty() {
        lines.push(Line::from("  （なし）"));
    } else {
        for account in &app.accounts {
            lines.push(Line::from(format!(
                "  {} — {}@{}:{} ({})",
                account.name,
                account.username,
                account.host,
                account.port,
                if account.use_tls { "TLS" } else { "平文" }
            )));
        }
    }

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let text = app
        .status
        .clone()
        .unwrap_or_else(|| "Tab:ペイン切替 j/k:移動 Enter:選択 /:検索 S:同期 r/s/a/d/z:既読/スター/アーカイブ/削除/スヌーズ ,:設定 ?:ヘルプ q:終了".to_string());
    f.render_widget(Paragraph::new(text), area);
}

fn render_help_overlay(f: &mut Frame, area: Rect) {
    let popup = centered_rect(60, 60, area);
    f.render_widget(Clear, popup);
    let lines = vec![
        Line::styled("キー操作", Style::default().add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::from("Tab / Shift+Tab   ペイン切替（フォルダ|一覧|本文）"),
        Line::from("↑/k, ↓/j          カーソル移動"),
        Line::from("Enter             フォルダ/ラベルを選択"),
        Line::from("/                 検索（Enterで確定、Escでキャンセル）"),
        Line::from("r                 既読/未読切替"),
        Line::from("s                 スター切替"),
        Line::from("a                 アーカイブ切替"),
        Line::from("d                 削除（ゴミ箱へ）/復元"),
        Line::from("z → 1/3/7         スヌーズ（1日後/3日後/1週間後）"),
        Line::from("S                 全アカウントを今すぐ同期"),
        Line::from(",                 設定画面の表示切替"),
        Line::from("q                 終了"),
        Line::from(""),
        Line::from("(何かキーを押すと閉じます)"),
    ];
    let block = Block::default().borders(Borders::ALL).title("ヘルプ");
    f.render_widget(Paragraph::new(lines).block(block).alignment(Alignment::Left), popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
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

fn format_date(unix_seconds: i64) -> String {
    // GUIのformatDate.jsと同じ「日付+時刻」表示のRust版。タイムゾーンは
    // ローカル環境に依存しない実装コストを優先し、UTCのまま表示する
    // （chrono/time crateを増やさずstd::time::SystemTimeの範囲で収める）。
    let datetime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix_seconds.max(0) as u64);
    let elapsed = datetime.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let days_since_epoch = elapsed / 86400;
    let secs_of_day = elapsed % 86400;
    let (year, month, day) = civil_from_days(days_since_epoch as i64);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Howard Hinnantの`civil_from_days`アルゴリズム（エポック日数→年月日、UTC）。
/// 依存クレートを増やさずに日付分解するための最小実装。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
