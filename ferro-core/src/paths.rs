use std::path::PathBuf;

/// CLIとGUI(Tauri)が共有するアプリデータのルートディレクトリ。
/// Windowsでは`%APPDATA%/ferro`、Linuxでは`$XDG_DATA_HOME/ferro`(通常`~/.local/share/ferro`)、
/// macOSでは`~/Library/Application Support/ferro`になる。
pub fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ferro")
}

pub fn db_path() -> PathBuf {
    app_data_dir().join("ferro.db")
}

pub fn maildir_dir() -> PathBuf {
    app_data_dir().join("maildir")
}

pub fn search_index_dir() -> PathBuf {
    app_data_dir().join("search_index")
}

/// アカウント設定（パスワードを除く: name/host/port/username/use_tls）を保持する
/// TOMLファイル。これが正の情報源で、DBの`accounts`テーブルは起動時等に
/// `account_config::reconcile`でこの内容を反映した結果になる
/// （`ferro_core::account_config`参照）。
pub fn accounts_config_path() -> PathBuf {
    app_data_dir().join("accounts.toml")
}
