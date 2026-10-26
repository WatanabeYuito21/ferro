//! メールのヘッダー・ソース（生のバイト列）を画面に出すための文字列化。
//!
//! Maildirのファイル自体がRFC 5322の生メールなので、デコードやパースはせず
//! 「ヘッダー部（最初の空行まで）」か「全体」をそのまま見せる。調査用（経路上の
//! ゲートウェイが付けたヘッダーや`Received`の連なりを確認する等）の表示なので、
//! RFC 2047のエンコードもそのまま残す。生のShift_JIS等で書かれた行だけは、件名の
//! 文字化け対策（`parse::recover_mojibake_header`）と同じ順で読み直す。

use super::parse::try_decode_legacy_japanese;

/// 全体表示で返す最大バイト数（大きな添付付きのメールで画面ごと固まらないように）。
pub const SOURCE_DISPLAY_LIMIT: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceText {
    pub text: String,
    /// `SOURCE_DISPLAY_LIMIT`で切り詰めたか。
    pub truncated: bool,
    /// メール全体のバイト数。
    pub total_bytes: usize,
}

/// ヘッダー部（最初の空行の手前まで、末尾の改行を含む）。空行が無ければ全体。
pub fn header_section(raw: &[u8]) -> &[u8] {
    let mut pos = 0;
    while pos < raw.len() {
        let line_end = raw[pos..].iter().position(|&b| b == b'\n').map_or(raw.len(), |i| pos + i + 1);
        let line = &raw[pos..line_end];
        if line == b"\n" || line == b"\r\n" {
            return &raw[..pos];
        }
        pos = line_end;
    }
    raw
}

/// `headers_only`ならヘッダー部だけ、そうでなければ全体（`SOURCE_DISPLAY_LIMIT`まで）を表示用の文字列にする。
pub fn source_text(raw: &[u8], headers_only: bool) -> SourceText {
    let section = if headers_only { header_section(raw) } else { raw };
    let (shown, truncated) = if section.len() > SOURCE_DISPLAY_LIMIT {
        // 行の途中（マルチバイト文字の途中）で切らないよう、直前の改行まで戻す。
        let cut = section[..SOURCE_DISPLAY_LIMIT].iter().rposition(|&b| b == b'\n').map_or(SOURCE_DISPLAY_LIMIT, |i| i + 1);
        (&section[..cut], true)
    } else {
        (section, false)
    };
    SourceText { text: decode_lines(shown), truncated, total_bytes: raw.len() }
}

/// 1行ずつ、UTF-8として読めればそのまま、読めなければ日本語の旧charsetを試し、
/// それも駄目なら置換文字入りで読む（行ごとにcharsetが違うメールもあるため）。
fn decode_lines(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        match std::str::from_utf8(line) {
            Ok(s) => text.push_str(s),
            Err(_) => match try_decode_legacy_japanese(line) {
                Some(s) => text.push_str(&s),
                None => text.push_str(&String::from_utf8_lossy(line)),
            },
        }
    }
    // 画面では改行コードの違いは要らないので\nに揃える。
    text.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &[u8] = b"From: a@example.com\r\nSubject: =?UTF-8?B?44OG44K544OI?=\r\nX-Folded: one\r\n two\r\n\r\nbody line\r\n\r\nsecond paragraph\r\n";

    #[test]
    fn header_section_stops_at_the_first_blank_line() {
        assert_eq!(
            header_section(RAW),
            &b"From: a@example.com\r\nSubject: =?UTF-8?B?44OG44K544OI?=\r\nX-Folded: one\r\n two\r\n"[..]
        );
        assert_eq!(header_section(b"From: a\nSubject: b\n\nbody"), &b"From: a\nSubject: b\n"[..]);
        assert_eq!(header_section(b"From: a\r\nno blank line"), &b"From: a\r\nno blank line"[..]);
    }

    #[test]
    fn source_text_keeps_encoded_words_and_normalizes_newlines() {
        let headers = source_text(RAW, true);
        assert_eq!(headers.text, "From: a@example.com\nSubject: =?UTF-8?B?44OG44K544OI?=\nX-Folded: one\n two\n");
        assert!(!headers.truncated);
        assert_eq!(headers.total_bytes, RAW.len());

        let full = source_text(RAW, false);
        assert!(full.text.ends_with("\nbody line\n\nsecond paragraph\n"));
    }

    #[test]
    fn raw_shift_jis_lines_are_decoded() {
        let (subject, _, _) = encoding_rs::SHIFT_JIS.encode("Subject: 検知しました\r\n");
        let mut raw = b"From: a@example.com\r\n".to_vec();
        raw.extend_from_slice(&subject);
        raw.extend_from_slice(b"\r\nbody");
        assert_eq!(source_text(&raw, true).text, "From: a@example.com\nSubject: 検知しました\n");
    }

    #[test]
    fn large_sources_are_cut_on_a_line_boundary() {
        let mut raw = b"Subject: big\r\n\r\n".to_vec();
        while raw.len() <= SOURCE_DISPLAY_LIMIT + 100 {
            raw.extend_from_slice("あいうえお\r\n".as_bytes());
        }
        let source = source_text(&raw, false);
        assert!(source.truncated);
        assert!(source.text.ends_with("あいうえお\n"));
        assert!(!source.text.contains('\u{FFFD}'));
        assert_eq!(source.total_bytes, raw.len());
    }
}
