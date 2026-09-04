//! アカウントの作成・削除は「設定ファイル」「DB行」「keyring上のパスワード」という
//! 複数のストアにまたがる操作になる。CLIとGUIの両方で必要になるため、
//! ロールバック等の整合性ロジックをここに一箇所にまとめる。
//!
//! 注: ここは自動テストを持たない。実際にOSのkeyring（Windows資格情報
//! マネージャー/secret-service等）に書き込み・削除を行うため、CIや
//! secret-service未設定の開発機（CLAUDE.md記載のWSL等）でテストが
//! 不安定になったり、開発者の実際の資格情報ストアを汚したりする。
//! db::accounts/account_config側のCRUD自体は既にテスト済み。

use std::path::Path;

use crate::account_config::{self, AccountConfig, AccountConfigError, AccountsFile};
use crate::credentials;
use crate::db::Connection;
use crate::db::accounts::{self, Account};
use crate::db::messages;
use crate::maildir;
use crate::search::{SearchError, SearchIndex};

#[derive(Debug, thiserror::Error)]
pub enum AddAccountError {
    #[error(transparent)]
    Config(#[from] AccountConfigError),
    #[error(transparent)]
    Keyring(#[from] keyring::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RemoveAccountError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Search(#[from] SearchError),
}

/// アカウントを追加する: `accounts.toml`にエントリを追記して保存し、DBに反映
/// （`account_config::reconcile`）した上で、パスワードをOS keyringに保存する。
///
/// 設定ファイルの反映やkeyring保存に失敗した場合は、それまでに行った変更
/// （ファイルへの追記・DB行）をロールバックする。これをしないと、認証情報のない
/// 中途半端な状態のアカウントが残ってしまう（過去に実際に踏んだ罠。
/// CLAUDE.mdの開発時トラブルシューティング参照。設定ファイル導入後もこの
/// 原則は変わらない）。
pub fn add(
    conn: &Connection,
    config_path: &Path,
    new: &AccountConfig,
    password: &str,
) -> Result<Account, AddAccountError> {
    let mut file = account_config::load(config_path)?;
    if file.accounts.iter().any(|a| a.name == new.name) {
        return Err(AccountConfigError::DuplicateName(new.name.clone()).into());
    }
    file.accounts.push(new.clone());
    account_config::save(config_path, &file)?;

    let reconciled = match account_config::reconcile(conn, &file) {
        Ok(accounts) => accounts,
        Err(e) => {
            revert_file_entry(config_path, &mut file, &new.name);
            return Err(e.into());
        }
    };
    let account = reconciled
        .into_iter()
        .find(|a| a.name == new.name)
        .expect("just-added account must be present in the reconciled result");

    if let Err(e) = credentials::set_password(account.id, password) {
        let _ = accounts::delete(conn, account.id);
        revert_file_entry(config_path, &mut file, &new.name);
        return Err(e.into());
    }

    Ok(account)
}

fn revert_file_entry(config_path: &Path, file: &mut AccountsFile, name: &str) {
    file.accounts.retain(|a| a.name != name);
    let _ = account_config::save(config_path, file);
}

/// アカウント行、そのアカウントの全メッセージ（DB行・Maildirファイル・検索インデックス）、
/// keyring上のパスワード、設定ファイル上のエントリをまとめて削除する。
///
/// `messages.account_id`は`accounts.id`への外部キー参照だが`ON DELETE CASCADE`は
/// 付けていないため、メッセージが1件でも残っていると`accounts::delete`は
/// 外部キー制約違反で失敗する（`db::accounts`のテスト参照）。そのため必ず
/// メッセージ側を先に消してからアカウント行を消す。
///
/// 設定ファイルからも消しておかないと、次回`account_config::reconcile`した際に
/// 「ファイルにはあるがDBに無い」＝新規アカウントとして復活してしまう
/// （しかもパスワード無し・メッセージ履歴無しの状態で）。
///
/// keyring・設定ファイルの削除失敗（既に無い等）はベストエフォートとして無視する。
pub fn remove(
    conn: &Connection,
    maildir_base: &Path,
    search_index: &SearchIndex,
    config_path: &Path,
    account_id: i64,
) -> Result<(), RemoveAccountError> {
    let account = accounts::get(conn, account_id)?;

    let account_messages = messages::list_all_for_account(conn, account_id)?;
    for message in &account_messages {
        // Maildirファイルが既に無い場合等はベストエフォートで無視する
        // （DB/検索インデックスからの削除ほど致命的ではないため）。
        let _ = maildir::remove(maildir_base, account_id, &message.uidl);
        search_index.delete_message(message.id)?;
    }
    if !account_messages.is_empty() {
        search_index.commit()?;
    }

    messages::delete_all_for_account(conn, account_id)?;
    accounts::delete(conn, account_id)?;
    let _ = credentials::delete_password(account_id);

    if let Some(account) = account
        && let Ok(mut file) = account_config::load(config_path)
    {
        revert_file_entry(config_path, &mut file, &account.name);
    }

    Ok(())
}
