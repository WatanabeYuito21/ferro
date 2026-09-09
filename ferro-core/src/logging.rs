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

use std::fs::OpenOptions;
use std::io::Write;

use crate::db::now_unix;
use crate::paths::log_path;

/// 診断ログに1行追記する。書き込み失敗（ディスクフルや権限エラー等）は
/// 無視する。
pub fn log_line(line: &str) {
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let ts = format_utc_datetime(now_unix());
    let _ = writeln!(file, "[{ts}] {line}");
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
}
