//! UI設定（一覧のプレビュー行/差出人アイコン表示/既読にするまでの遅延/
//! バックグラウンド自動同期の間隔/外観/メール保持日数）をTOMLファイルで管理する。
//!
//! `settings.toml`が正の情報源。`color_rules.toml`と同じく、パスワードや
//! keyring/Maildirとの紐付けが無い純粋な表示・動作設定なので、DBテーブルへの
//! reconcileは不要（GUI/TUIはこのファイルを直接読み書きするだけでよい）。
//! 元はSQLiteの`settings`テーブル（key/value）に保存していたが、
//! `accounts.toml`/`color_rules.toml`と一貫させ、バックアップ・手編集を
//! しやすくするためファイルベースに移行した（`migrate_from_db_once`が
//! 既存ユーザーの設定を初回起動時にこのファイルへ引き継ぐ。
//! `settings`テーブル自体は当面残すが、以後この移行専用コード以外からは
//! 読み書きしない）。
//! Ferroは受信専用なので「送信の取り消し」「画像を自動で読み込む」（本文は
//! プレーンテキストのみ表示する設計のため画像読み込み自体が無い）は対象外。

use std::fs;
use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub show_preview_line: bool,
    pub show_sender_avatar: bool,
    pub mark_read_delay: bool,
    /// バックグラウンド自動同期の間隔（分）。GUIの`spawn_background_sync`が
    /// 各サイクルの先頭でこれを読み直すので、変更は次回サイクルから反映される
    /// （実行中のsleepを割り込んで即座に反映するような複雑な仕組みは持たない）。
    pub sync_interval_minutes: i64,
    /// "light" | "dark" | "system"（システム追従。OSのprefers-color-schemeに従う）。
    /// 見た目の適用自体はフロント側（`appearance.js`）が`<html data-theme>`と
    /// CSSカスタムプロパティを書き換えることで行う。
    pub theme: String,
    /// アクセントカラー（差し色）。CSSの色として解釈できる文字列（例: "#3f6b5c"）。
    /// hoverや薄い背景色はフロント側でCSSの`color-mix()`により動的に導出するため、
    /// ここではベースの1色だけを保持する。
    pub accent_color: String,
    /// フォントファミリー名（例: "Noto Sans JP"、"Yu Gothic"）。GUIの設定画面が
    /// OSにインストール済みのフォント一覧（`list_installed_fonts`コマンド、
    /// `font-kit`crate）から選ばせるため、固定の候補リストではなく任意の文字列。
    /// 存在しない/入力ミスのフォント名が入っていても、フロント側が常に
    /// フォールバック（`'指定名', 'Noto Sans JP', system-ui, sans-serif`）を
    /// 付けて適用するので、CSSが無効になったりクラッシュしたりはしない。
    pub font_family: String,
    /// "small" | "medium" | "large"。
    pub font_size: String,
    /// メールの保持日数。0は「無期限（自動削除しない）」を意味する
    /// （デフォルト。既存ユーザーが不意にメールを失わないよう、明示的に
    /// 設定しない限り何も消さない）。正の値を設定すると、`date_header`が
    /// その日数より前のメッセージを完全に削除する対象にする
    /// （`message_actions::purge_expired_batch`参照。スター付きは例外的に
    /// 保持日数に関わらず対象外）。
    pub retention_days: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_preview_line: true,
            show_sender_avatar: false,
            mark_read_delay: true,
            sync_interval_minutes: 5,
            theme: "light".to_string(),
            accent_color: "#3f6b5c".to_string(),
            font_family: "Noto Sans JP".to_string(),
            font_size: "medium".to_string(),
            retention_days: 0,
        }
    }
}

const VALID_THEMES: [&str; 3] = ["light", "dark", "system"];
const VALID_FONT_SIZES: [&str; 3] = ["small", "medium", "large"];

impl Settings {
    /// TOMLとしては型が妥当でも意味的に不正な値（未知のtheme文字列、負の
    /// retention_days等）を安全な既定値へ補正する。手編集で壊れた値のせいで
    /// 意図せず大量削除が走ったり、フロント側が知らないtheme値を渡されたり
    /// しないようにするための安全策（旧DB実装のkeyごとのフォールバックと同じ
    /// 考え方）。TOMLの構文自体が壊れている場合（型不一致等）は`load`が
    /// `SettingsError::Parse`としてこれより先に弾く。
    fn normalize(&mut self) {
        let default = Settings::default();
        if !VALID_THEMES.contains(&self.theme.as_str()) {
            self.theme = default.theme;
        }
        if !VALID_FONT_SIZES.contains(&self.font_size.as_str()) {
            self.font_size = default.font_size;
        }
        if self.accent_color.trim().is_empty() {
            self.accent_color = default.accent_color;
        }
        if self.font_family.trim().is_empty() {
            self.font_family = default.font_family;
        }
        if self.sync_interval_minutes <= 0 {
            self.sync_interval_minutes = default.sync_interval_minutes;
        }
        if self.retention_days < 0 {
            self.retention_days = default.retention_days;
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
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
}

/// 設定ファイルを読み込む。存在しない場合は既定値を返す（`color_rules::load`と
/// 同じ理由）。
pub fn load(path: &Path) -> Result<Settings, SettingsError> {
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = fs::read_to_string(path)?;
    let mut settings: Settings = toml::from_str(&text).map_err(|source| SettingsError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    settings.normalize();
    Ok(settings)
}

/// 読み込みに失敗しても呼び出し元の処理を止めないためのフォールバック版。
/// バックグラウンド同期・保持期間クリーンアップ等、設定画面のように利用者へ
/// 直接エラーを見せる手段が無い箇所で使う（一時的なファイル破損等で
/// バックグラウンド処理自体が止まってしまわないように）。
pub fn load_or_default(path: &Path) -> Settings {
    load(path).unwrap_or_default()
}

pub fn save(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
    let text = toml::to_string_pretty(settings)?;
    fs::write(path, text)?;
    Ok(())
}

/// 移行専用: ファイルベースに切り替える前の実装（SQLiteの`settings`
/// key/valueテーブル）に保存されていた既存ユーザーの設定を、初回起動時
/// だけ新しい設定ファイルへ引き継ぐ。`path`（設定ファイル）が既に存在する
/// 場合は何もしない（ファイルの存在自体を「移行済み」の目印として使う。
/// `settings`テーブルは当面残すが、以後はこの関数以外から読み書きしない）。
pub fn migrate_from_db_once(conn: &Connection, path: &Path) -> Result<(), SettingsError> {
    if path.exists() {
        return Ok(());
    }
    let mut stmt = match conn.prepare("SELECT key, value FROM settings") {
        Ok(stmt) => stmt,
        // 新規インストール等でテーブル自体が無い場合は移行するものが無い。
        Err(_) => return Ok(()),
    };
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut settings = Settings::default();
    for row in rows {
        let (key, value) = row?;
        match key.as_str() {
            "show_preview_line" => settings.show_preview_line = value == "1",
            "show_sender_avatar" => settings.show_sender_avatar = value == "1",
            "mark_read_delay" => settings.mark_read_delay = value == "1",
            "sync_interval_minutes" => {
                if let Ok(minutes) = value.parse() {
                    settings.sync_interval_minutes = minutes;
                }
            }
            "theme" => settings.theme = value,
            "accent_color" => settings.accent_color = value,
            "font_family" => settings.font_family = value,
            "font_size" => settings.font_size = value,
            "retention_days" => {
                if let Ok(days) = value.parse() {
                    settings.retention_days = days;
                }
            }
            _ => {}
        }
    }

    settings.normalize();
    save(path, &settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;
    use tempfile::tempdir;

    #[test]
    fn load_missing_file_returns_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert_eq!(load(&path).unwrap(), Settings::default());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let custom = Settings {
            show_preview_line: false,
            show_sender_avatar: true,
            mark_read_delay: false,
            sync_interval_minutes: 15,
            theme: "dark".to_string(),
            accent_color: "#2255aa".to_string(),
            font_family: "monospace".to_string(),
            font_size: "large".to_string(),
            retention_days: 90,
        };
        save(&path, &custom).unwrap();
        assert_eq!(load(&path).unwrap(), custom);

        save(&path, &Settings::default()).unwrap();
        assert_eq!(load(&path).unwrap(), Settings::default());
    }

    #[test]
    fn invalid_sync_interval_falls_back_to_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "sync_interval_minutes = -1\n").unwrap();
        assert_eq!(load(&path).unwrap().sync_interval_minutes, 5);
    }

    /// 負の保持日数のせいで意図せず大量削除が走ることが無いよう、
    /// 既定の0(無期限)にフォールバックすることを確認する。
    #[test]
    fn invalid_retention_days_falls_back_to_unlimited() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "retention_days = -1\n").unwrap();
        assert_eq!(load(&path).unwrap().retention_days, 0);
    }

    #[test]
    fn invalid_theme_and_font_size_fall_back_to_defaults() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            "theme = \"not-a-theme\"\nfont_size = \"huge\"\naccent_color = \"\"\nfont_family = \"\"\n",
        )
        .unwrap();

        let settings = load(&path).unwrap();
        assert_eq!(settings.theme, Settings::default().theme);
        assert_eq!(settings.font_size, Settings::default().font_size);
        assert_eq!(settings.accent_color, Settings::default().accent_color);
        assert_eq!(settings.font_family, Settings::default().font_family);
    }

    /// `font_family`は固定候補ではなく、OSにインストールされている任意の
    /// フォント名を受け付ける（`list_installed_fonts`コマンド参照）。
    #[test]
    fn arbitrary_font_family_name_is_accepted() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "font_family = \"Comic Sans MS\"\n").unwrap();
        assert_eq!(load(&path).unwrap().font_family, "Comic Sans MS");
    }

    /// 構文自体が壊れている（型不一致）場合はエラーとして伝播する
    /// （`color_rules::load`と同じ挙動）。
    #[test]
    fn malformed_toml_is_an_error_not_a_silent_default() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "retention_days = \"not-a-number\"\n").unwrap();
        assert!(matches!(load(&path), Err(SettingsError::Parse { .. })));
    }

    #[test]
    fn migrate_from_db_once_carries_over_existing_settings_and_only_runs_once() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('theme', 'dark')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('retention_days', '30')",
            [],
        )
        .unwrap();

        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        migrate_from_db_once(&conn, &path).unwrap();

        let migrated = load(&path).unwrap();
        assert_eq!(migrated.theme, "dark");
        assert_eq!(migrated.retention_days, 30);

        // ファイルが既に存在するので、DB側をその後変更しても2度目は何もしない。
        conn.execute("UPDATE settings SET value = 'light' WHERE key = 'theme'", [])
            .unwrap();
        migrate_from_db_once(&conn, &path).unwrap();
        assert_eq!(load(&path).unwrap().theme, "dark");
    }

    #[test]
    fn migrate_from_db_once_is_a_noop_when_db_has_no_settings_rows() {
        let conn = open_in_memory().unwrap();
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");

        migrate_from_db_once(&conn, &path).unwrap();
        assert_eq!(load(&path).unwrap(), Settings::default());
    }
}
