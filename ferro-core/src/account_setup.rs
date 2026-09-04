//! アカウントの作成・削除は「DB行」と「keyring上のパスワード」という2つの
//! ストアにまたがる操作になる。CLIとGUIの両方で必要になるため、
//! ロールバック等の整合性ロジックをここに一箇所にまとめる。
//!
//! 注: ここは自動テストを持たない。実際にOSのkeyring（Windows資格情報
//! マネージャー/secret-service等）に書き込み・削除を行うため、CIや
//! secret-service未設定の開発機（CLAUDE.md記載のWSL等）でテストが
//! 不安定になったり、開発者の実際の資格情報ストアを汚したりする。
//! db::accounts側のCRUD自体は既にテスト済み。

use std::path::Path;

use crate::credentials;
use crate::db::Connection;
use crate::db::accounts::{self, Account, NewAccount};
use crate::db::messages;
use crate::maildir;
use crate::search::{SearchError, SearchIndex};

#[derive(Debug, thiserror::Error)]
pub enum CreateAccountError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
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

/// アカウントを作成し、パスワードをOS keyringに保存する。
///
/// keyring保存に失敗した場合はアカウント行をロールバックする。これを
/// しないと、認証情報のない中途半端な状態のアカウントがDBに残ってしまう
/// （過去に実際に踏んだ罠。CLAUDE.mdの開発時トラブルシューティング参照）。
pub fn create(
    conn: &Connection,
    new: &NewAccount,
    password: &str,
) -> Result<Account, CreateAccountError> {
    let id = accounts::insert(conn, new)?;

    if let Err(e) = credentials::set_password(id, password) {
        let _ = accounts::delete(conn, id);
        return Err(e.into());
    }

    Ok(accounts::get(conn, id)?.expect("just-inserted account must exist"))
}

/// アカウント行、そのアカウントの全メッセージ（DB行・Maildirファイル・検索インデックス）、
/// keyring上のパスワードをまとめて削除する。
///
/// `messages.account_id`は`accounts.id`への外部キー参照だが`ON DELETE CASCADE`は
/// 付けていないため、メッセージが1件でも残っていると`accounts::delete`は
/// 外部キー制約違反で失敗する（`db::accounts`のテスト参照）。そのため必ず
/// メッセージ側を先に消してからアカウント行を消す。
///
/// keyring側の削除失敗（既に無い等）はベストエフォートとして無視する。
pub fn remove(
    conn: &Connection,
    maildir_base: &Path,
    search_index: &SearchIndex,
    account_id: i64,
) -> Result<(), RemoveAccountError> {
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
    Ok(())
}
