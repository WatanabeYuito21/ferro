mod bench;

use std::io::{IsTerminal, Write};

use clap::{Parser, Subcommand};
use ferro_core::account_config::{self, AccountConfig};
use ferro_core::account_setup;
use ferro_core::db::accounts;
use ferro_core::db::labels;
use ferro_core::db::messages;
use ferro_core::label_rules::{LabelRule, RuleActions};
use ferro_core::message_filter::MessageFilter;
use ferro_core::search::SearchIndex;
use ferro_core::{credentials, label_rules, paths, reindex, settings, sync};

#[derive(Parser)]
#[command(name = "ferro", version, about = "1000万件規模でも高速に動くPOP3メーラー(CLI)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// アカウント管理
    Account {
        #[command(subcommand)]
        action: AccountAction,
    },
    /// アカウントを同期し、新着メールをMaildir/DBに取り込む
    Sync {
        account_id: i64,
        /// この呼び出しで新規に取得する件数の上限（省略時は無制限）
        #[arg(long)]
        limit: Option<u32>,
        /// アカウントがuse_tls=falseの場合に限り、平文接続を明示的に許可する
        #[arg(long)]
        allow_plaintext: bool,
    },
    /// 保存済みメッセージを新着順に一覧表示する
    List {
        /// 指定しなければ全アカウント横断で一覧表示する
        #[arg(long)]
        account: Option<i64>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
        /// このUnixエポック秒より古いメッセージから表示する（次ページ用カーソル）
        #[arg(long)]
        before: Option<i64>,
    },
    /// 全文検索インデックスを検索する（`from:`/`subject:`/`has:attachment`/`is:unread`/
    /// `label:`/`in:`/`after:YYYY-MM-DD`/`before:`等の詳細検索の書式も使える）
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// UIDL非対応アカウントの重複メッセージを検出する。既定は件数を表示するだけで
    /// 何も変更しない。`--apply`を付けると重複をDB/Maildir/検索インデックスから消す
    Dedupe {
        #[arg(long)]
        apply: bool,
    },
    /// 全メッセージから検索インデックスを作り直す（DB/Maildirから再構築可能な派生キャッシュ）
    Reindex,
    /// データベースと検索インデックスを詰めてディスクを空ける（メールを大量に消した後などに。
    /// 一時的に同じくらいの空き容量が要る。GUI/TUIを閉じて実行）
    Optimize,
    /// 開発用: 大量の合成メッセージでSQLite挿入/Tantivy索引/一覧/検索のスループット・
    /// レイテンシを計測する。本番のDB/Maildir/検索インデックスには一切触れず、
    /// $TMPDIR/ferro-bench配下に専用データを生成する
    Bench { count: u64 },
    /// 既読/未読を設定する（--allで受信箱の未読をすべて既読にする）
    Read {
        #[arg(required_unless_present = "all")]
        message_id: Option<i64>,
        /// 指定すると未読に戻す（省略時は既読にする）
        #[arg(long)]
        unread: bool,
        /// 受信箱の未読をすべて既読にする
        #[arg(long, conflicts_with_all = ["message_id", "unread"])]
        all: bool,
    },
    /// フラグを立てる/外す
    Flag {
        message_id: i64,
        /// 指定するとフラグを外す（省略時は立てる）
        #[arg(long)]
        off: bool,
    },
    /// 論理削除する/復元する（POP3サーバー側のDELEとは独立したローカルの削除フラグ）
    Delete {
        message_id: i64,
        /// 指定すると復元する（省略時は削除する）
        #[arg(long)]
        restore: bool,
    },
    /// メッセージを.eml（保存済みの生メールそのまま）として書き出す
    Export {
        message_id: i64,
        /// 書き出し先。省略すると件名から作ったファイル名でカレントディレクトリに書く
        #[arg(long, short)]
        output: Option<std::path::PathBuf>,
    },
    /// ラベル管理（作成/削除はGUI限定。ここでは一覧表示のみ）
    Label {
        #[command(subcommand)]
        action: LabelAction,
    },
    /// 受信時の自動ラベル付けルール（`label_rules.toml`）
    LabelRule {
        #[command(subcommand)]
        action: LabelRuleAction,
    },
}

#[derive(Subcommand)]
enum LabelAction {
    /// ラベル一覧を表示する（付与件数付き）
    List,
}

#[derive(Subcommand)]
enum LabelRuleAction {
    /// ルールを1件追加する。From/Subject/Bodyは省略可能（すべて省略した条件は
    /// 何にもマッチしない）。指定した条件はAND（すべて満たした場合のみ付与）。
    /// --labelと--mark-read/--archive/--star/--skip-notificationのどれか1つ以上が要る
    Add {
        /// 付与するラベルの名前（既存のラベル名。`ferro label list`で確認できる）
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        body: Option<String>,
        /// true=添付ありのみ、false=添付なしのみ、省略時は問わない
        #[arg(long)]
        has_attachment: Option<bool>,
        /// 既読にする（既読のメールは新着通知も出ない）
        #[arg(long)]
        mark_read: bool,
        /// アーカイブする（受信箱に出さない）
        #[arg(long)]
        archive: bool,
        /// スターを付ける
        #[arg(long)]
        star: bool,
        /// 新着のデスクトップ通知を出さない（GUI）
        #[arg(long)]
        skip_notification: bool,
    },
    /// ルール一覧を表示する
    List,
    /// ルールを削除する（`List`で表示される番号を指定）
    Remove { index: usize },
    /// 既存の全メッセージへ遡ってルールを適用する（「今すぐ適用」に相当）
    Apply,
}

#[derive(Subcommand)]
enum AccountAction {
    /// アカウントを追加する。`accounts.toml`にエントリを追記してから反映する
    /// （直接ファイルを編集して追加することもできる）。パスワードは対話プロンプトで
    /// 入力し、OSのkeyringに保存する
    Add {
        #[arg(long)]
        name: String,
        #[arg(long)]
        host: String,
        #[arg(long)]
        port: u16,
        #[arg(long)]
        username: String,
        /// ポート995なら暗黙的TLS、それ以外ならSTLSでアップグレードして接続する
        #[arg(long)]
        use_tls: bool,
    },
    /// アカウント一覧を表示する
    List,
    /// アカウントを削除する（DB行・Maildir・検索インデックス・keyring・
    /// `accounts.toml`のエントリを全て消す。POP3サーバー上のメールには触れない）
    Remove { account_id: i64 },
    /// `accounts.toml`を読み直してDBに反映する（他のどのaccountサブコマンドも
    /// 実行時に自動でこれを行うため、明示的に叩く必要は通常ない）
    ReloadConfig,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    std::fs::create_dir_all(paths::app_data_dir())?;
    let conn = ferro_core::db::open(&paths::db_path())?;
    let search_index = SearchIndex::open_or_create(&paths::search_index_dir())?;

    // accounts.tomlが正の情報源。存在すれば毎回起動時にDBへ反映する（無ければ空扱い）。
    // ファイルに無くなったアカウントは削除しない（`account_config::reconcile`参照）。
    account_config::load_and_reconcile(&conn, &paths::accounts_config_path())?;

    match cli.command {
        Command::Account { action } => run_account_command(&conn, &search_index, action)?,
        Command::Sync {
            account_id,
            limit,
            allow_plaintext,
        } => run_sync_command(&conn, &search_index, account_id, limit, allow_plaintext)?,
        Command::List {
            account,
            limit,
            before,
        } => run_list_command(&conn, account, before, limit)?,
        Command::Search { query, limit } => run_search_command(&conn, &search_index, &query, limit)?,
        Command::Reindex => run_reindex_command(&conn, &search_index)?,
        Command::Optimize => {
            let report = ferro_core::maintenance::optimize(
                &conn,
                &paths::db_path(),
                &paths::maildir_dir(),
                &search_index,
                &paths::search_index_dir(),
            )?;
            let mb = |bytes: u64| bytes as f64 / 1_048_576.0;
            println!(
                "freed {:.1} MB (database {:.1} -> {:.1} MB, mail {:.1} -> {:.1} MB, search index {:.1} -> {:.1} MB).",
                mb(report.freed()),
                mb(report.db_before),
                mb(report.db_after),
                mb(report.mail_before),
                mb(report.mail_after),
                mb(report.index_before),
                mb(report.index_after)
            );
        }
        Command::Dedupe { apply } => run_dedupe_command(&conn, &search_index, apply)?,
        Command::Bench { count } => bench::run(count)?,
        Command::Read { message_id, unread, all } => {
            if all {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64);
                let count = messages::mark_folder_read(&conn, messages::Folder::Inbox, now)?;
                println!("marked {count} message(s) in the inbox as read.");
            } else if let Some(message_id) = message_id {
                messages::set_read(&conn, message_id, !unread)?;
                println!("message #{message_id} marked as {}.", if unread { "unread" } else { "read" });
            }
        }
        Command::Flag { message_id, off } => {
            messages::set_flagged(&conn, message_id, !off)?;
            println!("message #{message_id} flag {}.", if off { "removed" } else { "set" });
        }
        Command::Delete { message_id, restore } => {
            ferro_core::message_actions::set_deleted(&conn, &search_index, message_id, !restore)?;
            println!("message #{message_id} {}.", if restore { "restored" } else { "deleted" });
        }
        Command::Export { message_id, output } => {
            let message = messages::get(&conn, message_id)?
                .ok_or_else(|| anyhow::anyhow!("message #{message_id} not found"))?;
            let raw = ferro_core::maildir::load(&paths::maildir_dir(), message.account_id, &message.uidl)?;
            let output = output.unwrap_or_else(|| {
                ferro_core::mail::export::suggested_eml_filename(message.subject.as_deref(), message.id).into()
            });
            std::fs::write(&output, raw)?;
            println!("message #{message_id} exported to {}.", output.display());
        }
        Command::Label { action } => run_label_command(&conn, action)?,
        Command::LabelRule { action } => run_label_rule_command(&conn, action)?,
    }

    Ok(())
}

fn run_label_command(conn: &ferro_core::db::Connection, action: LabelAction) -> anyhow::Result<()> {
    match action {
        LabelAction::List => {
            for label in labels::list_with_counts(conn)? {
                println!("{:<20} {:<10} {}件", label.label.name, label.label.color, label.count);
            }
        }
    }
    Ok(())
}

fn run_label_rule_command(conn: &ferro_core::db::Connection, action: LabelRuleAction) -> anyhow::Result<()> {
    let config_path = paths::label_rules_config_path();
    match action {
        LabelRuleAction::Add { label, from, subject, body, has_attachment, mark_read, archive, star, skip_notification } => {
            let filter = MessageFilter {
                from_contains: from,
                subject_contains: subject,
                body_contains: body,
                has_attachment,
            };
            if filter.is_empty() {
                anyhow::bail!("at least one of --from/--subject/--body/--has-attachment is required");
            }
            let actions = RuleActions { mark_read, archive, star, skip_notification };
            label_rules::add(&config_path, LabelRule { label_name: label.unwrap_or_default(), filter, actions })?;
            println!("rule added.");
        }
        LabelRuleAction::List => {
            for (i, rule) in label_rules::load(&config_path)?.rules.iter().enumerate() {
                println!("[{i}] label={:?} {:?} {:?}", rule.label_name, rule.filter, rule.actions);
            }
        }
        LabelRuleAction::Remove { index } => {
            label_rules::remove(&config_path, index)?;
            println!("rule #{index} removed.");
        }
        LabelRuleAction::Apply => {
            let rules = label_rules::load(&config_path)?;
            let applied = label_rules::apply_all(conn, &paths::maildir_dir(), &rules)?;
            println!("rules changed {applied} existing message(s).");
        }
    }
    Ok(())
}

fn run_account_command(
    conn: &ferro_core::db::Connection,
    search_index: &SearchIndex,
    action: AccountAction,
) -> anyhow::Result<()> {
    match action {
        AccountAction::Add {
            name,
            host,
            port,
            username,
            use_tls,
        } => {
            let password = rpassword::prompt_password("Password: ")?;
            let new_account = AccountConfig {
                name,
                host,
                port,
                username,
                use_tls,
            };
            let account = account_setup::add(conn, &paths::accounts_config_path(), &new_account, &password)
                .map_err(|e| match e {
                    account_setup::AddAccountError::Keyring(inner) => anyhow::anyhow!(
                        "failed to save the password to the OS keyring: {inner}\n\
                         account was not created. See CLAUDE.md's keyring troubleshooting notes."
                    ),
                    other => anyhow::anyhow!(other),
                })?;

            println!("account #{} ({}) created.", account.id, account.name);
        }
        AccountAction::List => {
            let statuses = ferro_core::db::sync_status::list(conn)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            for account in accounts::list(conn)? {
                println!(
                    "#{:<4} {:<20} {}@{}:{} (use_tls={}{})",
                    account.id,
                    account.name,
                    account.username,
                    account.host,
                    account.port,
                    account.use_tls,
                    if account.uidl_supported == Some(false) { ", UIDL非対応" } else { "" }
                );
                println!(
                    "      {}",
                    ferro_core::db::sync_status::describe(statuses.get(&account.id), now)
                );
            }
        }
        AccountAction::Remove { account_id } => {
            account_setup::remove(
                conn,
                &paths::maildir_dir(),
                search_index,
                &paths::accounts_config_path(),
                account_id,
            )?;
            println!("account #{account_id} removed.");
        }
        AccountAction::ReloadConfig => {
            let path = paths::accounts_config_path();
            let accounts = account_config::load_and_reconcile(conn, &path)?;
            println!(
                "reloaded {} account(s) from {}.",
                accounts.len(),
                path.display()
            );
        }
    }
    Ok(())
}

fn run_sync_command(
    conn: &ferro_core::db::Connection,
    search_index: &SearchIndex,
    account_id: i64,
    limit: Option<u32>,
    allow_plaintext: bool,
) -> anyhow::Result<()> {
    let account = accounts::get(conn, account_id)?
        .ok_or_else(|| anyhow::anyhow!("account #{account_id} not found"))?;
    let password = credentials::get_password(account_id).map_err(|e| {
        anyhow::anyhow!(
            "failed to read the password from the OS keyring: {e}\n\
             run `ferro account add` again, or see CLAUDE.md's keyring troubleshooting notes."
        )
    })?;

    let label_rules = label_rules::load_or_default(&paths::label_rules_config_path());
    let summary = sync::sync_account_with_limit(
        conn,
        &paths::maildir_dir(),
        &account,
        &password,
        allow_plaintext,
        limit,
        search_index,
        &label_rules,
        settings::load_or_default(&paths::settings_config_path()).server_delete_cutoff(),
        |fetched, total| {
            print!("\rfetched {fetched}/{total}...");
            let _ = std::io::stdout().flush();
        },
    )?;
    println!();

    println!(
        "fetched {} message(s), {} remaining{}",
        summary.fetched,
        summary.remaining,
        if summary.ended_early {
            " (stopped early after repeated disconnects; re-run to continue)"
        } else {
            ""
        }
    );
    Ok(())
}

fn run_search_command(
    conn: &ferro_core::db::Connection,
    search_index: &SearchIndex,
    query: &str,
    limit: usize,
) -> anyhow::Result<()> {
    // `after:`/`before:`の日付はUTCの0時で区切る（CLIは利用者のタイムゾーンを知らない）。
    for id in ferro_core::message_search::search_query(conn, search_index, query, 0, limit)? {
        let Some(message) = messages::get(conn, id)? else {
            continue;
        };
        println!("{}", format_message_line(&message));
    }
    Ok(())
}

fn format_message_line(message: &messages::Message) -> String {
    format!(
        "#{:<6} {} {} {:<30} {:<40} {}",
        message.id,
        if message.is_read { " " } else { "N" },
        if message.is_flagged { "*" } else { " " },
        message.from_addr.as_deref().unwrap_or("(unknown sender)"),
        message.subject.as_deref().unwrap_or("(no subject)"),
        message.date_header,
    )
}

fn run_dedupe_command(
    conn: &ferro_core::db::Connection,
    search_index: &SearchIndex,
    apply: bool,
) -> anyhow::Result<()> {
    let groups = ferro_core::dedupe::find_duplicates(conn)?;
    let removable: usize = groups.iter().map(|g| g.remove_ids.len()).sum();
    println!(
        "{} duplicate group(s), {removable} row(s) would be removed (UIDL-unsupported accounts only).",
        groups.len()
    );
    if !apply {
        println!("dry run: nothing was changed. re-run with --apply to remove them.");
        return Ok(());
    }
    let summary =
        ferro_core::dedupe::remove_duplicates(conn, &paths::maildir_dir(), search_index, &groups)?;
    println!(
        "removed {} duplicate message(s) in {} group(s); {} skipped (run again to retry).",
        summary.removed, summary.groups, summary.skipped
    );
    Ok(())
}

fn run_reindex_command(
    conn: &ferro_core::db::Connection,
    search_index: &SearchIndex,
) -> anyhow::Result<()> {
    // 進捗は端末に出しているときだけ同じ行を上書きして表示する
    // （リダイレクト先のログに\r混じりの行を大量に残さないため）。
    let show_progress = std::io::stderr().is_terminal();
    let count = reindex::reindex_all_with_progress(conn, &paths::maildir_dir(), search_index, |p| {
        if show_progress {
            eprint!("\rreindexing... {}/{}", p.processed, p.total);
        }
    })?;
    if show_progress {
        eprintln!();
    }
    println!("reindexed {count} message(s).");
    Ok(())
}

fn run_list_command(
    conn: &ferro_core::db::Connection,
    account: Option<i64>,
    before: Option<i64>,
    limit: u32,
) -> anyhow::Result<()> {
    for message in messages::list_recent(conn, account, before, limit)? {
        println!("{}", format_message_line(&message));
    }
    Ok(())
}
