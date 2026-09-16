//! 受信時に自動でラベルを付与するルールをTOMLファイルで管理する。
//! `color_rules.rs`と同じ構造（正の情報源はファイル、DBテーブルへのreconcileは無い）だが、
//! こちらは実際にDBの`message_labels`へ書き込む（表示だけの色分けと違い、ラベル付与は
//! 永続状態の変更）。
//!
//! 判定（`MessageFilter::matches`）は`sync`（新着1件をその場で判定）と`apply_all`
//! （既存メールへの遡及適用、「今すぐ適用」ボタン）の両方から呼ぶが、常に同じ完全一致
//! （大文字小文字無視の部分文字列）ロジックを通す。詳細検索（Tantivyのバイグラム＋
//! 前方一致）とは意図的に別の判定エンジンにしている: もし詳細検索と同じあいまい判定を
//! 使うと、sync時の自動判定とバックフィルの結果がズレて「新着では付いたのに今すぐ適用
//! では付かない」という食い違いを生みかねないため（CLAUDE.mdに記録されている過去の
//! 実害パターンと同種の罠）。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::db::Connection;
use crate::db::labels;
use crate::db::messages;
use crate::mail::parse::extract_plain_text_body;
use crate::maildir;
use crate::message_filter::MessageFilter;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LabelRule {
    /// 付与するラベルの名前。`labels.name`はUNIQUE制約済み（migration 0003）なので
    /// 自然キーとして安全に使える。ラベルがリネーム/削除されて解決できなくなった
    /// ルールは（`accounts.toml`のアカウントと違い）自動削除も警告もせず、単に
    /// 何も付与しないルールとして静かにスキップする。
    pub label_name: String,
    #[serde(flatten)]
    pub filter: MessageFilter,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct LabelRulesFile {
    #[serde(rename = "rule", default)]
    pub rules: Vec<LabelRule>,
}

#[derive(Debug, thiserror::Error)]
pub enum LabelRulesError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: std::path::PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error(transparent)]
    Serialize(#[from] toml::ser::Error),
    #[error("rule index {0} out of range")]
    IndexOutOfRange(usize),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
}

/// 設定ファイルを読み込む。存在しない場合は空扱い（`color_rules::load`と同じ理由）。
pub fn load(path: &Path) -> Result<LabelRulesFile, LabelRulesError> {
    if !path.exists() {
        return Ok(LabelRulesFile::default());
    }
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|source| LabelRulesError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// バックグラウンド同期用。読み込み失敗（構文エラー等）時はエラーを見せる手段が
/// 無いため、空扱いにフォールバックして同期自体は止めない
/// （`settings::load_or_default`と同じ考え方。CLAUDE.md参照）。
pub fn load_or_default(path: &Path) -> LabelRulesFile {
    load(path).unwrap_or_default()
}

pub fn save(path: &Path, file: &LabelRulesFile) -> Result<(), LabelRulesError> {
    let text = toml::to_string_pretty(file)?;
    fs::write(path, text)?;
    Ok(())
}

/// ルールを1件追記して保存する。GUIの「追加」ボタンから呼ぶ。
pub fn add(path: &Path, rule: LabelRule) -> Result<LabelRulesFile, LabelRulesError> {
    let mut file = load(path)?;
    file.rules.push(rule);
    save(path, &file)?;
    Ok(file)
}

/// ルールをインデックス指定で1件削除して保存する（`color_rules::remove`と同じ理由で
/// 永続的なidは持たせていない）。
pub fn remove(path: &Path, index: usize) -> Result<LabelRulesFile, LabelRulesError> {
    let mut file = load(path)?;
    if index >= file.rules.len() {
        return Err(LabelRulesError::IndexOutOfRange(index));
    }
    file.rules.remove(index);
    save(path, &file)?;
    Ok(file)
}

/// 1件の新着メッセージにマッチするルールのラベル名一覧を返す（sync時に呼ぶ）。
pub fn matching_label_names<'a>(
    rules: &'a LabelRulesFile,
    subject: &str,
    from: &str,
    body: &str,
    has_attachment: bool,
) -> Vec<&'a str> {
    rules
        .rules
        .iter()
        .filter(|rule| rule.filter.matches(subject, from, body, has_attachment))
        .map(|rule| rule.label_name.as_str())
        .collect()
}

/// `matching_label_names`で得たラベル名を実際にメッセージへ付与する。ラベル名が
/// 解決できない（リネーム/削除済み）場合は静かにスキップする。
pub fn apply_matching_labels(
    conn: &Connection,
    message_id: i64,
    label_names: &[&str],
) -> Result<(), LabelRulesError> {
    for name in label_names {
        if let Some(label) = labels::find_by_name(conn, name)? {
            labels::set_on_message(conn, message_id, label.id, true)?;
        }
    }
    Ok(())
}

const BACKFILL_PAGE_SIZE: u32 = 1000;

/// 既存の全メッセージへ遡ってルールを適用する（「今すぐ適用」）。`reindex_all`と同じ
/// `messages::list_all_for_reindex`のidカーソルでバッチ処理し、1000万件規模でも
/// 一度に全件をメモリに載せない。
///
/// 差出人・件名・添付有無はDB行（`messages`テーブル）だけで判定できるため、
/// どのルールも`body_contains`を指定していなければMaildirは一切読まない。
/// 1件でも本文条件があれば、そのメッセージだけMaildirから読み直して本文を取り出す
/// （`reindex_all`同様、Maildirに実体が無い＝削除済み等のメッセージは静かにスキップする）。
///
/// 戻り値は新たに付与した(message, label)組の件数（既に付与済みだった分は
/// `INSERT OR IGNORE`で数えない。`db::labels::set_on_message`の戻り値からは
/// 増分がわからないため、事前に付いていたラベル集合と比較して数える）。
pub fn apply_all(
    conn: &Connection,
    maildir_base: &Path,
    rules: &LabelRulesFile,
) -> Result<usize, LabelRulesError> {
    if rules.rules.is_empty() {
        return Ok(0);
    }
    let needs_body = rules.rules.iter().any(|r| r.filter.body_contains.is_some());

    let mut applied = 0usize;
    let mut after_id = 0i64;
    loop {
        let page = messages::list_all_for_reindex(conn, after_id, BACKFILL_PAGE_SIZE)?;
        if page.is_empty() {
            break;
        }
        after_id = page.last().expect("page checked non-empty above").id;

        for message in &page {
            let from = format!(
                "{} {}",
                message.from_name.as_deref().unwrap_or(""),
                message.from_addr.as_deref().unwrap_or("")
            );
            let body = if needs_body {
                maildir::load(maildir_base, message.account_id, &message.uidl)
                    .ok()
                    .and_then(|raw| extract_plain_text_body(&raw))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let has_attachment = message.attachment_count > 0;

            let matched = matching_label_names(
                rules,
                message.subject.as_deref().unwrap_or(""),
                &from,
                &body,
                has_attachment,
            );
            if matched.is_empty() {
                continue;
            }

            let before = labels::for_message(conn, message.id)?;
            apply_matching_labels(conn, message.id, &matched)?;
            let after = labels::for_message(conn, message.id)?;
            applied += after.len().saturating_sub(before.len());
        }
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::{self as db_messages, NewMessage};
    use crate::db::open_in_memory;
    use tempfile::tempdir;

    fn rule(label_name: &str, filter: MessageFilter) -> LabelRule {
        LabelRule { label_name: label_name.to_string(), filter }
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("label_rules.toml");
        let file = load(&path).unwrap();
        assert!(file.rules.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("label_rules.toml");
        let file = LabelRulesFile {
            rules: vec![rule(
                "領収書",
                MessageFilter {
                    from_contains: Some("invoice@".to_string()),
                    has_attachment: Some(true),
                    ..Default::default()
                },
            )],
        };
        save(&path, &file).unwrap();
        assert_eq!(load(&path).unwrap(), file);
    }

    #[test]
    fn add_appends_and_remove_deletes_the_target_index() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("label_rules.toml");
        save(&path, &LabelRulesFile::default()).unwrap();

        add(&path, rule("A", MessageFilter { subject_contains: Some("x".into()), ..Default::default() })).unwrap();
        let file = add(&path, rule("B", MessageFilter { subject_contains: Some("y".into()), ..Default::default() }))
            .unwrap();
        assert_eq!(file.rules.len(), 2);

        let file = remove(&path, 0).unwrap();
        assert_eq!(file.rules, vec![rule("B", MessageFilter { subject_contains: Some("y".into()), ..Default::default() })]);
    }

    #[test]
    fn remove_out_of_range_index_errors_without_modifying_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("label_rules.toml");
        save(&path, &LabelRulesFile { rules: vec![rule("A", MessageFilter::default())] }).unwrap();

        assert!(remove(&path, 5).is_err());
        assert_eq!(load(&path).unwrap().rules.len(), 1);
    }

    #[test]
    fn load_or_default_falls_back_to_empty_on_malformed_toml() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("label_rules.toml");
        fs::write(&path, "not valid = [ toml").unwrap();

        assert!(load(&path).is_err());
        assert!(load_or_default(&path).rules.is_empty());
    }

    fn make_account(conn: &Connection) -> i64 {
        accounts::insert(
            conn,
            &NewAccount {
                name: "test",
                host: "pop.example.com",
                port: 995,
                username: "user",
                use_tls: true,
            },
        )
        .unwrap()
    }

    #[test]
    fn apply_all_labels_only_messages_matching_the_rule() {
        let conn = open_in_memory().unwrap();
        let maildir_dir = tempdir().unwrap();
        let account_id = make_account(&conn);
        let label_id = labels::insert(&conn, "領収書", "#C08A2E").unwrap();

        let matching = db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "u1",
                message_id_header: None,
                subject: Some("9月分領収書"),
                from_name: None,
                from_addr: Some("invoice@example.com"),
                to_addr: None,
                date_header: 100,
                size_bytes: 0,
                attachment_count: 1,
                preview: None,
            },
        )
        .unwrap()
        .unwrap();
        db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "u2",
                message_id_header: None,
                subject: Some("お知らせ"),
                from_name: None,
                from_addr: Some("news@example.com"),
                to_addr: None,
                date_header: 200,
                size_bytes: 0,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap();

        let rules = LabelRulesFile {
            rules: vec![rule(
                "領収書",
                MessageFilter {
                    subject_contains: Some("領収書".to_string()),
                    has_attachment: Some(true),
                    ..Default::default()
                },
            )],
        };

        let applied = apply_all(&conn, maildir_dir.path(), &rules).unwrap();
        assert_eq!(applied, 1);

        let matched_labels = labels::for_message(&conn, matching).unwrap();
        assert_eq!(matched_labels.len(), 1);
        assert_eq!(matched_labels[0].id, label_id);

        // 2回目は既に付与済みなので増分は0。
        assert_eq!(apply_all(&conn, maildir_dir.path(), &rules).unwrap(), 0);
    }

    #[test]
    fn apply_all_skips_rules_whose_label_no_longer_exists() {
        let conn = open_in_memory().unwrap();
        let maildir_dir = tempdir().unwrap();
        let account_id = make_account(&conn);

        db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "u1",
                message_id_header: None,
                subject: Some("hello"),
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 100,
                size_bytes: 0,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap();

        // "存在しない"ラベルは一度も作られていない。
        let rules = LabelRulesFile {
            rules: vec![rule(
                "存在しない",
                MessageFilter { subject_contains: Some("hello".to_string()), ..Default::default() },
            )],
        };

        assert_eq!(apply_all(&conn, maildir_dir.path(), &rules).unwrap(), 0);
    }

    #[test]
    fn apply_all_reads_maildir_only_when_a_rule_needs_the_body() {
        let conn = open_in_memory().unwrap();
        let maildir_dir = tempdir().unwrap();
        let account_id = make_account(&conn);
        labels::insert(&conn, "重要", "#C08A2E").unwrap();

        // Maildirに実体を置かない(=読もうとしたらエラーになる)まま、本文条件付きの
        // ルールを適用する。body_containsが無いルールのみなら実体不要のはず。
        let message_id = db_messages::insert_new(
            &conn,
            &NewMessage {
                account_id,
                uidl: "u1",
                message_id_header: None,
                subject: Some("critical alert"),
                from_name: None,
                from_addr: None,
                to_addr: None,
                date_header: 100,
                size_bytes: 0,
                attachment_count: 0,
                preview: None,
            },
        )
        .unwrap()
        .unwrap();

        let rules = LabelRulesFile {
            rules: vec![rule(
                "重要",
                MessageFilter { subject_contains: Some("critical".to_string()), ..Default::default() },
            )],
        };

        let applied = apply_all(&conn, maildir_dir.path(), &rules).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(labels::for_message(&conn, message_id).unwrap().len(), 1);
    }
}
