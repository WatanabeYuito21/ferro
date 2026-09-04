//! アカウント設定（パスワードを除く）をTOMLファイルで管理する。
//!
//! `accounts.toml`が正の情報源。DBの`accounts`テーブルはこれを反映した結果として
//! 扱い、`reconcile`で同期する。名前(`name`)をキーに既存アカウントと照合し、
//! 見つかれば接続設定を上書き（内部idは維持し、keyring/messages/Maildirとの
//! 紐付けを保つ）、見つからなければ新規作成する。
//!
//! ファイルから無くなったアカウントは自動削除しない。削除するとそのアカウントの
//! メッセージも一緒に消える操作（`account_setup::remove`参照）なので、
//! ファイルを編集しただけで意図せずメールを失うことがないようにするため。
//! 明示的に`ferro account remove`することで削除する。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::db::Connection;
use crate::db::accounts::{self, Account, NewAccount};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AccountConfig {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// 明示的な選択を必須にするため、デフォルト値は用意していない
    /// （省略した場合はパースエラーになる）。
    pub use_tls: bool,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct AccountsFile {
    #[serde(rename = "account", default)]
    pub accounts: Vec<AccountConfig>,
}

#[derive(Debug, thiserror::Error)]
pub enum AccountConfigError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: std::path::PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error(transparent)]
    Serialize(#[from] toml::ser::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error("duplicate account name in config: {0}")]
    DuplicateName(String),
}

/// 設定ファイルを読み込む。存在しない場合は空扱い（初回起動でファイルがまだ
/// 無いのは正常な状態なので、エラーにはしない）。
pub fn load(path: &Path) -> Result<AccountsFile, AccountConfigError> {
    if !path.exists() {
        return Ok(AccountsFile::default());
    }
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|source| AccountConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

pub fn save(path: &Path, file: &AccountsFile) -> Result<(), AccountConfigError> {
    let text = toml::to_string_pretty(file)?;
    fs::write(path, text)?;
    Ok(())
}

/// 設定ファイルの内容をDBに反映する。反映後の全アカウントを返す。
pub fn reconcile(conn: &Connection, file: &AccountsFile) -> Result<Vec<Account>, AccountConfigError> {
    let mut seen_names = std::collections::HashSet::new();
    for entry in &file.accounts {
        if !seen_names.insert(entry.name.as_str()) {
            return Err(AccountConfigError::DuplicateName(entry.name.clone()));
        }
    }

    let mut result = Vec::with_capacity(file.accounts.len());
    for entry in &file.accounts {
        let account = match accounts::find_by_name(conn, &entry.name)? {
            Some(existing) => {
                accounts::update_settings(
                    conn,
                    existing.id,
                    &entry.host,
                    entry.port,
                    &entry.username,
                    entry.use_tls,
                )?;
                accounts::get(conn, existing.id)?.expect("just-updated account must exist")
            }
            None => {
                let id = accounts::insert(
                    conn,
                    &NewAccount {
                        name: &entry.name,
                        host: &entry.host,
                        port: entry.port,
                        username: &entry.username,
                        use_tls: entry.use_tls,
                    },
                )?;
                accounts::get(conn, id)?.expect("just-inserted account must exist")
            }
        };
        result.push(account);
    }
    Ok(result)
}

/// 設定ファイルを読み込んでからDBに反映する、`load`+`reconcile`のショートカット。
pub fn load_and_reconcile(conn: &Connection, path: &Path) -> Result<Vec<Account>, AccountConfigError> {
    let file = load(path)?;
    reconcile(conn, &file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use tempfile::tempdir;

    fn entry(name: &str, host: &str) -> AccountConfig {
        AccountConfig {
            name: name.to_string(),
            host: host.to_string(),
            port: 995,
            username: "user".to_string(),
            use_tls: true,
        }
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("accounts.toml");
        let file = load(&path).unwrap();
        assert!(file.accounts.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("accounts.toml");
        let file = AccountsFile {
            accounts: vec![entry("Work", "pop.example.com"), entry("Home", "pop.other.com")],
        };
        save(&path, &file).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, file);
    }

    #[test]
    fn reconcile_inserts_new_accounts() {
        let conn = open_in_memory().unwrap();
        let file = AccountsFile {
            accounts: vec![entry("Work", "pop.example.com")],
        };
        let result = reconcile(&conn, &file).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Work");
        assert_eq!(result[0].host, "pop.example.com");
    }

    #[test]
    fn reconcile_updates_existing_account_by_name_and_keeps_its_id() {
        let conn = open_in_memory().unwrap();
        let first = reconcile(
            &conn,
            &AccountsFile {
                accounts: vec![entry("Work", "old-host.example.com")],
            },
        )
        .unwrap();
        let original_id = first[0].id;

        let second = reconcile(
            &conn,
            &AccountsFile {
                accounts: vec![entry("Work", "new-host.example.com")],
            },
        )
        .unwrap();

        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id, original_id, "id must be preserved across reconcile");
        assert_eq!(second[0].host, "new-host.example.com");
        // 上書き後も1行しか無いこと（新規作成されて重複していないこと）を確認する。
        assert_eq!(accounts::list(&conn).unwrap().len(), 1);
    }

    #[test]
    fn reconcile_does_not_remove_accounts_missing_from_the_file() {
        let conn = open_in_memory().unwrap();
        reconcile(
            &conn,
            &AccountsFile {
                accounts: vec![entry("Work", "pop.example.com")],
            },
        )
        .unwrap();

        // 空のファイルに変わっても、既存のDB行は消えない
        // （削除はファイル編集ではなく`ferro account remove`で明示的に行う）。
        let result = reconcile(&conn, &AccountsFile::default()).unwrap();
        assert!(result.is_empty(), "reconcile only returns accounts from the file");
        assert_eq!(
            accounts::list(&conn).unwrap().len(),
            1,
            "existing account must not be deleted just because it's missing from the file"
        );
    }

    #[test]
    fn reconcile_rejects_duplicate_names_in_the_file() {
        let conn = open_in_memory().unwrap();
        let file = AccountsFile {
            accounts: vec![
                entry("Work", "pop.example.com"),
                entry("Work", "other.example.com"),
            ],
        };
        assert!(reconcile(&conn, &file).is_err());
    }
}
