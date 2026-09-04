use clap::{Parser, Subcommand};
use ferro_core::account_setup;
use ferro_core::db::accounts::{self, NewAccount};
use ferro_core::db::messages;
use ferro_core::search::SearchIndex;
use ferro_core::{credentials, paths, reindex, sync};

#[derive(Parser)]
#[command(name = "ferro", about = "1000万件規模でも高速に動くPOP3メーラー(CLI)")]
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
    /// 全文検索インデックスを検索する
    Search {
        query: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// 全メッセージから検索インデックスを作り直す（DB/Maildirから再構築可能な派生キャッシュ）
    Reindex,
}

#[derive(Subcommand)]
enum AccountAction {
    /// アカウントを追加する。パスワードは対話プロンプトで入力し、OSのkeyringに保存する
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
    /// アカウントを削除する（DB上のアカウント行とkeyring上のパスワードを両方消す）
    Remove { account_id: i64 },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    std::fs::create_dir_all(paths::app_data_dir())?;
    let conn = ferro_core::db::open(&paths::db_path())?;
    let search_index = SearchIndex::open_or_create(&paths::search_index_dir())?;

    match cli.command {
        Command::Account { action } => run_account_command(&conn, action)?,
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
    }

    Ok(())
}

fn run_account_command(conn: &ferro_core::db::Connection, action: AccountAction) -> anyhow::Result<()> {
    match action {
        AccountAction::Add {
            name,
            host,
            port,
            username,
            use_tls,
        } => {
            let password = rpassword::prompt_password("Password: ")?;
            let new_account = NewAccount {
                name: &name,
                host: &host,
                port,
                username: &username,
                use_tls,
            };
            let account = account_setup::create(conn, &new_account, &password).map_err(|e| {
                match e {
                    account_setup::CreateAccountError::Keyring(inner) => anyhow::anyhow!(
                        "failed to save the password to the OS keyring: {inner}\n\
                         account was not created. See CLAUDE.md's keyring troubleshooting notes."
                    ),
                    other => anyhow::anyhow!(other),
                }
            })?;

            println!("account #{} ({}) created.", account.id, account.name);
        }
        AccountAction::List => {
            for account in accounts::list(conn)? {
                println!(
                    "#{:<4} {:<20} {}@{}:{} (use_tls={})",
                    account.id,
                    account.name,
                    account.username,
                    account.host,
                    account.port,
                    account.use_tls
                );
            }
        }
        AccountAction::Remove { account_id } => {
            account_setup::remove(conn, account_id)?;
            println!("account #{account_id} removed.");
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

    let summary = sync::sync_account_with_limit(
        conn,
        &paths::maildir_dir(),
        &account,
        &password,
        allow_plaintext,
        limit,
        search_index,
    )?;

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
    for id in search_index.search(query, limit)? {
        let Some(message) = messages::get(conn, id)? else {
            continue;
        };
        println!(
            "#{:<6} {:<30} {:<40} {}",
            message.id,
            message.from_addr.as_deref().unwrap_or("(unknown sender)"),
            message.subject.as_deref().unwrap_or("(no subject)"),
            message.date_header,
        );
    }
    Ok(())
}

fn run_reindex_command(
    conn: &ferro_core::db::Connection,
    search_index: &SearchIndex,
) -> anyhow::Result<()> {
    let count = reindex::reindex_all(conn, &paths::maildir_dir(), search_index)?;
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
        println!(
            "#{:<6} {:<30} {:<40} {}",
            message.id,
            message.from_addr.as_deref().unwrap_or("(unknown sender)"),
            message.subject.as_deref().unwrap_or("(no subject)"),
            message.date_header,
        );
    }
    Ok(())
}
