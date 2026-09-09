//! FerroのTUI版エントリポイント。`ferro-core`を土台に、GUI(`src-tauri`)と
//! ほぼ同等の機能を3ペイン構成（フォルダ|一覧|本文）のターミナルUIで提供する
//! （CLAUDE.md記載のneomutt的な使い方を想定）。同じDB/Maildir/検索インデックスの
//! パスをCLI/GUIと共有する。

mod app;
mod background;
mod colors;
mod keymap;
mod ui;

use std::io::stdout;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ferro_core::search::SearchIndex;
use ferro_core::{account_config, db, paths, settings};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use app::App;

/// メインループの1周ごとの入力待ちタイムアウト。バックグラウンドスレッドからの
/// 通知（同期進捗等）や既読化タイマーをこの間隔で拾う。
const TICK_INTERVAL: Duration = Duration::from_millis(250);

fn main() -> anyhow::Result<()> {
    // `--version`/`-V`だけの簡易対応。clapを新たに依存に足すほどではないため
    // 素朴に引数を見る（`ferro-cli`の`--version`と同じ流儀を揃えるため）。
    if std::env::args().nth(1).as_deref().is_some_and(|a| a == "--version" || a == "-V") {
        println!("ferro-tui {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    std::fs::create_dir_all(paths::app_data_dir())?;
    let write_conn = db::open(&paths::db_path())?;
    let read_conn = db::open(&paths::db_path())?;
    // accounts.tomlが正の情報源。GUI/CLIと同じく起動時に反映する
    // （手編集ファイルの構文ミス等で失敗してもTUI自体は起動させる）。
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
    let search_index = SearchIndex::open_or_create(&paths::search_index_dir())?;

    // 検索インデックスの文書数が、DBが「投入済み」と認識している件数より
    // 少ない場合の不整合検知・復旧（GUI版`run`と同じ理由。詳細は
    // `ferro_core::db::messages::reset_all_fts_indexed`のドキュメント参照）。
    if let Ok(indexed_count) = db::messages::count_indexed(&write_conn)
        && indexed_count > search_index.num_docs() as i64
    {
        let _ = db::messages::reset_all_fts_indexed(&write_conn);
    }

    let write_conn = Arc::new(Mutex::new(write_conn));
    let read_conn = Arc::new(Mutex::new(read_conn));
    let search_index = Arc::new(search_index);

    let (tx, rx) = mpsc::channel();
    background::spawn(Arc::clone(&write_conn), Arc::clone(&search_index), tx.clone());

    let mut app = App::new(read_conn, write_conn, search_index, rx, tx)?;

    let mut terminal = setup_terminal()?;
    let result = run_event_loop(&mut terminal, &mut app);
    restore_terminal()?;
    result
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<std::io::Stdout>>> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;

    // パニック時も端末をraw mode/代替画面から必ず復元する
    // （ratatuiでの定番の落とし穴。復元しないと終了後の端末が壊れて見える）。
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));

    let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    Ok(terminal)
}

fn restore_terminal() -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn run_event_loop(terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>, app: &mut App) -> anyhow::Result<()> {
    loop {
        terminal.draw(|f| ui::render(f, app))?;

        if event::poll(TICK_INTERVAL)? {
            if let Event::Key(key) = event::read()? {
                // Windowsのcrosstermはキー押下・離上の両方をイベントとして送るため、
                // 押下(Press)だけを処理しないと1回の入力が2回反映されることがある。
                if key.kind == event::KeyEventKind::Press {
                    keymap::handle_key(app, key)?;
                }
            }
        }
        app.tick()?;

        if app.should_quit {
            return Ok(());
        }
    }
}
