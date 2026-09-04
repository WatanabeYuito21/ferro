use tempfile::tempdir;

use super::*;

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
