//! 日本語（および中国語・韓国語）テキストのための簡易トークナイザ。
//!
//! Tantivyの標準"default"トークナイザは空白・記号区切り前提で、日本語のような
//! 分かち書きしない言語のテキストをほぼ1文の1トークンとして扱ってしまい、
//! 実質検索できない。形態素解析（lindera等、辞書同梱で数十MB単位）を使う手も
//! あるが、このアプリは軽量なデスクトップ配布を志向しているため
//! （CLAUDE.md参照: notmuch/Xapian不採用の理由と同じ動機）、
//! 辞書を持たないCJK文字のバイグラム（2文字ずつの重複あり分割）方式を採用する。
//! Elasticsearch/LuceneのCJKAnalyzerと同じ考え方で、形態素解析ほど精密ではないが
//! 辞書更新の必要がなく、実装も検証も単純。
//!
//! CJK文字以外（英数字）の連続は通常の単語区切りとして扱う。

use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF   // ひらがな・カタカナ
        | 0x3400..=0x4DBF  // CJK拡張A
        | 0x4E00..=0x9FFF  // CJK統合漢字
        | 0xF900..=0xFAFF  // CJK互換漢字
        | 0xFF66..=0xFF9F  // 半角カタカナ
    )
}

fn tokenize(text: &str) -> Vec<Token> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let offset_at = |i: usize| -> usize {
        if i < n { chars[i].0 } else { text.len() }
    };

    let mut tokens = Vec::new();
    let mut position = 0usize;
    let mut i = 0usize;

    while i < n {
        let c = chars[i].1;
        if is_cjk(c) {
            let run_start = i;
            let mut j = i;
            while j < n && is_cjk(chars[j].1) {
                j += 1;
            }
            if j - run_start == 1 {
                push_token(&mut tokens, text, offset_at(run_start), offset_at(run_start + 1), &mut position);
            } else {
                for k in run_start..(j - 1) {
                    push_token(&mut tokens, text, offset_at(k), offset_at(k + 2), &mut position);
                }
            }
            i = j;
        } else if c.is_alphanumeric() {
            let run_start = i;
            let mut j = i;
            while j < n && chars[j].1.is_alphanumeric() && !is_cjk(chars[j].1) {
                j += 1;
            }
            push_token(&mut tokens, text, offset_at(run_start), offset_at(j), &mut position);
            i = j;
        } else {
            i += 1;
        }
    }

    tokens
}

fn push_token(tokens: &mut Vec<Token>, text: &str, start: usize, end: usize, position: &mut usize) {
    tokens.push(Token {
        offset_from: start,
        offset_to: end,
        position: *position,
        text: text[start..end].to_string(),
        position_length: 1,
    });
    *position += 1;
}

#[derive(Clone, Default)]
pub struct CjkBigramTokenizer;

pub struct CjkBigramTokenStream {
    tokens: Vec<Token>,
    index: usize,
}

impl Tokenizer for CjkBigramTokenizer {
    type TokenStream<'a> = CjkBigramTokenStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> CjkBigramTokenStream {
        CjkBigramTokenStream {
            tokens: tokenize(text),
            index: 0,
        }
    }
}

impl TokenStream for CjkBigramTokenStream {
    fn advance(&mut self) -> bool {
        if self.index < self.tokens.len() {
            self.index += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.tokens[self.index - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.index - 1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Vec<String> {
        tokenize(text).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn splits_japanese_into_overlapping_bigrams() {
        assert_eq!(run("全文検索"), vec!["全文", "文検", "検索"]);
    }

    #[test]
    fn single_cjk_character_is_its_own_token() {
        assert_eq!(run("検"), vec!["検"]);
    }

    #[test]
    fn latin_words_are_split_normally_not_bigrammed() {
        assert_eq!(run("Hello world"), vec!["Hello", "world"]);
    }

    #[test]
    fn mixed_japanese_and_latin_text() {
        assert_eq!(
            run("Subject: 検索テスト123"),
            vec!["Subject", "検索", "索テ", "テス", "スト", "123"]
        );
    }

    #[test]
    fn offsets_point_back_into_the_original_string() {
        let text = "検索エンジン";
        let tokens = tokenize(text);
        for token in &tokens {
            assert_eq!(&text[token.offset_from..token.offset_to], token.text);
        }
    }

    #[test]
    fn positions_are_sequential() {
        let tokens = tokenize("全文検索 test");
        let positions: Vec<usize> = tokens.iter().map(|t| t.position).collect();
        assert_eq!(positions, vec![0, 1, 2, 3]);
    }
}
