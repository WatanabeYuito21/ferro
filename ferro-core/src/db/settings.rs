//! UI設定（一覧のプレビュー行/差出人アイコン表示/既読にするまでの遅延/
//! バックグラウンド自動同期の間隔）。Ferroは受信専用なので「送信の取り消し」
//! 「画像を自動で読み込む」（本文はプレーンテキストのみ表示する設計のため
//! 画像読み込み自体が無い）は対象外。

use rusqlite::{Connection, params};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub show_preview_line: bool,
    pub show_sender_avatar: bool,
    pub mark_read_delay: bool,
    /// バックグラウンド自動同期の間隔（分）。GUIの`spawn_background_sync`が
    /// 各サイクルの先頭でこれを読み直すので、変更は次回サイクルから反映される
    /// （実行中のsleepを割り込んで即座に反映するような複雑な仕組みは持たない）。
    pub sync_interval_minutes: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_preview_line: true,
            show_sender_avatar: false,
            mark_read_delay: true,
            sync_interval_minutes: 5,
        }
    }
}

const KEY_PREVIEW: &str = "show_preview_line";
const KEY_AVATAR: &str = "show_sender_avatar";
const KEY_READ_DELAY: &str = "mark_read_delay";
const KEY_SYNC_INTERVAL: &str = "sync_interval_minutes";

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
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![KEY_SYNC_INTERVAL, settings.sync_interval_minutes.to_string()],
    )?;
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
}
