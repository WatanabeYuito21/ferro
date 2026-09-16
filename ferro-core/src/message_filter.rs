//! 「差出人/件名/本文に特定の文字列を含む」「添付ファイルがある/ない」という条件を
//! 表す共有データ形。`label_rules`（sync時の自動ラベル付け）と`search`（詳細検索の
//! Tantivyクエリ）の両方の入力として使うが、判定エンジンは意図的に別々にする
//! （`label_rules`のドキュメント参照。sync時の自動判定とバックフィルで判定結果が
//! ズレないよう、ラベルルール側は常にこの完全一致ロジックのみを通す）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageFilter {
    /// 差出人（表示名・アドレスをまとめた文字列）に含まれるべき部分文字列。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_contains: Option<String>,
    /// Noneは「問わない」。Some(true)は添付あり、Some(false)は添付なしのみ一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_attachment: Option<bool>,
}

impl MessageFilter {
    /// 何も指定されていない（=判定不能な空条件）かどうか。
    pub fn is_empty(&self) -> bool {
        self.from_contains.is_none()
            && self.subject_contains.is_none()
            && self.body_contains.is_none()
            && self.has_attachment.is_none()
    }

    /// 指定された全フィールドをAND判定する。大文字小文字は区別しない部分一致。
    /// 空条件（`is_empty`）は「何も指定していない」＝常にtrueではなく常にfalseとして扱う
    /// （呼び出し側が空のルールを誤って全メッセージにマッチさせないようにするため）。
    pub fn matches(&self, subject: &str, from: &str, body: &str, has_attachment: bool) -> bool {
        if self.is_empty() {
            return false;
        }
        if let Some(needle) = &self.from_contains
            && !contains_ignore_case(from, needle)
        {
            return false;
        }
        if let Some(needle) = &self.subject_contains
            && !contains_ignore_case(subject, needle)
        {
            return false;
        }
        if let Some(needle) = &self.body_contains
            && !contains_ignore_case(body, needle)
        {
            return false;
        }
        if let Some(expected) = self.has_attachment
            && expected != has_attachment
        {
            return false;
        }
        true
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_filter_matches_nothing() {
        let filter = MessageFilter::default();
        assert!(filter.is_empty());
        assert!(!filter.matches("subject", "from", "body", false));
        assert!(!filter.matches("subject", "from", "body", true));
    }

    #[test]
    fn from_contains_is_case_insensitive() {
        let filter = MessageFilter {
            from_contains: Some("ALICE".to_string()),
            ..Default::default()
        };
        assert!(filter.matches("hi", "Alice <alice@example.com>", "body", false));
        assert!(!filter.matches("hi", "Bob <bob@example.com>", "body", false));
    }

    #[test]
    fn all_specified_fields_must_match_and() {
        let filter = MessageFilter {
            from_contains: Some("invoice@".to_string()),
            subject_contains: Some("領収書".to_string()),
            body_contains: None,
            has_attachment: Some(true),
        };
        assert!(filter.matches("9月分領収書", "invoice@example.com", "body", true));
        // 件名が違う
        assert!(!filter.matches("お知らせ", "invoice@example.com", "body", true));
        // 添付が無い
        assert!(!filter.matches("9月分領収書", "invoice@example.com", "body", false));
        // 差出人が違う
        assert!(!filter.matches("9月分領収書", "someone@example.com", "body", true));
    }

    #[test]
    fn has_attachment_false_requires_no_attachment() {
        let filter = MessageFilter {
            has_attachment: Some(false),
            ..Default::default()
        };
        assert!(filter.matches("s", "f", "b", false));
        assert!(!filter.matches("s", "f", "b", true));
    }

    #[test]
    fn unspecified_fields_are_ignored() {
        let filter = MessageFilter {
            subject_contains: Some("hello".to_string()),
            ..Default::default()
        };
        assert!(filter.matches("hello world", "anyone", "anything", true));
        assert!(filter.matches("hello world", "anyone", "anything", false));
    }
}
