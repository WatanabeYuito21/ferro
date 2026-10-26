//! 検索結果から開いた本文で、検索語に一致した箇所を目立たせるための補助。
//!
//! 検索そのもの（Tantivyのバイグラム＋前方一致）は語を部分文字列として探すので、ここでも
//! 大文字小文字を無視した部分文字列として探す。否定の語（`-foo`）は一致しないものを探した
//! 結果なので強調しない。GUIはフロント側（`frontend/src/lib/highlight.js`）が同じ規則で
//! 正規表現を組み、こちらの`terms`だけを使う。TUIは`ranges`も使う。

use super::query::{self, TextField, TextTerm};

/// `query`のうち強調に使う語（否定でないもの）。
pub fn terms(query: &str) -> Vec<TextTerm> {
    query::parse(query, 0).text.into_iter().filter(|term| !term.negated && !term.text.trim().is_empty()).collect()
}

/// `field`の欄に当てる語だけを選ぶ（キー無しの語はどの欄にも当てる）。
pub fn needles_for(terms: &[TextTerm], field: TextField) -> Vec<&str> {
    terms
        .iter()
        .filter(|term| term.field == TextField::Any || term.field == field)
        .map(|term| term.text.as_str())
        .collect()
}

/// `text`の中で`needles`のどれかに一致する範囲（バイト位置、重ならず昇順）。同じ位置から
/// 複数の語が一致するときは長い方を取る。大文字小文字は区別しない。
pub fn ranges(text: &str, needles: &[&str]) -> Vec<(usize, usize)> {
    let needles: Vec<Vec<char>> =
        needles.iter().map(|n| n.chars().collect::<Vec<_>>()).filter(|n| !n.is_empty()).collect();
    if needles.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let longest = needles
            .iter()
            .filter(|needle| {
                needle.len() <= chars.len() - i
                    && needle.iter().zip(&chars[i..]).all(|(&a, &(_, b))| a == b || a.to_lowercase().eq(b.to_lowercase()))
            })
            .map(|needle| needle.len())
            .max();
        match longest {
            Some(len) => {
                let start = chars[i].0;
                let end = chars.get(i + len).map_or(text.len(), |&(pos, _)| pos);
                found.push((start, end));
                i += len;
            }
            None => i += 1,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_skip_negated_words_and_keep_fields() {
        let terms = terms("障害 -復旧 subject:\"週次 レポート\" is:unread from:Nagios");
        let pairs: Vec<(TextField, &str)> = terms.iter().map(|t| (t.field, t.text.as_str())).collect();
        assert_eq!(
            pairs,
            vec![(TextField::Any, "障害"), (TextField::Subject, "週次 レポート"), (TextField::From, "Nagios")]
        );
        assert_eq!(needles_for(&terms, TextField::Body), vec!["障害"]);
        assert_eq!(needles_for(&terms, TextField::Subject), vec!["障害", "週次 レポート"]);
    }

    #[test]
    fn ranges_are_case_insensitive_and_prefer_longer_matches() {
        let text = "Disk ERROR on srv-01: error count 3 (エラー)";
        assert_eq!(
            ranges(text, &["error", "エラー"]).iter().map(|&(s, e)| &text[s..e]).collect::<Vec<_>>(),
            vec!["ERROR", "error", "エラー"]
        );
        assert_eq!(
            ranges("srv-01 srv-012", &["srv-0", "srv-01"]).iter().map(|&(s, e)| (s, e)).collect::<Vec<_>>(),
            vec![(0, 6), (7, 13)]
        );
        assert!(ranges("abc", &[]).is_empty());
        assert!(ranges("ab", &["abc"]).is_empty());
    }
}
