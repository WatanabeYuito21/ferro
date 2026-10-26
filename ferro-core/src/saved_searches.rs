//! 保存した検索（スマートフォルダ）。検索欄の文字列（`search::query::parse`の書式）に
//! 名前を付けて`saved_searches.toml`に保存し、GUI/TUIのサイドバーから1回で開けるようにする。
//!
//! `color_rules.toml`/`label_rules.toml`と同じくファイルが正の情報源で、DBテーブルは無い。
//! 中身は検索の文字列だけなので、開くたびに普通の検索として実行する（結果を保存しない）。
//! サイドバーに出す未読数も`unread_count`でその都度数える。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::db::Connection;
use crate::message_search::{self, MessageSearchError};
use crate::search::SearchIndex;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SavedSearch {
    pub name: String,
    /// 検索欄に入れる文字列（`from:` `is:unread`等の書式も含む）。
    pub query: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SavedSearchesFile {
    #[serde(rename = "search", default)]
    pub searches: Vec<SavedSearch>,
}

#[derive(Debug, thiserror::Error)]
pub enum SavedSearchesError {
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
    #[error("saved search index {0} out of range")]
    IndexOutOfRange(usize),
    #[error("名前と検索条件の両方を入力してください")]
    Empty,
}

/// 無ければ空扱い（`color_rules::load`と同じ）。
pub fn load(path: &Path) -> Result<SavedSearchesFile, SavedSearchesError> {
    if !path.exists() {
        return Ok(SavedSearchesFile::default());
    }
    let text = fs::read_to_string(path)?;
    toml::from_str(&text).map_err(|source| SavedSearchesError::Parse { path: path.to_path_buf(), source })
}

pub fn save(path: &Path, file: &SavedSearchesFile) -> Result<(), SavedSearchesError> {
    fs::write(path, toml::to_string_pretty(file)?)?;
    Ok(())
}

/// 追加する。同じ名前が既にあれば、その検索条件を置き換える（並び順は保つ）。
pub fn add(path: &Path, search: SavedSearch) -> Result<SavedSearchesFile, SavedSearchesError> {
    let search = SavedSearch { name: search.name.trim().to_string(), query: search.query.trim().to_string() };
    if search.name.is_empty() || search.query.is_empty() {
        return Err(SavedSearchesError::Empty);
    }
    let mut file = load(path)?;
    match file.searches.iter_mut().find(|s| s.name == search.name) {
        Some(existing) => existing.query = search.query,
        None => file.searches.push(search),
    }
    save(path, &file)?;
    Ok(file)
}

/// 番号で1件削除する（`color_rules::remove`と同じく永続的なidは持たせていない）。
pub fn remove(path: &Path, index: usize) -> Result<SavedSearchesFile, SavedSearchesError> {
    let mut file = load(path)?;
    if index >= file.searches.len() {
        return Err(SavedSearchesError::IndexOutOfRange(index));
    }
    file.searches.remove(index);
    save(path, &file)?;
    Ok(file)
}

/// サイドバーに出す未読数を数える上限（これ以上は「999+」のように出す）。
pub const UNREAD_COUNT_CAP: usize = 1000;

/// `query`に合う未読メッセージの件数（`UNREAD_COUNT_CAP`まで）。検索条件に`is:unread`を
/// 足して実行するのと同じ（全文検索の語とDB側の条件が両方ある場合の取りこぼし方も同じ。
/// `message_search`参照）。`is:read`を指定した検索なら常に0。
pub fn unread_count(
    conn: &Connection,
    index: &SearchIndex,
    query: &str,
    utc_offset_seconds: i64,
) -> Result<usize, MessageSearchError> {
    let mut parsed = crate::search::query::parse(query, utc_offset_seconds);
    if parsed.is_empty() || parsed.is_read == Some(true) {
        return Ok(0);
    }
    parsed.is_read = Some(false);
    Ok(message_search::search(conn, index, &parsed, UNREAD_COUNT_CAP, crate::db::now_unix())?.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::accounts::{self, NewAccount};
    use crate::db::messages::{self, NewMessage};
    use crate::db::open_in_memory;
    use crate::search::IndexableMessage;
    use tempfile::tempdir;

    fn search(name: &str, query: &str) -> SavedSearch {
        SavedSearch { name: name.to_string(), query: query.to_string() }
    }

    #[test]
    fn add_replaces_a_search_with_the_same_name_and_remove_deletes_by_index() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("saved_searches.toml");
        assert!(load(&path).unwrap().searches.is_empty());

        add(&path, search("障害", "subject:障害 is:unread")).unwrap();
        add(&path, search(" 請求 ", " 請求書 ")).unwrap();
        let file = add(&path, search("障害", "subject:障害")).unwrap();
        assert_eq!(file.searches, vec![search("障害", "subject:障害"), search("請求", "請求書")]);
        assert_eq!(load(&path).unwrap(), file);

        assert!(matches!(add(&path, search("", "x")), Err(SavedSearchesError::Empty)));
        assert!(matches!(remove(&path, 2), Err(SavedSearchesError::IndexOutOfRange(2))));
        assert_eq!(remove(&path, 0).unwrap().searches, vec![search("請求", "請求書")]);
    }

    #[test]
    fn unread_count_counts_only_unread_matches() {
        let conn = open_in_memory().unwrap();
        let index = SearchIndex::create_in_ram().unwrap();
        let account_id = accounts::insert(
            &conn,
            &NewAccount { name: "a", host: "pop.example.com", port: 995, username: "u", use_tls: true },
        )
        .unwrap();
        let mut ids = Vec::new();
        for (i, subject) in ["障害 発生", "障害 復旧", "定期報告"].iter().enumerate() {
            let id = messages::insert_new(
                &conn,
                &NewMessage {
                    account_id,
                    uidl: &i.to_string(),
                    message_id_header: None,
                    subject: Some(subject),
                    from_name: None,
                    from_addr: None,
                    to_addr: None,
                    date_header: 1000 + i as i64,
                    size_bytes: 0,
                    attachment_count: 0,
                    preview: None,
                },
            )
            .unwrap()
            .unwrap();
            index
                .index_message(&IndexableMessage { id, subject, from: "", body: "", date_header: 1000 + i as i64 })
                .unwrap();
            ids.push(id);
        }
        index.commit().unwrap();
        messages::set_read(&conn, ids[1], true).unwrap();

        assert_eq!(unread_count(&conn, &index, "障害", 0).unwrap(), 1);
        assert_eq!(unread_count(&conn, &index, "in:inbox", 0).unwrap(), 2);
        assert_eq!(unread_count(&conn, &index, "障害 is:read", 0).unwrap(), 0);
        assert_eq!(unread_count(&conn, &index, "  ", 0).unwrap(), 0);
    }
}
