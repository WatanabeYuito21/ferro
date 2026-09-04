use clap::{Parser, Subcommand};
use ferro_core::db::accounts::{self, NewAccount};
use ferro_core::db::messages;
use ferro_core::{credentials, paths, sync};

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

    match cli.command {
        Command::Account { action } => run_account_command(&conn, action)?,
        Command::Sync {
            account_id,
            limit,
            allow_plaintext,
        } => run_sync_command(&conn, account_id, limit, allow_plaintext)?,
        Command::List {
            account,
            limit,
            before,
        } => run_list_command(&conn, account, before, limit)?,
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
            let id = accounts::insert(
                conn,
                &NewAccount {
                    name: &name,
                    host: &host,
                    port,
                    username: &username,
                    use_tls,
                },
            )?;

            let password = rpassword::prompt_password("Password: ")?;
            if let Err(e) = credentials::set_password(id, &password) {
                // keyring保存に失敗した状態でアカウント行だけ残ると、認証情報のない
                // 壊れたレコードになる(過去に実際に踏んだ罠)。失敗時はロールバックする。
                let _ = accounts::delete(conn, id);
                anyhow::bail!(
                    "failed to save the password to the OS keyring: {e}\n\
                     account was not created. See CLAUDE.md's keyring troubleshooting notes."
                );
            }

            println!("account #{id} ({name}) created.");
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
            accounts::delete(conn, account_id)?;
            let _ = credentials::delete_password(account_id);
            println!("account #{account_id} removed.");
        }
    }
    Ok(())
}

fn run_sync_command(
    conn: &ferro_core::db::Connection,
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
