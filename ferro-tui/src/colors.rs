//! 16進カラー文字列("#rrggbb")の`ratatui::style::Color`変換と、CJK文字幅を
//! 考慮した文字列の省略表示（GUI側の`text-overflow: ellipsis`に相当。
//! `unicode-width`を使うことで全角文字を途中で切らないようにする）。

use ratatui::style::Color;
use unicode_width::UnicodeWidthChar;

use ferro_core::color_rules::ColorRule;

/// 色分けルールの判定（`frontend/src/lib/colorRules.js`の`matchColor`のRust版）。
/// 件名・差出人（表示名・アドレス）・本文プレビューのいずれかに、ルールの
/// パターンが(大文字小文字を区別せず)部分一致で含まれていればマッチ。
/// 複数のルールが該当する場合は先頭から見て最初にマッチしたものを使う
/// （color_rules.tomlに書かれた順序＝優先順位。GUI版と同じ仕様）。
pub fn match_color(
    rules: &[ColorRule],
    subject: Option<&str>,
    from_name: Option<&str>,
    from_addr: Option<&str>,
    preview: Option<&str>,
) -> Option<Color> {
    if rules.is_empty() {
        return None;
    }
    let haystack = [subject, from_name, from_addr, preview]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if haystack.is_empty() {
        return None;
    }
    for rule in rules {
        if !rule.pattern.is_empty() && haystack.contains(&rule.pattern.to_lowercase()) {
            return parse_hex_color(&rule.color);
        }
    }
    None
}

/// "#rrggbb"形式の文字列を`Color::Rgb`に変換する。パースできない場合は
/// `None`（呼び出し側でフォールバック色を使う。色分けルール・ラベル色の
/// 手入力/手編集ミスで表示が壊れないようにするため）。
pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

/// 表示幅(`max_width`桁)に収まるよう、全角文字を途中で切らずに省略する。
/// 収まらない場合は末尾を1文字分の"…"に置き換える。
pub fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let total_width: usize = text.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total_width <= max_width {
        return text.to_string();
    }

    let budget = max_width.saturating_sub(1); // "…"の分を1引く
    let mut result = String::new();
    let mut width = 0usize;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if width + w > budget {
            break;
        }
        width += w;
        result.push(c);
    }
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_color_with_and_without_hash() {
        assert_eq!(parse_hex_color("#3f6b5c"), Some(Color::Rgb(0x3f, 0x6b, 0x5c)));
        assert_eq!(parse_hex_color("3f6b5c"), Some(Color::Rgb(0x3f, 0x6b, 0x5c)));
    }

    #[test]
    fn rejects_invalid_hex_color() {
        assert_eq!(parse_hex_color("not-a-color"), None);
        assert_eq!(parse_hex_color("#fff"), None);
    }

    #[test]
    fn truncate_keeps_short_ascii_text_unchanged() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
    }

    #[test]
    fn truncate_ascii_text_appends_ellipsis() {
        assert_eq!(truncate_to_width("hello world", 8), "hello w…");
    }

    #[test]
    fn truncate_never_splits_a_wide_character_in_half() {
        // 全角文字(幅2)が丁度収まらない場合、その文字ごと省略される。
        // "あい"(幅4) + "…"(幅1) = 5に収まるが、"う"まで含めると幅6で溢れる。
        assert_eq!(truncate_to_width("あいうえお", 5), "あい…");
    }

    fn rule(pattern: &str, color: &str) -> ColorRule {
        ColorRule { pattern: pattern.to_string(), color: color.to_string() }
    }

    #[test]
    fn match_color_finds_pattern_in_subject_case_insensitively() {
        let rules = vec![rule("critical", "#b00020")];
        let color = match_color(&rules, Some("Critical alert"), None, None, None);
        assert_eq!(color, Some(Color::Rgb(0xb0, 0x00, 0x20)));
    }

    #[test]
    fn match_color_returns_none_when_nothing_matches() {
        let rules = vec![rule("critical", "#b00020")];
        assert_eq!(match_color(&rules, Some("all good"), None, None, None), None);
    }

    #[test]
    fn match_color_prefers_the_first_matching_rule() {
        let rules = vec![rule("alert", "#111111"), rule("critical", "#222222")];
        let color = match_color(&rules, Some("critical alert"), None, None, None);
        assert_eq!(color, Some(Color::Rgb(0x11, 0x11, 0x11)));
    }
}
