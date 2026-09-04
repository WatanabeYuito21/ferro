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
