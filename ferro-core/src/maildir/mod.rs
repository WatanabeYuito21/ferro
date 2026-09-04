//! Maildir形式での生メール保存。1ファイル1メッセージ、
//! `cur/xx/yy/<account_id>-<uidl>.eml` にハッシュ分散して格納する。
//!
//! ファイルパスは account_id + uidl から決定的に導出できるため、DB側に
//! パスを保持する必要はない（`db::messages`のuidlさえあれば辿り着ける）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

mod hash;
use hash::fnv1a64;

/// 指定メッセージの保存先パスを計算する（ファイルの存在有無に関わらず決定的）。
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

/// 生メールを保存する。書き込み途中でクラッシュしても壊れたファイルが残らないよう、
/// 一時ファイルに書いてから最終パスへrenameする。
pub fn store(base_dir: &Path, account_id: i64, uidl: &str, raw: &[u8]) -> io::Result<PathBuf> {
    let path = message_path(base_dir, account_id, uidl);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }

    let tmp_path = path.with_extension("eml.tmp");
    fs::write(&tmp_path, raw)?;
    fs::rename(&tmp_path, &path)?;
    Ok(path)
}

pub fn load(base_dir: &Path, account_id: i64, uidl: &str) -> io::Result<Vec<u8>> {
    fs::read(message_path(base_dir, account_id, uidl))
}

pub fn exists(base_dir: &Path, account_id: i64, uidl: &str) -> bool {
    message_path(base_dir, account_id, uidl).is_file()
}

/// 保存済みメッセージを削除する（アカウント削除時のクリーンアップ用）。
/// 既に無い場合も成功扱いにする（削除したい状態には既になっているため）。
pub fn remove(base_dir: &Path, account_id: i64, uidl: &str) -> io::Result<()> {
    match fs::remove_file(message_path(base_dir, account_id, uidl)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// UIDLはPOP3仕様上ほぼ安全な文字集合(印字可能ASCIIかつ空白なし)だが、
/// ファイル名として不安全になりうる文字（パス区切りやWindowsの予約文字など）を
/// 念のため`%XX`にエスケープする。ファイル名からuidlへ戻す必要はない
/// （DB側の`messages.uidl`が正なので、ここは一方向でよい）。
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn message_path_is_deterministic_and_hash_distributed() {
        let base = Path::new("/maildir-base");
        let path_a = message_path(base, 1, "uidl-abc");
        let path_b = message_path(base, 1, "uidl-abc");
        assert_eq!(path_a, path_b);

        let components: Vec<_> = path_a
            .strip_prefix(base)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(components[0], "cur");
        assert_eq!(components[1].len(), 2);
        assert_eq!(components[2].len(), 2);
        assert_eq!(components[3], "1-uidl-abc.eml");
    }

    #[test]
    fn store_load_exists_roundtrip() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        assert!(!exists(base, 42, "uidl-1"));

        let raw = b"Subject: hello\r\n\r\nbody";
        let path = store(base, 42, "uidl-1", raw).unwrap();

        assert!(path.is_file());
        assert!(exists(base, 42, "uidl-1"));
        assert_eq!(load(base, 42, "uidl-1").unwrap(), raw);

        // 上書き保存も問題なく行える（再syncでの再取得を想定）。
        let updated = b"Subject: updated\r\n\r\nnew body";
        store(base, 42, "uidl-1", updated).unwrap();
        assert_eq!(load(base, 42, "uidl-1").unwrap(), updated);
    }

    #[test]
    fn different_accounts_or_uidls_do_not_collide() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        store(base, 1, "same-uidl", b"account 1").unwrap();
        store(base, 2, "same-uidl", b"account 2").unwrap();

        assert_eq!(load(base, 1, "same-uidl").unwrap(), b"account 1");
        assert_eq!(load(base, 2, "same-uidl").unwrap(), b"account 2");
    }

    #[test]
    fn unsafe_characters_in_uidl_are_sanitized_out_of_the_filename() {
        let dir = tempdir().unwrap();
        let base = dir.path();

        // POP3仕様上は想定しにくいが、パス区切りが混入しても
        // ディレクトリを脱走せず安全に保存・読み出しできることを確認する。
        let nasty_uidl = "../../etc/passwd";
        let path = store(base, 1, nasty_uidl, b"payload").unwrap();

        // cur/xx/yy/filename の4階層から変わらない(=脱走していない)こと、
        // かつファイル名自体にパス区切り文字が残っていないことを確認する。
        let relative = path.strip_prefix(base).unwrap();
        assert_eq!(relative.components().count(), 4);
        let filename = path.file_name().unwrap().to_string_lossy();
        assert!(!filename.contains('/') && !filename.contains('\\'));

        assert_eq!(load(base, 1, nasty_uidl).unwrap(), b"payload");
    }

    #[test]
    fn remove_deletes_the_file_and_is_idempotent() {
        let dir = tempdir().unwrap();
        let base = dir.path();
        store(base, 1, "u1", b"body").unwrap();
        assert!(exists(base, 1, "u1"));

        remove(base, 1, "u1").unwrap();
        assert!(!exists(base, 1, "u1"));

        // 既に無い状態でもう一度呼んでもエラーにならない。
        remove(base, 1, "u1").unwrap();
    }

    #[test]
    fn load_missing_message_returns_not_found_error() {
        let dir = tempdir().unwrap();
        let err = load(dir.path(), 1, "does-not-exist").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
