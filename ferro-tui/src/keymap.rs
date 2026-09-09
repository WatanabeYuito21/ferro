//! キー入力のディスパッチ。フォーカス中のペイン/画面・検索入力中かどうかに応じて
//! `App`のメソッドを呼び分ける。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_input::backend::crossterm::EventHandler;

use crate::app::{App, Pane, Screen, SidebarSelection, FOLDERS};

pub fn handle_key(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    // 検索の入力中は、Enter/Esc以外は全てテキスト入力として扱う
    // （global quitの'q'等を誤って飲み込まれないよう、これを最優先で分岐する）。
    if app.search_mode {
        match key.code {
            KeyCode::Enter => app.run_search()?,
            KeyCode::Esc => app.cancel_search(),
            _ => {
                app.search_input.handle_event(&crossterm::event::Event::Key(key));
                app.run_search()?;
            }
        }
        return Ok(());
    }

    // スヌーズのプリセット選択待ち（`z`の直後）。
    if app.awaiting_snooze_choice {
        app.awaiting_snooze_choice = false;
        match key.code {
            KeyCode::Char('1') => app.snooze("1d")?,
            KeyCode::Char('3') => app.snooze("3d")?,
            KeyCode::Char('7') => app.snooze("1w")?,
            _ => {}
        }
        return Ok(());
    }

    if app.show_help {
        // ヘルプは何かキーを押せば閉じる。
        app.show_help = false;
        return Ok(());
    }

    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
            return Ok(());
        }
        KeyCode::Char('?') => {
            app.show_help = true;
            return Ok(());
        }
        KeyCode::Char(',') => {
            app.screen = match app.screen {
                Screen::Messages => Screen::Settings,
                Screen::Settings => Screen::Messages,
            };
            return Ok(());
        }
        _ => {}
    }

    match app.screen {
        Screen::Settings => {
            match key.code {
                KeyCode::Esc => app.screen = Screen::Messages,
                KeyCode::Char('R') => app.run_reindex_now(),
                KeyCode::Char('P') => app.run_purge_now(),
                _ => {}
            }
            Ok(())
        }
        Screen::Messages => handle_messages_key(app, key),
    }
}

fn handle_messages_key(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    match key.code {
        KeyCode::Char('/') => {
            app.start_search();
            return Ok(());
        }
        KeyCode::Char('S') => {
            app.sync_now();
            return Ok(());
        }
        KeyCode::Tab => {
            app.focus = match app.focus {
                Pane::Sidebar => Pane::List,
                Pane::List => Pane::Detail,
                Pane::Detail => Pane::Sidebar,
            };
            return Ok(());
        }
        KeyCode::BackTab => {
            app.focus = match app.focus {
                Pane::Sidebar => Pane::Detail,
                Pane::List => Pane::Sidebar,
                Pane::Detail => Pane::List,
            };
            return Ok(());
        }
        _ => {}
    }

    match app.focus {
        Pane::Sidebar => handle_sidebar_key(app, key),
        Pane::List | Pane::Detail => handle_list_or_detail_key(app, key),
    }
}

fn sidebar_len(app: &App) -> usize {
    FOLDERS.len() + app.labels.len()
}

fn sidebar_selection_at(app: &App, index: usize) -> Option<SidebarSelection> {
    if index < FOLDERS.len() {
        Some(SidebarSelection::Folder(FOLDERS[index].0))
    } else {
        app.labels.get(index - FOLDERS.len()).map(|l| SidebarSelection::Label(l.label.id))
    }
}

fn handle_sidebar_key(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if app.sidebar_cursor > 0 {
                app.sidebar_cursor -= 1;
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.sidebar_cursor + 1 < sidebar_len(app) {
                app.sidebar_cursor += 1;
            }
        }
        KeyCode::Enter => {
            if let Some(selection) = sidebar_selection_at(app, app.sidebar_cursor) {
                app.select_sidebar(selection)?;
                app.focus = Pane::List;
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_list_or_detail_key(app: &mut App, key: KeyEvent) -> anyhow::Result<()> {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => app.move_list_cursor(-1)?,
        KeyCode::Down | KeyCode::Char('j') => app.move_list_cursor(1)?,
        KeyCode::Char('r') => {
            if let Some(m) = app.current_list().get(app.list_cursor).cloned() {
                app.set_read(m.id, !m.is_read)?;
            }
        }
        KeyCode::Char('s') => app.toggle_flagged()?,
        KeyCode::Char('a') => app.toggle_archived()?,
        KeyCode::Char('d') => app.toggle_deleted()?,
        KeyCode::Char('z') => app.awaiting_snooze_choice = true,
        _ => {}
    }
    // Ctrl+CはOS標準の割り込みだが、raw modeでは素通りしてこないので
    // 明示的にも拾っておく（一部端末環境向けの保険）。
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.should_quit = true;
    }
    Ok(())
}
