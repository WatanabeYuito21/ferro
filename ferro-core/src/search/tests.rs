use tempfile::tempdir;

use super::*;
use tantivy::schema::TEXT;

fn sample<'a>(id: i64, subject: &'a str, from: &'a str, body: &'a str) -> IndexableMessage<'a> {
    IndexableMessage {
        id,
        subject,
        from,
        body,
    }
}

#[test]
fn index_and_search_roundtrip() {
    let index = SearchIndex::create_in_ram().unwrap();

    index
        .index_message(&sample(
            1,
            "Hello world",
            "Alice alice@example.com",
            "this is the body text",
        ))
        .unwrap();
    index
        .index_message(&sample(
            2,
            "Something else",
            "Bob bob@example.com",
            "unrelated content",
        ))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("hello", 10).unwrap(), vec![1]);
    assert_eq!(index.search("alice", 10).unwrap(), vec![1]);
    assert_eq!(index.search("unrelated", 10).unwrap(), vec![2]);
    assert!(index.search("nonexistent-term", 10).unwrap().is_empty());
}

/// 日本語は分かち書きされないため、標準の空白区切りトークナイザでは
/// 件名・本文がほぼ1つの巨大トークンになり実質検索できない。
/// CJKバイグラムトークナイザにより部分一致で検索できることを確認する。
#[test]
fn japanese_text_is_searchable_by_substring() {
    let index = SearchIndex::create_in_ram().unwrap();

    index
        .index_message(&sample(
            1,
            "明日の会議について",
            "田中太郎 tanaka@example.com",
            "資料を添付しましたのでご確認ください。全文検索のテストです。",
        ))
        .unwrap();
    index
        .index_message(&sample(
            2,
            "旅行の計画",
            "鈴木花子 suzuki@example.com",
            "来月の旅行先について相談したいことがあります。",
        ))
        .unwrap();
    index.commit().unwrap();

    // 件名中の部分文字列（分かち書き単位ではなく、単なる部分一致）でヒットする。
    assert_eq!(index.search("会議", 10).unwrap(), vec![1]);
    // 本文中の部分文字列。
    assert_eq!(index.search("全文検索", 10).unwrap(), vec![1]);
    assert_eq!(index.search("旅行", 10).unwrap(), vec![2]);
    // 差出人の表示名でもヒットする。
    assert_eq!(index.search("田中", 10).unwrap(), vec![1]);
    assert!(index.search("存在しない単語", 10).unwrap().is_empty());
}

/// `QueryParser`はデフォルトだとOR結合で、CJKバイグラムトークナイザは
/// クエリ自体も複数の2文字片に分割するため、OR結合のままだと「クエリの
/// バイグラムのうちどれか1つでも含む文書」までヒットしてノイズだらけになる
/// （実際に「検索精度が低い」という形で踏んだ）。`set_conjunction_by_default`で
/// AND結合にし、クエリを構成する全バイグラムを含む文書だけがヒットすることを確認する。
#[test]
fn multi_bigram_query_requires_all_fragments_and_avoids_noisy_partial_matches() {
    let index = SearchIndex::create_in_ram().unwrap();

    index
        .index_message(&sample(1, "全文検索のテストです", "a a@example.com", "本文"))
        .unwrap();
    // 「検索」というバイグラムだけは共通するが、「全文検索」というクエリ全体とは
    // 無関係な文書（OR結合だとこれもヒットしてしまう）。
    index
        .index_message(&sample(2, "検索窓口のご案内", "b b@example.com", "本文"))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("全文検索", 10).unwrap(), vec![1]);
}

/// 件名・差出人の一致を本文一致より優先するフィールドブースト
/// (`set_field_boost`)により、件名にクエリを含む文書が上位に出ることを確認する。
#[test]
fn subject_matches_rank_above_body_only_matches() {
    let index = SearchIndex::create_in_ram().unwrap();

    index
        .index_message(&sample(1, "unrelated subject", "a a@example.com", "mentions apple in the body"))
        .unwrap();
    index
        .index_message(&sample(2, "apple", "b b@example.com", "a different body"))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("apple", 10).unwrap(), vec![2, 1]);
}

#[test]
fn hyphenated_hostname_query_matches_hyphenated_hostname_in_body() {
    let index = SearchIndex::create_in_ram().unwrap();
    index
        .index_message(&sample(1, "Alert", "monitor@example.com", "Host SRV-ADE-W01 is down"))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("SRV-ADE-W01", 10).unwrap(), vec![1]);
}

#[test]
fn reindexing_same_id_replaces_previous_document() {
    let index = SearchIndex::create_in_ram().unwrap();

    index
        .index_message(&sample(1, "Old subject", "a a@example.com", "old body"))
        .unwrap();
    index.commit().unwrap();
    assert_eq!(index.search("old", 10).unwrap(), vec![1]);

    index
        .index_message(&sample(1, "New subject", "a a@example.com", "new body"))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("new", 10).unwrap(), vec![1]);
    assert!(index.search("old", 10).unwrap().is_empty());
}

#[test]
fn delete_message_removes_only_the_target_document() {
    let index = SearchIndex::create_in_ram().unwrap();
    index
        .index_message(&sample(1, "keep me", "a a@example.com", "body"))
        .unwrap();
    index
        .index_message(&sample(2, "delete me", "b b@example.com", "body"))
        .unwrap();
    index.commit().unwrap();

    index.delete_message(2).unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("keep", 10).unwrap(), vec![1]);
    assert!(index.search("delete", 10).unwrap().is_empty());
}

#[test]
fn clear_removes_all_documents() {
    let index = SearchIndex::create_in_ram().unwrap();
    index
        .index_message(&sample(1, "subject", "from", "body"))
        .unwrap();
    index.commit().unwrap();
    assert_eq!(index.search("subject", 10).unwrap(), vec![1]);

    index.clear().unwrap();
    assert!(index.search("subject", 10).unwrap().is_empty());
}

/// CLAUDE.mdの方針: 索引は完全にSQLite/Maildirから再構築可能な派生キャッシュ。
/// スキーマ変更で既存インデックスと不一致になったら、自動的に削除して作り直す。
#[test]
fn reopening_with_incompatible_schema_recreates_the_index() {
    let dir = tempdir().unwrap();

    let mut builder = Schema::builder();
    builder.add_text_field("something_else", TEXT);
    let old_schema = builder.build();
    Index::open_or_create(open_mmap_dir(dir.path()).unwrap(), old_schema).unwrap();

    let index = SearchIndex::open_or_create(dir.path()).unwrap();
    index
        .index_message(&sample(1, "subject", "from", "body"))
        .unwrap();
    index.commit().unwrap();
    assert_eq!(index.search("subject", 10).unwrap(), vec![1]);
}
