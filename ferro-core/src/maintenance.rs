//! ディスクの最適化（設定画面の「今すぐ最適化する」、`ferro optimize`）。生メールの保存先
//! （`maildir`の`messages.db`、v0.0.40）も同じく詰める。
//!
//! メールを消してもSQLiteのファイルは縮まず（空いたページは再利用されるだけ）、検索
//! インデックスも削除した文書を「削除済み」の印として持ち続ける（領域はセグメントの
//! マージまで回収されない）。保持期間の整理（`message_actions::purge_expired_batch`）で
//! 大量に消した後などに、両方を詰めてディスクを取り戻す。
//!
//! 検索インデックスはセグメントを1つにマージしてから不要になったファイルを消し、DBは
//! `VACUUM`してからWALを切り詰める。統計（`ANALYZE`/`PRAGMA optimize`）は取らない
//! （`EXPLAIN QUERY PLAN`で確かめたインデックスの選び方が統計次第で変わりうるため）。
//! どちらも一時的に元と同じくらいの空き容量が要る。

use std::fs;
use std::path::Path;

use crate::db::Connection;
use crate::search::{SearchError, SearchIndex};

#[derive(Debug, thiserror::Error)]
pub enum MaintenanceError {
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error(transparent)]
    Search(#[from] SearchError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// 最適化の前後のサイズ（バイト）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OptimizeReport {
    pub db_before: u64,
    pub db_after: u64,
    /// 生メールの保存先（`maildir`）。
    pub mail_before: u64,
    pub mail_after: u64,
    pub index_before: u64,
    pub index_after: u64,
}

impl OptimizeReport {
    /// 取り戻せたバイト数（増えた場合は0）。
    pub fn freed(&self) -> u64 {
        (self.db_before + self.mail_before + self.index_before)
            .saturating_sub(self.db_after + self.mail_after + self.index_after)
    }
}

/// 検索インデックスとDBを最適化する。`conn`は書き込み用の接続（`VACUUM`は他の書き込みと
/// 同時にはできないので、呼び出し側は同期と同じロックを取ってから渡す）。再構築中なら
/// 検索インデックスの最適化はせずエラーにする（`SearchError::RebuildInProgress`）。
pub fn optimize(
    conn: &Connection,
    db_path: &Path,
    maildir_base: &Path,
    index: &SearchIndex,
    index_dir: &Path,
) -> Result<OptimizeReport, MaintenanceError> {
    let mut report = OptimizeReport {
        db_before: db_size(db_path),
        mail_before: crate::maildir::storage_size(maildir_base),
        index_before: dir_size(index_dir),
        ..Default::default()
    };
    {
        // 再構築と同時に走らないよう、再構築と同じ印を立てる（起動時の拾い直しも止まる）。
        let _guard = index.begin_rebuild()?;
        index.optimize()?;
    }
    crate::logging::log_line("maintenance: search index merged");
    conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
    crate::logging::log_line("maintenance: database vacuumed");
    crate::maildir::vacuum(maildir_base)?;
    crate::logging::log_line("maintenance: mail store vacuumed");
    report.db_after = db_size(db_path);
    report.mail_after = crate::maildir::storage_size(maildir_base);
    report.index_after = dir_size(index_dir);
    Ok(report)
}

/// DBファイルとWAL・共有メモリのファイルの合計。
fn db_size(db_path: &Path) -> u64 {
    let mut total = file_size(db_path);
    for suffix in ["-wal", "-shm"] {
        let mut path = db_path.as_os_str().to_owned();
        path.push(suffix);
        total += file_size(Path::new(&path));
    }
    total
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

pub(crate) fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => dir_size(&entry.path()),
            Ok(_) => entry.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::IndexableMessage;
    use tempfile::tempdir;

    #[test]
    fn optimize_shrinks_the_database_after_deletes_and_keeps_the_index_searchable() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("ferro.db");
        let conn = crate::db::open(&db_path).unwrap();
        conn.execute_batch("CREATE TABLE filler (data BLOB); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) INSERT INTO filler SELECT zeroblob(4096) FROM n;")
            .unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); DELETE FROM filler;").unwrap();

        let index = SearchIndex::create_in_ram().unwrap();
        for id in 1..=3 {
            index
                .index_message(&IndexableMessage { id, subject: "障害 発生", from: "", body: "", date_header: id })
                .unwrap();
            index.commit().unwrap();
        }
        index.delete_message(2).unwrap();
        index.commit().unwrap();

        let mail = dir.path().join("maildir");
        crate::maildir::store(&mail, 1, "u1", &vec![b'x'; 256 * 1024]).unwrap();
        crate::maildir::remove(&mail, 1, "u1").unwrap();
        let report = optimize(&conn, &db_path, &mail, &index, &dir.path().join("no-index-dir")).unwrap();
        assert!(report.mail_after < report.mail_before, "{report:?}");
        assert!(report.db_after < report.db_before / 2, "{report:?}");
        assert!(report.freed() > 0);
        assert_eq!(index.search("障害", 10).unwrap(), vec![3, 1]);
        assert_eq!(index.num_docs(), 2);

        // 再構築中は最適化しない。
        let _guard = index.begin_rebuild().unwrap();
        assert!(matches!(
            optimize(&conn, &db_path, &mail, &index, dir.path()),
            Err(MaintenanceError::Search(SearchError::RebuildInProgress))
        ));
    }
}
