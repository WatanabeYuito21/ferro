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
