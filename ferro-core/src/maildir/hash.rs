//! ディレクトリ分散に使うハッシュ関数。
//!
//! ここで計算した値が既存メッセージの保存先ディレクトリを決めるため、
//! アルゴリズムを将来変更すると過去に保存したファイルが見つからなくなる。
//! std::collections::hash_map::DefaultHasher(SipHash)は安定性が保証されて
//! いないため使わず、仕様が固定されたFNV-1a(64bit)を自前で実装する。

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

pub(crate) fn fnv1a64(data: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    // FNV-1a公式テストベクタ（http://www.isthe.com/chongo/src/fnv/test_fnv.c）と
    // 一致することを確認し、実装の安定性を担保する。
    #[test]
    fn matches_known_fnv1a64_test_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }
}
