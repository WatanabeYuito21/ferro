//! 検索欄に入力された文字列を、全文検索の語と絞り込み条件に分解する（詳細検索）。
//!
//! Gmail風の`キー:値`の書式を受け付ける（日本語のキー名も使える）:
//!
//! | 書式 | 意味 |
//! | --- | --- |
//! | `from:` / `差出人:` | 差出人（表示名・アドレス）に含む |
//! | `subject:` / `件名:` | 件名に含む |
//! | `body:` / `本文:` | 本文に含む |
//! | `has:attachment` / `has:添付` | 添付ファイルあり |
//! | `is:unread` / `is:read` / `is:starred` | 未読 / 既読 / スター付き（`未読`/`既読`/`スター`も可） |
//! | `in:inbox` / `starred` / `snoozed` / `archive` / `trash` | フォルダ（`受信箱`等も可） |
//! | `label:` / `ラベル:` | ラベル名（完全一致、大文字小文字は区別しない） |
//! | `after:2026-01-01` / `before:2026-02-01` | 受信日（afterはその日を含み、beforeは含まない） |
//!
//! 値に空白を含めたい場合は`subject:"週次 レポート"`のように`"`で囲む。先頭に`-`を
//! 付けると否定（`-from:noreply`、`-has:attachment`）。キー名が分からない語や、
//! 値が解釈できない語（`before:あした`等）は普通の検索語として扱う（入力したものが
//! 黙って捨てられないように）。

use crate::db::messages::Folder;

/// 全文検索の語をどのフィールドで探すか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextField {
    /// 件名・差出人・本文のどれか（キー無しの語）。
    Any,
    From,
    Subject,
    Body,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextTerm {
    pub field: TextField,
    pub text: String,
    pub negated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedQuery {
    pub text: Vec<TextTerm>,
    /// この時刻（Unix秒）以降に受信したもの。
    pub after: Option<i64>,
    /// この時刻（Unix秒）より前に受信したもの。
    pub before: Option<i64>,
    pub has_attachment: Option<bool>,
    pub is_read: Option<bool>,
    pub is_flagged: Option<bool>,
    /// (フォルダ, 否定か)
    pub folders: Vec<(Folder, bool)>,
    /// (ラベル名, 否定か)
    pub labels: Vec<(String, bool)>,
}

impl ParsedQuery {
    /// 全文検索の語が1つでもあるか（否定の語も含む）。
    pub fn has_text(&self) -> bool {
        !self.text.is_empty()
    }

    /// DB側で絞り込む条件（フラグ・フォルダ・ラベル・添付）があるか。日付は
    /// 全文検索側でも絞り込めるので含めない。
    pub fn has_db_filters(&self) -> bool {
        self.has_attachment.is_some()
            || self.is_read.is_some()
            || self.is_flagged.is_some()
            || !self.folders.is_empty()
            || !self.labels.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        !self.has_text() && !self.has_db_filters() && self.after.is_none() && self.before.is_none()
    }
}

/// `query`を解釈する。`utc_offset_seconds`は`after:`/`before:`の日付を何時の0時と
/// みなすか（利用者のタイムゾーン。日本なら`9 * 3600`）。
pub fn parse(query: &str, utc_offset_seconds: i64) -> ParsedQuery {
    let mut parsed = ParsedQuery::default();
    for token in split_tokens(query) {
        let (negated, body) = match token.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => (true, rest),
            _ => (false, token.as_str()),
        };
        if !apply_operator(&mut parsed, body, negated, utc_offset_seconds) {
            let text = unquote(body);
            if !text.is_empty() {
                parsed.text.push(TextTerm { field: TextField::Any, text, negated });
            }
        }
    }
    parsed
}

/// 空白で区切る。ただし`"`で囲まれた中の空白では区切らない（`subject:"a b"`）。
fn split_tokens(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in query.chars() {
        if c == '"' {
            in_quotes = !in_quotes;
            current.push(c);
        } else if c.is_whitespace() && !in_quotes {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn unquote(value: &str) -> String {
    value.replace('"', "").trim().to_string()
}

/// `key:value`の形なら`parsed`に反映してtrueを返す。キーが分からない・値が
/// 解釈できない場合はfalse（呼び出し側が普通の検索語として扱う）。
fn apply_operator(parsed: &mut ParsedQuery, token: &str, negated: bool, utc_offset_seconds: i64) -> bool {
    let Some((key, value)) = token.split_once(':') else {
        return false;
    };
    let value = unquote(value);
    if value.is_empty() {
        return false;
    }
    let key = key.to_lowercase();
    let lower = value.to_lowercase();
    match key.as_str() {
        "from" | "差出人" => push_text(parsed, TextField::From, value, negated),
        "subject" | "件名" => push_text(parsed, TextField::Subject, value, negated),
        "body" | "本文" => push_text(parsed, TextField::Body, value, negated),
        "has" if matches!(lower.as_str(), "attachment" | "attachments" | "添付") => {
            parsed.has_attachment = Some(!negated);
        }
        "is" => match lower.as_str() {
            "unread" | "未読" => parsed.is_read = Some(negated),
            "read" | "既読" => parsed.is_read = Some(!negated),
            "starred" | "flagged" | "スター" => parsed.is_flagged = Some(!negated),
            _ => return false,
        },
        "in" => {
            let folder = match lower.as_str() {
                "inbox" | "受信箱" => Folder::Inbox,
                "starred" | "スター付き" => Folder::Starred,
                "snoozed" | "スヌーズ" => Folder::Snoozed,
                "archive" | "archived" | "アーカイブ" => Folder::Archive,
                "trash" | "ゴミ箱" => Folder::Trash,
                _ => return false,
            };
            parsed.folders.push((folder, negated));
        }
        "label" | "ラベル" => parsed.labels.push((value, negated)),
        // 日付の否定は「その日より前/以降」と同じ意味になるので、入れ替えて扱う。
        "after" | "before" => {
            let Some(at) = parse_date(&value, utc_offset_seconds) else {
                return false;
            };
            if (key == "after") != negated {
                parsed.after = Some(parsed.after.map_or(at, |current| current.max(at)));
            } else {
                parsed.before = Some(parsed.before.map_or(at, |current| current.min(at)));
            }
        }
        _ => return false,
    }
    true
}

fn push_text(parsed: &mut ParsedQuery, field: TextField, text: String, negated: bool) {
    parsed.text.push(TextTerm { field, text, negated });
}

/// `YYYY-MM-DD`（区切りは`/`や`.`でもよい）を、そのタイムゾーンでのその日の0時の
/// Unix秒にする。
fn parse_date(value: &str, utc_offset_seconds: i64) -> Option<i64> {
    let parts: Vec<&str> = value.split(['-', '/', '.']).collect();
    let [year, month, day] = parts.as_slice() else {
        return None;
    };
    let year: i64 = year.parse().ok()?;
    let month: u32 = month.parse().ok()?;
    let day: u32 = day.parse().ok()?;
    if !(1970..=9999).contains(&year) || !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86400 - utc_offset_seconds)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Howard Hinnantの`days_from_civil`（年月日→エポックからの日数、UTC）。
/// `logging::civil_from_days`の逆変換。
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn any(text: &str) -> TextTerm {
        TextTerm { field: TextField::Any, text: text.into(), negated: false }
    }

    #[test]
    fn plain_words_search_every_text_field() {
        let parsed = parse("請求書  alert", 0);
        assert_eq!(parsed.text, vec![any("請求書"), any("alert")]);
        assert!(!parsed.has_db_filters());
    }

    #[test]
    fn field_operators_and_quotes() {
        let parsed = parse(r#"from:alice subject:"週次 レポート" 本文:障害 -from:noreply"#, 0);
        assert_eq!(
            parsed.text,
            vec![
                TextTerm { field: TextField::From, text: "alice".into(), negated: false },
                TextTerm { field: TextField::Subject, text: "週次 レポート".into(), negated: false },
                TextTerm { field: TextField::Body, text: "障害".into(), negated: false },
                TextTerm { field: TextField::From, text: "noreply".into(), negated: true },
            ]
        );
    }

    #[test]
    fn flag_folder_label_and_attachment_filters() {
        let parsed = parse("has:attachment is:unread is:starred in:archive label:仕事 -label:old", 0);
        assert_eq!(parsed.has_attachment, Some(true));
        assert_eq!(parsed.is_read, Some(false));
        assert_eq!(parsed.is_flagged, Some(true));
        assert_eq!(parsed.folders, vec![(Folder::Archive, false)]);
        assert_eq!(parsed.labels, vec![("仕事".into(), false), ("old".into(), true)]);
        assert!(parsed.text.is_empty());

        let parsed = parse("-has:添付 -is:unread IS:既読", 0);
        assert_eq!(parsed.has_attachment, Some(false));
        assert_eq!(parsed.is_read, Some(true));
    }

    #[test]
    fn dates_are_midnight_in_the_given_time_zone() {
        // 2026-01-02 00:00 UTC = 1767312000
        assert_eq!(parse("after:2026-01-02", 0).after, Some(1_767_312_000));
        // 日本時間の0時はUTCの前日15時。
        assert_eq!(parse("before:2026/01/02", 9 * 3600).before, Some(1_767_312_000 - 9 * 3600));
        // 否定は向きを入れ替える。
        assert_eq!(parse("-after:2026-01-02", 0).before, Some(1_767_312_000));
        // 複数指定したら一番狭い範囲。
        let parsed = parse("after:2026-01-01 after:2026-01-02 before:2026-03-01 before:2026-02-01", 0);
        assert_eq!(parsed.after, Some(1_767_312_000));
        assert_eq!(parsed.before, Some(1_769_904_000));
    }

    #[test]
    fn unknown_or_invalid_operators_stay_as_plain_words() {
        let parsed = parse("http://example.com before:2026-02-30 has:pdf is:maybe :x", 0);
        assert_eq!(
            parsed.text,
            vec![any("http://example.com"), any("before:2026-02-30"), any("has:pdf"), any("is:maybe"), any(":x")]
        );
        assert!(parsed.before.is_none());
    }

    #[test]
    fn a_lone_dash_is_a_word_not_a_negation() {
        assert_eq!(parse("-", 0).text, vec![any("-")]);
    }

    #[test]
    fn days_from_civil_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2024, 2, 29), 19782);
    }
}
