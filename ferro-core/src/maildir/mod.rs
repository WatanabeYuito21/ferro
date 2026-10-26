//! 生メールの保存。v0.0.40から、`<base_dir>/messages.db`（SQLite。メタデータの`ferro.db`とは
//! 別ファイル）に1通1行で保存する。キーは`account_id`+`uidl`で、DB側（`messages.uidl`）さえ
//! あれば辿り着ける点はファイル時代と同じ。
//!
//! それまでは1通1ファイル（`cur/xx/yy/<account_id>-<uidl>.eml`、ハッシュ分散）で保存して
//! いたが、実データ（7.6万通）でフォルダが6.6万個できて約330MBを占め（中身は143MB）、しかも
//! 初めて読むファイルは1通あたり約9ミリ秒（毎回のファイルオープンでウイルス対策の検査が
//! 走るとみられる）かかり、再構築やスレッドの後付けで全件を読むと10分以上かかっていた。
//! 1つのDBファイルに入れると、ディスクは中身と同程度になり、読み込みも桁違いに速い。
//!
//! メタデータの`ferro.db`と分けたのは、一覧・検索が使う方を小さいまま保つため（最適化の
//! VACUUMやバックアップが生メールの分まで重くならない）。
//!
//! 古いファイルは`migrate_files`が少しずつ`messages.db`へ移す（GUI/TUIの起動時に
//! バックグラウンドで）。移し終わるまでは、`load`/`exists`はDBに無ければファイルも見る。
//!
//! 接続は`base_dir`ごとにプロセス内で1本だけ開いて使い回す（全件を読む処理で毎回開き直すと
//! 遅いため）。SQLiteなので、別のFerroのプロセス（GUIとCLI等）から同時に開いてもよい。
//! テストビルドでは使い回さず毎回開く（Windowsでは開いたままの一時ディレクトリを消せないため）。

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

pub(crate) mod hash;
use hash::fnv1a64;

/// `base_dir`の中の保存先DBのファイル名。
pub const STORE_FILE: &str = "messages.db";

fn io_error(e: rusqlite::Error) -> io::Error {
    io::Error::other(e)
}

fn open_store(base_dir: &Path) -> io::Result<Connection> {
    fs::create_dir_all(base_dir)?;
    let conn = Connection::open(base_dir.join(STORE_FILE)).map_err(io_error)?;
    conn.busy_timeout(Duration::from_secs(10)).map_err(io_error)?;
    // テストビルドは呼び出しのたびに開き直すので、WALへの切り替えとディスクへの同期を省く
    // （毎回の切り替え・同期だけでテスト全体が数倍遅くなったため）。
    conn.execute_batch(if cfg!(test) {
        "PRAGMA synchronous = OFF;"
    } else {
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;"
    })
    .map_err(io_error)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS raw_messages (
             id         INTEGER PRIMARY KEY,
             account_id INTEGER NOT NULL,
             uidl       TEXT NOT NULL,
             raw        BLOB NOT NULL
         );
         CREATE UNIQUE INDEX IF NOT EXISTS idx_raw_messages_key ON raw_messages (account_id, uidl);",
    )
    .map_err(io_error)?;
    Ok(conn)
}

type Shared = Arc<Mutex<Connection>>;

/// `base_dir`の保存先DBへの接続（プロセス内で共有）を使って`f`を実行する。
fn with_store<T>(base_dir: &Path, f: impl FnOnce(&Connection) -> io::Result<T>) -> io::Result<T> {
    if cfg!(test) {
        return f(&open_store(base_dir)?);
    }
    static STORES: OnceLock<Mutex<HashMap<PathBuf, Shared>>> = OnceLock::new();
    let shared = {
        let mut stores = STORES.get_or_init(Default::default).lock().map_err(|_| io::Error::other("store map poisoned"))?;
        match stores.get(base_dir) {
            Some(shared) => Arc::clone(shared),
            None => {
                let shared = Arc::new(Mutex::new(open_store(base_dir)?));
                stores.insert(base_dir.to_path_buf(), Arc::clone(&shared));
                shared
            }
        }
    };
    let conn = shared.lock().map_err(|_| io::Error::other("store connection poisoned"))?;
    f(&conn)
}

fn load_from_store(conn: &Connection, account_id: i64, uidl: &str) -> io::Result<Option<Vec<u8>>> {
    conn.prepare_cached("SELECT raw FROM raw_messages WHERE account_id = ?1 AND uidl = ?2")
        .and_then(|mut stmt| stmt.query_row(params![account_id, uidl], |row| row.get(0)).optional())
        .map_err(io_error)
}

/// 生メールを保存する（同じキーがあれば置き換える）。
pub fn store(base_dir: &Path, account_id: i64, uidl: &str, raw: &[u8]) -> io::Result<()> {
    #[cfg(test)]
    tests::count_write(base_dir);
    with_store(base_dir, |conn| {
        conn.prepare_cached(
            "INSERT INTO raw_messages (account_id, uidl, raw) VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, uidl) DO UPDATE SET raw = excluded.raw",
        )
        .and_then(|mut stmt| stmt.execute(params![account_id, uidl, raw]))
        .map_err(io_error)?;
        Ok(())
    })
}

/// 保存済みの生メール。無ければ`NotFound`。
pub fn load(base_dir: &Path, account_id: i64, uidl: &str) -> io::Result<Vec<u8>> {
    if let Some(raw) = with_store(base_dir, |conn| load_from_store(conn, account_id, uidl))? {
        return Ok(raw);
    }
    // まだDBへ移していない古いファイル。読む直前に`migrate_files`が移して消した場合に
    // 備えて、ファイルが無ければもう一度DBを見る。
    match fs::read(message_path(base_dir, account_id, uidl)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => with_store(base_dir, |conn| {
            load_from_store(conn, account_id, uidl)?.ok_or(e)
        }),
        other => other,
    }
}

pub fn exists(base_dir: &Path, account_id: i64, uidl: &str) -> bool {
    let in_store = with_store(base_dir, |conn| {
        conn.prepare_cached("SELECT 1 FROM raw_messages WHERE account_id = ?1 AND uidl = ?2")
            .and_then(|mut stmt| stmt.exists(params![account_id, uidl]))
            .map_err(io_error)
    });
    matches!(in_store, Ok(true)) || message_path(base_dir, account_id, uidl).is_file()
}

/// 保存済みメッセージを削除する。既に無い場合も成功扱いにする（削除したい状態には既に
/// なっているため）。消した領域は次に保存するメールが使い回し、ファイル自体を縮めるのは
/// `vacuum`（設定画面の「ディスクを最適化」）。
pub fn remove(base_dir: &Path, account_id: i64, uidl: &str) -> io::Result<()> {
    // ファイルの削除もDBの接続を握ったまま行い、`migrate_files`が同じメールを移している
    // 途中に割り込まないようにする（消したはずのメールがDBへ移されて残らないように）。
    with_store(base_dir, |conn| {
        conn.prepare_cached("DELETE FROM raw_messages WHERE account_id = ?1 AND uidl = ?2")
            .and_then(|mut stmt| stmt.execute(params![account_id, uidl]))
            .map_err(io_error)?;
        match fs::remove_file(message_path(base_dir, account_id, uidl)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    })
}

/// 保存先DBを詰める（`maintenance::optimize`）。
pub fn vacuum(base_dir: &Path) -> io::Result<()> {
    with_store(base_dir, |conn| conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);").map_err(io_error))
}

/// 保存先DB（WAL等を含む）と、まだ移していない古いファイルの合計サイズ。
pub fn storage_size(base_dir: &Path) -> u64 {
    crate::maintenance::dir_size(base_dir)
}

/// v0.0.39までのファイルでの保存先（`cur/xx/yy/<account_id>-<uidl>.eml`）。今は`migrate_files`と
/// 移し終わる前の読み込みにだけ使う。
pub fn message_path(base_dir: &Path, account_id: i64, uidl: &str) -> PathBuf {
    let bucket_key = format!("{account_id}-{uidl}");
    let h = fnv1a64(bucket_key.as_bytes());
    let xx = (h & 0xff) as u8;
    let yy = ((h >> 8) & 0xff) as u8;

    base_dir
        .join("cur")
        .join(format!("{xx:02x}"))
        .join(format!("{yy:02x}"))
        .join(format!("{account_id}-{}.eml", sanitize_for_filename(uidl)))
}

/// UIDLのうちファイル名として不安全になりうる文字（パス区切りやWindowsの予約文字など）を
/// `%XX`にエスケープする（`%`自身もエスケープされるので、`unsanitize`で元に戻せる）。
fn sanitize_for_filename(uidl: &str) -> String {
    let mut out = String::with_capacity(uidl.len());
    for byte in uidl.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02x}"));
        }
    }
    out
}

fn unsanitize(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = name.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `<account_id>-<uidl>.eml`のファイル名からキーを取り出す。
fn parse_file_name(name: &str) -> Option<(i64, String)> {
    let stem = name.strip_suffix(".eml")?;
    let (account, uidl) = stem.split_once('-')?;
    Some((account.parse().ok()?, unsanitize(uidl)?))
}

/// まだDBへ移していない古いファイルを最大`limit`個、`messages.db`へ移す（移したファイルは消し、
/// 空になったフォルダも消す）。移した数を返し、0なら移し終わっている。何度呼んでも安全で、
/// 途中で止めても続きから再開できる（DBへの登録が済んでからファイルを消すため）。
///
/// ファイルの読み込み（初めて読むファイルは遅い）はDBの接続を握らずに行い、その間も
/// メールを開く等の操作を待たせない。DBへ入れる直前にファイルがまだあるかを確かめ、
/// その間に`remove`で消されたメールは移さない。DBに同じキーが既にあれば（移す前に
/// 同じメールを保存し直した等）DBの方を正とし、ファイルは捨てる。
pub fn migrate_files(base_dir: &Path, limit: usize) -> io::Result<usize> {
    let cur = base_dir.join("cur");
    let mut batch: Vec<(PathBuf, i64, String, Vec<u8>)> = Vec::new();
    let mut stray = Vec::new();
    'outer: for dir in sorted_subdirs(&cur)? {
        for sub in sorted_subdirs(&dir)? {
            let mut empty = true;
            for entry in fs::read_dir(&sub)? {
                let path = entry?.path();
                empty = false;
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                match parse_file_name(&name) {
                    // 読めないファイル（ウイルス対策が掴んでいる等）は今回は飛ばす。移せた分だけ
                    // 数えるので、読めないものしか残っていなければ0を返して終わる（次の起動で再試行）。
                    Some((account_id, uidl)) => match fs::read(&path) {
                        Ok(raw) => batch.push((path, account_id, uidl, raw)),
                        Err(e) => crate::logging::log_line(&format!(
                            "storage: could not read {} for migration: {e}",
                            path.display()
                        )),
                    },
                    // 書き込み途中で止まった`.eml.tmp`等。メールとしては不完全なので捨てる。
                    None => stray.push(path),
                }
                if batch.len() + stray.len() >= limit {
                    break 'outer;
                }
            }
            if empty {
                let _ = fs::remove_dir(&sub);
            }
        }
        let _ = fs::remove_dir(&dir); // 空なら消える（空でなければ失敗するだけ）
    }

    let moved = batch.len() + stray.len();
    with_store(base_dir, |conn| {
        let tx = conn.unchecked_transaction().map_err(io_error)?;
        for (path, account_id, uidl, raw) in &batch {
            if path.is_file() {
                tx.execute(
                    "INSERT OR IGNORE INTO raw_messages (account_id, uidl, raw) VALUES (?1, ?2, ?3)",
                    params![account_id, uidl, raw],
                )
                .map_err(io_error)?;
            }
        }
        tx.commit().map_err(io_error)?;
        for (path, ..) in &batch {
            remove_file_and_empty_parents(path, &cur);
        }
        for path in &stray {
            remove_file_and_empty_parents(path, &cur);
        }
        Ok(())
    })?;
    if moved == 0 {
        let _ = fs::remove_dir(&cur);
    }
    Ok(moved)
}

/// 古いファイルを全部移す（GUI/TUIの起動時にバックグラウンドのスレッドで呼ぶ）。
/// `migrate_files`を少しずつ繰り返し、合間に少し待ってディスクを占有しすぎないようにする。
/// 開始・終了・失敗を`ferro.log`に残す。移した数を返す。
pub fn migrate_all_files(base_dir: &Path) -> io::Result<usize> {
    if !base_dir.join("cur").exists() {
        return Ok(0);
    }
    crate::logging::log_line("storage: moving mail files into messages.db");
    let mut total = 0;
    loop {
        match migrate_files(base_dir, 200) {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(e) => {
                crate::logging::log_line(&format!("storage: migration stopped after {total} file(s): {e}"));
                return Err(e);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    crate::logging::log_line(&format!("storage: moved {total} file(s) into messages.db"));
    Ok(total)
}

fn sorted_subdirs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut dirs = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .collect::<Vec<_>>(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    dirs.sort();
    Ok(dirs)
}

fn remove_file_and_empty_parents(path: &Path, stop_at: &Path) {
    let _ = fs::remove_file(path);
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == stop_at || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tempfile::tempdir;

    /// `store`の呼び出し回数（`base_dir`ごと。並列に走る他のテストと混ざらないように）。
    fn writes() -> &'static Mutex<HashMap<PathBuf, usize>> {
        static WRITES: OnceLock<Mutex<HashMap<PathBuf, usize>>> = OnceLock::new();
        WRITES.get_or_init(Default::default)
    }

    pub(super) fn count_write(base_dir: &Path) {
        *writes().lock().unwrap().entry(base_dir.to_path_buf()).or_default() += 1;
    }

    /// テスト用: `base_dir`へ`store`した回数。
    pub(crate) fn write_count(base_dir: &Path) -> usize {
        writes().lock().unwrap().get(base_dir).copied().unwrap_or(0)
    }

    /// v0.0.39までの形式でファイルを置く（移行のテスト用）。
    fn store_as_file(base: &Path, account_id: i64, uidl: &str, raw: &[u8]) -> PathBuf {
        let path = message_path(base, account_id, uidl);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, raw).unwrap();
        path
    }

    #[test]
    fn store_load_exists_remove_roundtrip() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        assert!(!exists(base, 42, "uidl-1"));
        assert_eq!(load(base, 42, "uidl-1").unwrap_err().kind(), io::ErrorKind::NotFound);

        store(base, 42, "uidl-1", b"Subject: hello\r\n\r\nbody").unwrap();
        assert!(exists(base, 42, "uidl-1"));
        assert_eq!(load(base, 42, "uidl-1").unwrap(), b"Subject: hello\r\n\r\nbody");
        assert!(base.join(STORE_FILE).is_file());
        assert!(!base.join("cur").exists(), "new mail is not written as files");

        // 上書き保存（再syncでの再取得）と、アカウント・uidlごとの区別。
        store(base, 42, "uidl-1", b"updated").unwrap();
        store(base, 7, "uidl-1", b"other account").unwrap();
        assert_eq!(load(base, 42, "uidl-1").unwrap(), b"updated");
        assert_eq!(load(base, 7, "uidl-1").unwrap(), b"other account");

        remove(base, 42, "uidl-1").unwrap();
        assert!(!exists(base, 42, "uidl-1"));
        remove(base, 42, "uidl-1").unwrap();
        assert_eq!(write_count(base), 3);
    }

    #[test]
    fn old_files_are_read_until_migrated_and_then_moved_into_the_store() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        let nasty = "../a b%/c";
        let a = store_as_file(base, 1, "u1", b"one");
        store_as_file(base, 1, nasty, b"nasty");
        store_as_file(base, 2, "u1", b"two");
        // 移す前に同じメールを保存し直していたら、DBの方が正。
        store_as_file(base, 3, "dup", b"stale file");
        store(base, 3, "dup", b"fresh").unwrap();
        // 書き込み途中で止まった一時ファイルは捨てる。
        let tmp = a.with_extension("eml.tmp");
        fs::write(&tmp, b"partial").unwrap();

        assert!(exists(base, 1, nasty));
        assert_eq!(load(base, 1, nasty).unwrap(), b"nasty");

        let mut moved = 0;
        loop {
            let n = migrate_files(base, 2).unwrap();
            if n == 0 {
                break;
            }
            moved += n;
        }
        assert_eq!(moved, 5);
        assert!(!base.join("cur").exists(), "all files and folders are gone");
        assert_eq!(load(base, 1, "u1").unwrap(), b"one");
        assert_eq!(load(base, 1, nasty).unwrap(), b"nasty");
        assert_eq!(load(base, 2, "u1").unwrap(), b"two");
        assert_eq!(load(base, 3, "dup").unwrap(), b"fresh");
        assert_eq!(migrate_files(base, 10).unwrap(), 0);
    }

    #[test]
    fn remove_also_deletes_a_file_that_was_not_migrated_yet() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        store_as_file(base, 1, "u1", b"one");
        remove(base, 1, "u1").unwrap();
        assert!(!exists(base, 1, "u1"));
        assert_eq!(migrate_files(base, 10).unwrap(), 0);
        assert_eq!(load(base, 1, "u1").unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn file_names_roundtrip_through_sanitize() {
        for uidl in ["plain-1_2.3", "../../etc/passwd", "100%", "a b\\c:d"] {
            let name = format!("5-{}.eml", sanitize_for_filename(uidl));
            assert!(!name.contains('/') && !name.contains('\\'));
            assert_eq!(parse_file_name(&name), Some((5, uidl.to_string())));
        }
        assert_eq!(parse_file_name("5-x.eml.tmp"), None);
        assert_eq!(parse_file_name("notanumber-x.eml"), None);
    }

    #[test]
    fn vacuum_shrinks_the_store_after_removing_mail() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        let raw = vec![b'x'; 64 * 1024];
        for i in 0..50 {
            store(base, 1, &i.to_string(), &raw).unwrap();
        }
        vacuum(base).unwrap();
        let full = storage_size(base);
        for i in 0..50 {
            remove(base, 1, &i.to_string()).unwrap();
        }
        vacuum(base).unwrap();
        assert!(storage_size(base) < full / 4, "{} -> {}", full, storage_size(base));
    }
}
