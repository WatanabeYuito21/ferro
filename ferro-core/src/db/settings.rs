//! UI設定（一覧のプレビュー行/差出人アイコン表示/既読にするまでの遅延/
//! バックグラウンド自動同期の間隔）。Ferroは受信専用なので「送信の取り消し」
//! 「画像を自動で読み込む」（本文はプレーンテキストのみ表示する設計のため
//! 画像読み込み自体が無い）は対象外。

use rusqlite::{Connection, params};

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// "noto-sans" | "yu-gothic" | "meiryo" | "monospace"。
    pub font_family: String,
    /// "small" | "medium" | "large"。
    pub font_size: String,
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
            font_family: "noto-sans".to_string(),
            font_size: "medium".to_string(),
        }
    }
}

const KEY_PREVIEW: &str = "show_preview_line";
const KEY_AVATAR: &str = "show_sender_avatar";
const KEY_READ_DELAY: &str = "mark_read_delay";
const KEY_SYNC_INTERVAL: &str = "sync_interval_minutes";
const KEY_THEME: &str = "theme";
const KEY_ACCENT_COLOR: &str = "accent_color";
const KEY_FONT_FAMILY: &str = "font_family";
const KEY_FONT_SIZE: &str = "font_size";

const VALID_THEMES: [&str; 3] = ["light", "dark", "system"];
const VALID_FONT_FAMILIES: [&str; 4] = ["noto-sans", "yu-gothic", "meiryo", "monospace"];
const VALID_FONT_SIZES: [&str; 3] = ["small", "medium", "large"];

pub fn get(conn: &Connection) -> rusqlite::Result<Settings> {
    let mut settings = Settings::default();
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (key, value) = row?;
        match key.as_str() {
            KEY_PREVIEW => settings.show_preview_line = value == "1",
            KEY_AVATAR => settings.show_sender_avatar = value == "1",
            KEY_READ_DELAY => settings.mark_read_delay = value == "1",
            KEY_SYNC_INTERVAL => {
                // パース失敗・0以下は無視してデフォルトのまま
                // （設定ファイル相当のものが壊れていても起動を妨げないため）。
                if let Ok(minutes) = value.parse::<i64>()
                    && minutes > 0
                {
                    settings.sync_interval_minutes = minutes;
                }
            }
            KEY_THEME => {
                if VALID_THEMES.contains(&value.as_str()) {
                    settings.theme = value;
                }
            }
            KEY_ACCENT_COLOR => {
                // CSSの色として妥当かはフロント側に委ねる（不正な値でもCSS側で
                // 無視されるだけで実害が無いため）。空文字だけは弾く。
                if !value.is_empty() {
                    settings.accent_color = value;
                }
            }
            KEY_FONT_FAMILY => {
                if VALID_FONT_FAMILIES.contains(&value.as_str()) {
                    settings.font_family = value;
                }
            }
            KEY_FONT_SIZE => {
                if VALID_FONT_SIZES.contains(&value.as_str()) {
                    settings.font_size = value;
                }
            }
            _ => {}
        }
    }
    Ok(settings)
}

pub fn set(conn: &Connection, settings: &Settings) -> rusqlite::Result<()> {
    let bool_entries = [
        (KEY_PREVIEW, settings.show_preview_line),
        (KEY_AVATAR, settings.show_sender_avatar),
        (KEY_READ_DELAY, settings.mark_read_delay),
    ];
    for (key, value) in bool_entries {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, if value { "1" } else { "0" }],
        )?;
    }
    let string_entries = [
        (KEY_SYNC_INTERVAL, settings.sync_interval_minutes.to_string()),
        (KEY_THEME, settings.theme.clone()),
        (KEY_ACCENT_COLOR, settings.accent_color.clone()),
        (KEY_FONT_FAMILY, settings.font_family.clone()),
        (KEY_FONT_SIZE, settings.font_size.clone()),
    ];
    for (key, value) in string_entries {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_in_memory;

    #[test]
    fn get_returns_defaults_when_unset() {
        let conn = open_in_memory().unwrap();
        assert_eq!(get(&conn).unwrap(), Settings::default());
    }

    #[test]
    fn set_then_get_roundtrips() {
        let conn = open_in_memory().unwrap();
        let custom = Settings {
            show_preview_line: false,
            show_sender_avatar: true,
            mark_read_delay: false,
            sync_interval_minutes: 15,
            theme: "dark".to_string(),
            accent_color: "#2255aa".to_string(),
            font_family: "monospace".to_string(),
            font_size: "large".to_string(),
        };
        set(&conn, &custom).unwrap();
        assert_eq!(get(&conn).unwrap(), custom);

        // 2回目のsetは既存行を上書きする(INSERTの一意制約違反にならない)。
        set(&conn, &Settings::default()).unwrap();
        assert_eq!(get(&conn).unwrap(), Settings::default());
    }

    #[test]
    fn invalid_sync_interval_falls_back_to_default() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('sync_interval_minutes', 'not-a-number')",
            [],
        )
        .unwrap();
        assert_eq!(get(&conn).unwrap().sync_interval_minutes, 5);
    }

    #[test]
    fn invalid_theme_and_font_values_fall_back_to_defaults() {
        let conn = open_in_memory().unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('theme', 'not-a-theme')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('font_family', 'comic-sans')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('font_size', 'huge')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO settings (key, value) VALUES ('accent_color', '')", [])
            .unwrap();

        let settings = get(&conn).unwrap();
        assert_eq!(settings.theme, Settings::default().theme);
        assert_eq!(settings.font_family, Settings::default().font_family);
        assert_eq!(settings.font_size, Settings::default().font_size);
        assert_eq!(settings.accent_color, Settings::default().accent_color);
    }
}
