//! アカウントの作成・削除は「DB行」と「keyring上のパスワード」という2つの
//! ストアにまたがる操作になる。CLIとGUIの両方で必要になるため、
//! ロールバック等の整合性ロジックをここに一箇所にまとめる。
//!
//! 注: ここは自動テストを持たない。実際にOSのkeyring（Windows資格情報
//! マネージャー/secret-service等）に書き込み・削除を行うため、CIや
//! secret-service未設定の開発機（CLAUDE.md記載のWSL等）でテストが
//! 不安定になったり、開発者の実際の資格情報ストアを汚したりする。
//! db::accounts側のCRUD自体は既にテスト済み。

use crate::credentials;
use crate::db::Connection;
use crate::db::accounts::{self, Account, NewAccount};

#[derive(Debug, thiserror::Error)]
pub enum CreateAccountError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Keyring(#[from] keyring::Error),
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

/// アカウント行とkeyring上のパスワードを両方削除する。
/// keyring側の削除失敗（既に無い等）はベストエフォートとして無視する。
pub fn remove(conn: &Connection, account_id: i64) -> rusqlite::Result<()> {
    accounts::delete(conn, account_id)?;
    let _ = credentials::delete_password(account_id);
    Ok(())
}
