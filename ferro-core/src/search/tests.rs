use tempfile::tempdir;

use super::*;
use tantivy::schema::TEXT;

fn sample<'a>(id: i64, subject: &'a str, from: &'a str, body: &'a str) -> IndexableMessage<'a> {
    // 並び順のテスト(`search_orders_matches_by_newest_date_header_first`)以外は
    // 日付の値そのものを気にしないので、idをそのままdate_headerとしても使う
    // （idの大小関係と時系列を一致させておけば、他のテストの意図とも矛盾しない）。
    IndexableMessage {
        id,
        subject,
        from,
        body,
        date_header: id,
    }
}

fn sample_with_date<'a>(
    id: i64,
    subject: &'a str,
    from: &'a str,
    body: &'a str,
    date_header: i64,
) -> IndexableMessage<'a> {
    IndexableMessage { id, subject, from, body, date_header }
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

/// 「検索が単語検索っぽい」＝英数字は単語全体の完全一致でしかヒットしない、
/// という指摘への対応（`cjk_tokenizer`のドキュメント参照）。英数字もCJKと
/// 同じくバイグラム化したことで、単語の一部分だけの入力でもヒットするように
/// なったことを確認する。バイグラムはトークン化時の位置(position)込みで
/// フレーズクエリとして評価されるため、「crit」の3バイグラム(cr/ri/it)が
/// たまたまバラバラに存在するだけの無関係な文書までヒットする、ということは
/// 起きない（cranberry/riddle/fitnessはcr・ri・itをそれぞれ含むが、
/// 「crit」という並びでは一度も現れないためヒットしない）。
#[test]
fn english_substring_query_matches_part_of_a_word_but_not_scattered_fragments() {
    let index = SearchIndex::create_in_ram().unwrap();

    index
        .index_message(&sample(1, "Critical alert", "a a@example.com", "the service is critical"))
        .unwrap();
    index
        .index_message(&sample(
            2,
            "unrelated",
            "b b@example.com",
            "cranberry riddle fitness happen to contain the same fragments scattered apart",
        ))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("crit", 10).unwrap(), vec![1]);
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

/// 検索結果は関連度ではなく常に受信日時(date_header)の新しい順で返る
/// （「検索をした場合常に受信日付新しい順になるようにしてほしい」という指摘への対応）。
/// 件名一致(本来なら関連度が高いはずの文書)でも、本文一致で日付が新しい方が
/// 先に来ることを確認する。
#[test]
fn search_orders_matches_by_newest_date_header_first() {
    let index = SearchIndex::create_in_ram().unwrap();

    index
        .index_message(&sample_with_date(
            1,
            "apple",
            "a a@example.com",
            "a different body",
            /* date_header */ 1000,
        ))
        .unwrap();
    index
        .index_message(&sample_with_date(
            2,
            "unrelated subject",
            "b b@example.com",
            "mentions apple in the body",
            /* date_header */ 2000,
        ))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("apple", 10).unwrap(), vec![2, 1]);
}

/// マッチした文書がlimitを超える場合でも、常に一番新しいlimit件を返す
/// （関連度が高いが古い文書に押し出されて、新しいが関連度が低い文書が
/// 漏れることが無い）。
#[test]
fn search_returns_the_newest_matches_even_when_more_than_limit_match() {
    let index = SearchIndex::create_in_ram().unwrap();

    for i in 1..=5i64 {
        index
            .index_message(&sample_with_date(
                i,
                "alert",
                "monitor@example.com",
                "body",
                i * 1000,
            ))
            .unwrap();
    }
    index.commit().unwrap();

    assert_eq!(index.search("alert", 3).unwrap(), vec![5, 4, 3]);
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

/// 実際のNagios監視アラートに近いテキストに対する部分一致クエリ(「adeで検索しても
/// ヒットしない」の再現・回帰テスト)。件名・本文どちらも"srv-ade-w01"を含んでおり、
/// 短い部分文字列「ade」でもヒットすることを確認する。
#[test]
fn short_substring_query_matches_realistic_alert_subject_and_body() {
    let index = SearchIndex::create_in_ram().unwrap();
    index
        .index_message(&sample(
            1,
            "[AZURE_srv-ade-w01] found files: 0  critical",
            "monitor@example.com",
            "***** Nagios ***** Notification Type PROBLEM Service Ade lightfile_stg did not \
             created Host srv-ade-w01 Address 10.202.1",
        ))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("ade", 10).unwrap(), vec![1]);
}

/// 「srv-jpp-w02」を「srv-jpp-w」で検索してもヒットしない、という指摘の
/// 回帰テスト。クエリの最後の断片("w")が1文字だけになると、バイグラム
/// トークナイザはそれを単独のトークンとして扱う(2文字未満はバイグラムに
/// できないため)が、文書側の対応する語("w02")はバイグラム("w0","02")に
/// なっているため、単純な完全一致のフレーズクエリだと一致しなかった
/// （`build_word_query`のドキュメント参照。`PhrasePrefixQuery`で解決した）。
#[test]
fn query_truncated_mid_word_still_matches_via_prefix_on_the_last_fragment() {
    let index = SearchIndex::create_in_ram().unwrap();
    index
        .index_message(&sample(1, "Alert", "monitor@example.com", "Host srv-jpp-w02 is down"))
        .unwrap();
    index.commit().unwrap();

    assert_eq!(index.search("srv-jpp-w", 10).unwrap(), vec![1]);
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

/// 2つ目のプロセス（実際にはこのテスト内の2つ目の`SearchIndex`）が同じ
/// ディレクトリを開こうとした場合の回帰テスト。生のTantivyエラー
/// （"Could not acquire lock..."）のままだと、実際に別のFerroプロセス
/// （CLI/TUI/GUI）が起動中なのか、以前のプロセスが異常終了して
/// ロックファイルが残っているだけなのか区別できず対処のしようがない
/// （実際にTUIをテスト中に強制終了させたことでこの状態を踏み、無関係な
/// GUIが原因不明のパニックで起動できなくなった）。`WriterLockBusy`に
/// 読み替えて、両方の可能性と対処法を案内するようにした。
#[test]
fn open_or_create_reports_an_actionable_error_when_already_locked() {
    let dir = tempdir().unwrap();
    let _first = SearchIndex::open_or_create(dir.path()).unwrap();

    let err = match SearchIndex::open_or_create(dir.path()) {
        Ok(_) => panic!("expected opening an already-locked index to fail"),
        Err(e) => e,
    };
    assert!(
        matches!(err, SearchError::WriterLockBusy { .. }),
        "expected WriterLockBusy, got: {err:?}"
    );
    // パスと対処法（.lockファイルを削除しても安全）が案内に含まれること。
    let message = err.to_string();
    assert!(message.contains(&dir.path().display().to_string()));
    assert!(message.contains(".lock"));
}
