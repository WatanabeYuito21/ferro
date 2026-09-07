//! 特定の文字列を含むメッセージの一覧表示を色分けするルールをTOMLファイルで管理する。
//!
//! `color_rules.toml`が正の情報源。`account_config`と違い、パスワードやkeyring/
//! Maildirとの紐付けが無い純粋な表示設定なので、DBテーブルへのreconcile・内部id
//! 維持は不要（GUIはこのファイルを直接読み書きするだけでよい）。実際のマッチ判定
//! （件名/差出人/本文プレビューに含まれるか）はフロントエンド側（Svelte）が行う。
//! 仮想スクロールで画面に見えている行だけを判定すればよいため、1000万件規模の
//! メールボックスでも安全（CLAUDE.mdの「起動時に何も舐めない」方針とは別に、
//! 一覧表示のたびに全件を舐める設計にはならない）。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ColorRule {
    /// 大文字小文字を区別しない部分一致。件名・差出人（表示名・アドレス）・
    /// 本文プレビューのいずれかに含まれていればマッチする。
    pub pattern: String,
    /// CSSの色として解釈される文字列（例: "#b00020"）。値の妥当性はここでは
    /// 検証しない（フロントの`<input type="color">`が常に妥当な値を渡す前提。
    /// 手編集で不正な値が入ってもCSS側で無視されるだけで実害が無いため）。
    pub color: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ColorRulesFile {
    /// 先頭から順に評価し、最初にマッチしたルールの色を採用する
    /// （フロント側の`matchColor`参照）。
    #[serde(rename = "rule", default)]
    pub rules: Vec<ColorRule>,
}

#[derive(Debug, thiserror::Error)]
pub enum ColorRulesError {
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
}

/// 設定ファイルを読み込む。存在しない場合は空扱い（このファイルはオプション機能
/// なので、無いのが正常な状態。`account_config::load`と同じ理由）。
pub fn load(path: &Path) -> Result<ColorRulesFile, ColorRulesError> {
    if !path.exists() {
        return Ok(ColorRulesFile::default());
    }
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|source| ColorRulesError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

pub fn save(path: &Path, file: &ColorRulesFile) -> Result<(), ColorRulesError> {
    let text = toml::to_string_pretty(file)?;
    fs::write(path, text)?;
    Ok(())
}

/// ルールを1件追記して保存する。GUIの「追加」ボタンから呼ぶ。
pub fn add(path: &Path, rule: ColorRule) -> Result<ColorRulesFile, ColorRulesError> {
    let mut file = load(path)?;
    file.rules.push(rule);
    save(path, &file)?;
    Ok(file)
}

/// ルールをインデックス指定で1件削除して保存する。ルールに永続的なidは無い
/// （`accounts.toml`のnameのような自然キーが無いため、GUIが表示している
/// 配列の位置をそのまま使う。手編集との競合はaccounts.tomlほど気にしなくてよい:
/// 削除に失敗してもメッセージが消えるような取り返しのつかない操作ではないため）。
pub fn remove(path: &Path, index: usize) -> Result<ColorRulesFile, ColorRulesError> {
    let mut file = load(path)?;
    if index >= file.rules.len() {
        return Err(ColorRulesError::IndexOutOfRange(index));
    }
    file.rules.remove(index);
    save(path, &file)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn rule(pattern: &str, color: &str) -> ColorRule {
        ColorRule {
            pattern: pattern.to_string(),
            color: color.to_string(),
        }
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("color_rules.toml");
        let file = load(&path).unwrap();
        assert!(file.rules.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("color_rules.toml");
        let file = ColorRulesFile {
            rules: vec![rule("critical", "#b00020"), rule("PROBLEM", "#b00020")],
        };
        save(&path, &file).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, file);
    }

    #[test]
    fn add_appends_and_preserves_existing_order() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("color_rules.toml");
        save(&path, &ColorRulesFile { rules: vec![rule("first", "#111111")] }).unwrap();

        let file = add(&path, rule("second", "#222222")).unwrap();
        assert_eq!(file.rules, vec![rule("first", "#111111"), rule("second", "#222222")]);
        assert_eq!(load(&path).unwrap(), file);
    }

    #[test]
    fn remove_deletes_only_the_target_index() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("color_rules.toml");
        save(
            &path,
            &ColorRulesFile {
                rules: vec![rule("first", "#111111"), rule("second", "#222222")],
            },
        )
        .unwrap();

        let file = remove(&path, 0).unwrap();
        assert_eq!(file.rules, vec![rule("second", "#222222")]);
        assert_eq!(load(&path).unwrap(), file);
    }

    #[test]
    fn remove_out_of_range_index_errors_without_modifying_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("color_rules.toml");
        save(&path, &ColorRulesFile { rules: vec![rule("first", "#111111")] }).unwrap();

        assert!(remove(&path, 5).is_err());
        assert_eq!(load(&path).unwrap().rules, vec![rule("first", "#111111")]);
    }
}
