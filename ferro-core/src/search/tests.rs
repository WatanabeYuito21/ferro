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
    assert_eq!(index.num_docs(), 0);
}

/// `clear`は「その時点までに追加された文書」だけを消し、後から追加された文書は
/// 残す（再構築と並行してsyncが新着を投入しても、その新着が消えない）。
/// 未commitの文書もclear前に追加されていれば消える。
#[test]
fn clear_only_removes_documents_added_before_it() {
    let index = SearchIndex::create_in_ram().unwrap();
    index.index_message(&sample(1, "committed", "from", "body")).unwrap();
    index.commit().unwrap();
    index.index_message(&sample(2, "pending", "from", "body")).unwrap();

    index.clear().unwrap();
    index.index_message(&sample(3, "after", "from", "body")).unwrap();
    index.commit().unwrap();

    assert!(index.search("committed", 10).unwrap().is_empty());
    assert!(index.search("pending", 10).unwrap().is_empty());
    assert_eq!(index.search("after", 10).unwrap(), vec![3]);
}

/// writerの作り直しに失敗してスロットが空になった後でも、次の書き込みで
/// 作り直しを再試行して復帰する（以前は`WriterUnavailable`のまま二度と
/// 投入できず、再構築が0件のまま止まった）。
#[test]
fn empty_writer_slot_is_recreated_on_next_write() {
    let index = SearchIndex::create_in_ram().unwrap();
    *index.writer.lock().unwrap() = None;

    index.index_message(&sample(1, "recovered", "from", "body")).unwrap();
    index.commit().unwrap();
    assert_eq!(index.search("recovered", 10).unwrap(), vec![1]);
}

/// 次に作られる`.pos`ファイルの書き込みオープンを1回だけ`PermissionDenied`で失敗させる
/// ディレクトリ。Windows実機でセグメントファイルの作成が拒否されてtantivyの
/// ワーカースレッドが死んだ状況を再現する。
#[derive(Clone, Debug)]
struct FlakyDirectory {
    inner: tantivy::directory::RamDirectory,
    fail_next_pos_write: std::sync::Arc<AtomicBool>,
}

impl tantivy::Directory for FlakyDirectory {
    fn get_file_handle(
        &self,
        path: &Path,
    ) -> std::result::Result<std::sync::Arc<dyn tantivy::directory::FileHandle>, tantivy::directory::error::OpenReadError>
    {
        self.inner.get_file_handle(path)
    }
    fn delete(&self, path: &Path) -> std::result::Result<(), tantivy::directory::error::DeleteError> {
        self.inner.delete(path)
    }
    fn exists(&self, path: &Path) -> std::result::Result<bool, tantivy::directory::error::OpenReadError> {
        self.inner.exists(path)
    }
    fn open_write(
        &self,
        path: &Path,
    ) -> std::result::Result<tantivy::directory::WritePtr, tantivy::directory::error::OpenWriteError> {
        if path.extension().is_some_and(|ext| ext == "pos") && self.fail_next_pos_write.swap(false, Ordering::SeqCst) {
            return Err(tantivy::directory::error::OpenWriteError::wrap_io_error(
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                path.to_path_buf(),
            ));
        }
        self.inner.open_write(path)
    }
    fn atomic_read(&self, path: &Path) -> std::result::Result<Vec<u8>, tantivy::directory::error::OpenReadError> {
        self.inner.atomic_read(path)
    }
    fn atomic_write(&self, path: &Path, data: &[u8]) -> std::io::Result<()> {
        self.inner.atomic_write(path, data)
    }
    fn sync_directory(&self) -> std::io::Result<()> {
        self.inner.sync_directory()
    }
    fn watch(&self, watch_callback: tantivy::directory::WatchCallback) -> tantivy::Result<tantivy::directory::WatchHandle> {
        self.inner.watch(watch_callback)
    }
}

/// writerのワーカースレッドが死んで`index_message`（`add_document`）が失敗したら、
/// その場でwriterを作り直し、次の投入から元どおり使えるようになる。以前は
/// 作り直しがcommit失敗時だけだったため、以後の`index_message`が全て
/// "index writer was killed"で失敗し続け、再構築が0件のまま止まった。
#[test]
fn index_message_recreates_the_writer_after_its_worker_dies() {
    let fail_next_pos_write = std::sync::Arc::new(AtomicBool::new(false));
    let dir = FlakyDirectory {
        inner: tantivy::directory::RamDirectory::create(),
        fail_next_pos_write: fail_next_pos_write.clone(),
    };
    let (schema, fields) = build_schema();
    let index = Index::create(dir, schema, tantivy::IndexSettings::default()).unwrap();
    let index = SearchIndex::from_index(index, fields).unwrap();

    // 次のセグメントの作成を失敗させ、ワーカーが死んで`index_message`が
    // 失敗するまで投入する（ワーカーは非同期に死ぬので、最初の数件は通りうる）。
    fail_next_pos_write.store(true, Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut id = 1;
    while index.index_message(&sample(id, "lost", "from", "body")).is_ok() {
        assert!(std::time::Instant::now() < deadline, "the index writer worker never died");
        std::thread::sleep(Duration::from_millis(10));
        id += 1;
    }

    // 作り直されたwriterで、リトライ無しにそのまま投入・commitできる。
    index.index_message(&sample(1000, "recovered", "from", "body")).unwrap();
    index.commit().unwrap();
    assert_eq!(index.search("recovered", 10).unwrap(), vec![1000]);
}

#[test]
fn begin_rebuild_rejects_a_second_rebuild_until_the_first_ends() {
    let index = SearchIndex::create_in_ram().unwrap();
    let guard = index.begin_rebuild().unwrap();
    assert!(index.is_rebuilding());
    assert!(matches!(index.begin_rebuild(), Err(SearchError::RebuildInProgress)));

    drop(guard);
    assert!(!index.is_rebuilding());
    assert!(index.begin_rebuild().is_ok());
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
