//! 保存済みメッセージを`.eml`（Maildirに保存している生のRFC 5322バイト列そのもの）
//! として書き出す。Maildirのファイル自体が既に`.eml`なので、中身の変換はせず
//! コピーするだけ。ここにあるのは保存ダイアログ等で既定にするファイル名の組み立て。

/// ファイル名に使えない（Windowsで禁止されている）文字。Linux/macOSで使える文字も
/// 含むが、同じファイルを別のOSへ持っていけるよう全OSでこちらに揃える。
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windowsの予約デバイス名。拡張子を付けても（`CON.eml`等）ファイルとして作れない。
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 件名の文字数がこれを超えたら切り詰める（長すぎるパスを避けるため）。
const MAX_STEM_CHARS: usize = 80;

/// `.eml`の既定ファイル名を件名から作る。ファイル名に使えない文字・制御文字は
/// `_`に置き換え、空白は1つに詰める。件名が無い・使える文字が残らない場合は
/// `message-<id>.eml`にする。
pub fn suggested_eml_filename(subject: Option<&str>, message_id: i64) -> String {
    let mut stem = String::new();
    for c in subject.unwrap_or("").chars() {
        if c.is_whitespace() {
            if !stem.is_empty() && !stem.ends_with(' ') {
                stem.push(' ');
            }
        } else if c.is_control() || FORBIDDEN.contains(&c) {
            stem.push('_');
        } else {
            stem.push(c);
        }
    }
    let mut stem: String = stem.chars().take(MAX_STEM_CHARS).collect();
    // Windowsは末尾の空白・ピリオドを黙って取り除くので、最初から付けない。
    while stem.ends_with([' ', '.']) {
        stem.pop();
    }
    let reserved = RESERVED.iter().any(|name| name.eq_ignore_ascii_case(&stem));
    if stem.trim_matches('_').is_empty() || reserved {
        return format!("message-{message_id}.eml");
    }
    format!("{stem}.eml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_subject_as_the_file_name() {
        assert_eq!(suggested_eml_filename(Some("週次レポート"), 1), "週次レポート.eml");
    }

    #[test]
    fn replaces_characters_that_windows_forbids() {
        assert_eq!(
            suggested_eml_filename(Some("Re: [alert] a/b\\c?*<x>|\"y\""), 1),
            "Re_ [alert] a_b_c___x___y_.eml"
        );
    }

    #[test]
    fn collapses_whitespace_and_control_characters() {
        assert_eq!(suggested_eml_filename(Some("  a \t\r\n b  "), 1), "a b.eml");
        assert_eq!(suggested_eml_filename(Some("a\u{7}b"), 1), "a_b.eml");
    }

    #[test]
    fn strips_trailing_dots_and_spaces() {
        assert_eq!(suggested_eml_filename(Some("done... "), 1), "done.eml");
    }

    #[test]
    fn falls_back_to_the_message_id() {
        assert_eq!(suggested_eml_filename(None, 42), "message-42.eml");
        assert_eq!(suggested_eml_filename(Some("   "), 42), "message-42.eml");
        assert_eq!(suggested_eml_filename(Some("///"), 42), "message-42.eml");
        assert_eq!(suggested_eml_filename(Some("con"), 42), "message-42.eml");
    }

    #[test]
    fn truncates_long_subjects_on_character_boundaries() {
        let subject = "あ".repeat(200);
        let name = suggested_eml_filename(Some(&subject), 1);
        assert_eq!(name, format!("{}.eml", "あ".repeat(MAX_STEM_CHARS)));
    }
}
