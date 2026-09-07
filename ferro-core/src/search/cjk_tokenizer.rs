//! 部分一致（substring）検索のための自前バイグラムトークナイザ。
//!
//! 元々は日本語（分かち書きしない言語）専用だった: Tantivyの標準"default"
//! トークナイザは空白・記号区切り前提で、日本語のテキストをほぼ1文1トークンとして
//! 扱ってしまい実質検索できない。形態素解析（lindera等、辞書同梱で数十MB単位）を
//! 使う手もあるが、このアプリは軽量なデスクトップ配布を志向しているため
//! （CLAUDE.md参照: notmuch/Xapian不採用の理由と同じ動機）、辞書を持たない
//! バイグラム（2文字ずつの重複あり分割）方式を採用した。
//!
//! 当初は英数字の連続を「通常の単語区切り」（1トークン）として扱っていたが、
//! これだと英数字については単語全体の完全一致でしかヒットせず、「部分一致検索に
//! してほしい」という指摘のとおり利用者の直感（日本語と同じ感覚で一部分だけ
//! 入力しても見つかる）に反していた。そのため英数字も含め全ての文字種を同じ
//! バイグラム方式で統一している。Tantivyの`QueryParser`は、1つのクエリ語が
//! 複数トークンに分かれる場合それらを位置(position)込みのフレーズクエリとして
//! 組み立てる（`add_document`時と同じトークナイザでクエリも解析されるため）。
//! バイグラムの位置は元の文字列上で連続しているので、これにより「バイグラムが
//! 単にどこかに散らばって存在する」ではなく「元のクエリ文字列が実際に部分文字列
//! として連続して現れる」ことを要求でき、真の部分一致検索になる
//! （Elasticsearch/LuceneのCJKAnalyzerと同じ考え方）。
//! 辞書更新が要らず実装・検証も単純だが、形態素解析ほど精密ではない
//! （意味のある単語単位ではなく機械的な部分文字列一致になる）点は変わらない。

use tantivy::tokenizer::{Token, TokenStream, Tokenizer};

/// 「CJK文字かどうか」でバイグラムの区切り(run)を分けるためだけに使う
/// （英数字と地続きにバイグラムを作ると、"テスト123"の"ト"と"1"のように
/// 意味のない文字種をまたいだバイグラムができてしまうため）。
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
        if c.is_alphanumeric() {
            let run_is_cjk = is_cjk(c);
            let run_start = i;
            let mut j = i;
            while j < n && chars[j].1.is_alphanumeric() && is_cjk(chars[j].1) == run_is_cjk {
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

    /// 「検索が単語検索っぽい」＝英数字は単語全体の完全一致でしかヒットしない、
    /// という指摘への対応。英数字もCJKと同じくバイグラムにし、部分一致検索を
    /// 効くようにする（モジュールのドキュメントコメント参照）。
    #[test]
    fn latin_words_are_bigrammed_too_for_substring_search() {
        assert_eq!(
            run("Hello world"),
            vec!["He", "el", "ll", "lo", "wo", "or", "rl", "ld"]
        );
    }

    #[test]
    fn mixed_japanese_and_latin_text() {
        assert_eq!(
            run("Subject: 検索テスト123"),
            vec!["Su", "ub", "bj", "je", "ec", "ct", "検索", "索テ", "テス", "スト", "12", "23"]
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
        // 全文検索(3バイグラム) + test(3バイグラム: te,es,st) = 6トークン。
        let tokens = tokenize("全文検索 test");
        let positions: Vec<usize> = tokens.iter().map(|t| t.position).collect();
        assert_eq!(positions, vec![0, 1, 2, 3, 4, 5]);
    }
}
