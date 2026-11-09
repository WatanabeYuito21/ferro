//! 同期エラー等を追跡するための最小限の診断ログ。
//!
//! 今までFerroは同期エラーをGUIの一時的なトースト通知（閉じたら消える）や
//! CLI/TUIの標準エラー出力にしか出しておらず、外部の利用者から不具合報告を
//! もらっても後から状況を再現できなかった（実際にレンタルサーバー宛の
//! アカウントで"connection closed by server"を踏んだ第三者の報告を、これ以上
//! 診断する手段が無かった）。このモジュールは`paths::log_path()`
//! （`<app_data_dir>/ferro.log`）に人が読める1行ログを追記するだけの
//! 単純な仕組みで、tracing/log crateのような本格的なロギング基盤は
//! 個人利用のデスクトップアプリの規模には過剰と判断し導入していない。
//! ログの書き込み自体が失敗してもアプリの動作に影響させないため、
//! エラーは握りつぶす（呼び出し側でmatch/unwrapする必要がない）。
//!
//! パスワード等の秘匿情報は絶対にログへ書かない。
//!
//! ローテートは`settings.toml`の`log_max_kb`（既定1024KB。0はローテートしない）と
//! `log_keep_files`（既定1世代）で決める。追記の前にファイルが上限以上なら
//! `ferro.log.1`→`ferro.log.2`…と1つずつずらし、`ferro.log`を`ferro.log.1`にしてから
//! 新しく書き始める（`log_keep_files`を超えた古いものは消す）。設定は書き込みのたびに
//! 読み直す（ログは同期のエラー等でたまに出るだけなので負担にならず、CLI/TUI/GUIの
//! どれから書いても、起動し直さなくても同じ設定が効く）。設定ファイルが読めなければ
//! 既定値を使う。CLI/TUI/GUIが同時に書いても毎回open→追記→closeなので、競合しても
//! 1回余分に回すか回し損ねる程度で害は無い。

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::db::now_unix;
use crate::paths::{log_path, settings_config_path};
use crate::settings;

/// ローテートの設定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rotation {
    /// これ以上の大きさになったら回す。0は回さない。
    max_bytes: u64,
    /// 残す古いログの世代数。
    keep_files: u32,
}

impl Rotation {
    fn from_settings(s: &settings::Settings) -> Self {
        Rotation {
            max_bytes: (s.log_max_kb.max(0) as u64).saturating_mul(1024),
            keep_files: s.log_keep_files.clamp(0, settings::MAX_LOG_KEEP_FILES) as u32,
        }
    }
}

/// 診断ログに1行追記する。書き込み失敗（ディスクフルや権限エラー等）は
/// 無視する。
pub fn log_line(line: &str) {
    // テストは失敗経路をわざと踏むので、利用者の本物のログを汚さない。
    if cfg!(test) {
        return;
    }
    let rotation = Rotation::from_settings(&settings::load_or_default(&settings_config_path()));
    let ts = format_utc_datetime(now_unix());
    append_line(&log_path(), &format!("[{ts}] {line}"), rotation);
}

/// `n`世代前のログのファイル名（`ferro.log`→`ferro.log.<n>`）。
fn rotated_path(path: &Path, n: u32) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

/// `path`を`.1`へ回し、既存の`.1`〜は1つずつずらす。`keep_files`を超えるものは消す
/// （設定で世代数を減らした場合に残っている分も含めて）。失敗は無視する。
fn rotate(path: &Path, keep_files: u32) {
    // Windowsのrenameは既存の移動先を置き換える（MoveFileExのREPLACE_EXISTING）。
    for n in (1..keep_files).rev() {
        let from = rotated_path(path, n);
        if from.exists() {
            let _ = std::fs::rename(&from, rotated_path(path, n + 1));
        }
    }
    if keep_files == 0 {
        let _ = std::fs::remove_file(path);
    } else {
        let _ = std::fs::rename(path, rotated_path(path, 1));
    }
    let mut n = keep_files + 1;
    loop {
        let extra = rotated_path(path, n);
        if !extra.exists() {
            break;
        }
        let _ = std::fs::remove_file(extra);
        n += 1;
    }
}

/// `path`が上限以上なら回してから`line`を1行追記する。失敗は無視する
/// （回せなくても追記は試みる）。
fn append_line(path: &Path, line: &str, rotation: Rotation) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if rotation.max_bytes > 0
        && std::fs::metadata(path).is_ok_and(|m| m.len() >= rotation.max_bytes)
    {
        rotate(path, rotation.keep_files);
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

/// `unix_seconds`をUTCの"YYYY-MM-DD HH:MM:SS"に変換する。タイムゾーンは
/// ローカル環境に依存しない実装コストを優先し、UTCのまま出力する
/// （chrono/time crateを増やさずstd::time::SystemTimeの範囲で収める。
/// `ferro-tui`のメッセージ一覧の日時表示と同じ考え方・同じアルゴリズム）。
fn format_utc_datetime(unix_seconds: i64) -> String {
    let secs = unix_seconds.max(0) as u64;
    let days_since_epoch = secs / 86400;
    let secs_of_day = secs % 86400;
    let (year, month, day) = civil_from_days(days_since_epoch as i64);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Howard Hinnantの`civil_from_days`アルゴリズム（エポック日数→年月日、UTC）。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_known_unix_timestamp_as_utc() {
        // 2024-01-02 03:04:05 UTC
        assert_eq!(format_utc_datetime(1704164645), "2024-01-02 03:04:05");
    }

    #[test]
    fn clamps_negative_timestamps_to_the_epoch() {
        assert_eq!(format_utc_datetime(-100), "1970-01-01 00:00:00");
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    fn rot(max_bytes: u64, keep_files: u32) -> Rotation {
        Rotation { max_bytes, keep_files }
    }

    #[test]
    fn appends_without_rotating_below_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferro.log");
        append_line(&path, "one", rot(100, 1));
        append_line(&path, "two", rot(100, 1));
        assert_eq!(read(&path), "one\ntwo\n");
        assert!(!rotated_path(&path, 1).exists());
    }

    #[test]
    fn rotates_to_dot_one_once_the_limit_is_reached() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferro.log");
        append_line(&path, "0123456789", rot(10, 1)); // 11バイト（改行込み）
        append_line(&path, "next", rot(10, 1));
        assert_eq!(read(&rotated_path(&path, 1)), "0123456789\n");
        assert_eq!(read(&path), "next\n");
        assert_eq!(rotated_path(&path, 1).file_name().unwrap(), "ferro.log.1");
    }

    #[test]
    fn a_second_rotation_replaces_the_old_dot_one_when_keeping_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferro.log");
        for line in ["first-generation", "second-generation", "third"] {
            append_line(&path, line, rot(10, 1));
        }
        assert_eq!(read(&rotated_path(&path, 1)), "second-generation\n");
        assert_eq!(read(&path), "third\n");
        assert!(!rotated_path(&path, 2).exists());
    }

    #[test]
    fn keeps_the_configured_number_of_generations() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferro.log");
        for line in ["gen-aaaaaa-1", "gen-aaaaaa-2", "gen-aaaaaa-3", "gen-aaaaaa-4", "now"] {
            append_line(&path, line, rot(10, 3));
        }
        assert_eq!(read(&path), "now\n");
        assert_eq!(read(&rotated_path(&path, 1)), "gen-aaaaaa-4\n");
        assert_eq!(read(&rotated_path(&path, 2)), "gen-aaaaaa-3\n");
        assert_eq!(read(&rotated_path(&path, 3)), "gen-aaaaaa-2\n");
        assert!(!rotated_path(&path, 4).exists());
    }

    #[test]
    fn reducing_the_generations_removes_the_extra_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferro.log");
        for line in ["gen-aaaaaa-1", "gen-aaaaaa-2", "gen-aaaaaa-3", "gen-aaaaaa-4"] {
            append_line(&path, line, rot(10, 3));
        }
        append_line(&path, "now", rot(10, 1));
        assert_eq!(read(&rotated_path(&path, 1)), "gen-aaaaaa-4\n");
        assert!(!rotated_path(&path, 2).exists());
        assert!(!rotated_path(&path, 3).exists());
    }

    #[test]
    fn keeping_zero_generations_discards_the_old_log() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferro.log");
        append_line(&path, "0123456789", rot(10, 0));
        append_line(&path, "next", rot(10, 0));
        assert_eq!(read(&path), "next\n");
        assert!(!rotated_path(&path, 1).exists());
    }

    #[test]
    fn a_zero_limit_never_rotates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ferro.log");
        append_line(&path, "0123456789", rot(0, 1));
        append_line(&path, "next", rot(0, 1));
        assert_eq!(read(&path), "0123456789\nnext\n");
        assert!(!rotated_path(&path, 1).exists());
    }

    #[test]
    fn rotation_comes_from_the_settings() {
        let s = settings::Settings {
            log_max_kb: 2,
            log_keep_files: 3,
            ..Default::default()
        };
        assert_eq!(Rotation::from_settings(&s), rot(2048, 3));
        assert_eq!(Rotation::from_settings(&Default::default()), rot(1024 * 1024, 1));
    }
}
