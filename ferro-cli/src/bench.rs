//! `ferro bench <count>`: 開発用のスループット/レイテンシ計測コマンド。
//!
//! 実データは本番のDB/検索インデックスとは完全に別の`$TMPDIR/ferro-bench`配下に
//! 生成し、既存データには一切触れない。生メールの実体（Maildir）は本ベンチマークの
//! 対象外（SQLite挿入とTantivy索引のスループット、一覧・検索のレイテンシを見るのが
//! 目的で、ファイルシステムへの大量の小ファイル書き込みは別の関心事のため）。

use std::time::{Duration, Instant};

use ferro_core::db::accounts::NewAccount;
use ferro_core::db::messages::NewMessage;
use ferro_core::db::{self, messages};
use ferro_core::search::{IndexableMessage, SearchIndex};

/// DB挿入・検索索引投入ともにこの件数ごとにコミットする
/// （1件ずつコミットするとコミット回数がボトルネックになるため）。
const BATCH_SIZE: u64 = 5_000;

pub fn run(count: u64) -> anyhow::Result<()> {
    let bench_dir = std::env::temp_dir().join("ferro-bench");
    if bench_dir.exists() {
        std::fs::remove_dir_all(&bench_dir)?;
    }
    std::fs::create_dir_all(&bench_dir)?;
    println!("bench directory: {}", bench_dir.display());

    let conn = db::open(&bench_dir.join("bench.db"))?;
    let search_index = SearchIndex::open_or_create(&bench_dir.join("search_index"))?;

    let account_id = ferro_core::db::accounts::insert(
        &conn,
        &NewAccount {
            name: "bench",
            host: "bench.invalid",
            port: 995,
            username: "bench",
            use_tls: true,
        },
    )?;

    let base_time = 2_000_000_000i64; // 適当な固定エポック秒（2033年頃）。日付の意味自体はベンチに無関係。

    println!("generating and inserting {count} message(s) into SQLite...");
    let insert_elapsed = insert_messages(&conn, account_id, count, base_time)?;
    report_throughput("SQLite insert", count, insert_elapsed);

    println!("indexing {count} message(s) into Tantivy...");
    let index_elapsed = index_messages(&conn, &search_index)?;
    report_throughput("Tantivy index", count, index_elapsed);

    bench_list_recent(&conn, account_id, count, base_time)?;
    bench_search(&search_index)?;

    println!(
        "done. bench data left at {} for inspection; re-running `ferro bench` clears it first.",
        bench_dir.display()
    );
    Ok(())
}

fn insert_messages(
    conn: &db::Connection,
    account_id: i64,
    count: u64,
    base_time: i64,
) -> anyhow::Result<Duration> {
    let start = Instant::now();
    let mut inserted_since_commit = 0u64;
    let mut tx = conn.unchecked_transaction()?;

    for i in 0..count {
        messages::insert_new(
            &tx,
            &NewMessage {
                account_id,
                uidl: &format!("bench-{i}"),
                message_id_header: None,
                subject: Some(&format!("Benchmark message {i}")),
                from_name: Some("Bench Sender"),
                from_addr: Some(&format!("sender{i}@example.com")),
                to_addr: None,
                date_header: base_time - i as i64,
                size_bytes: 0,
                attachment_count: 0,
                preview: None,
            },
        )?;
        inserted_since_commit += 1;

        if inserted_since_commit >= BATCH_SIZE {
            tx.commit()?;
            tx = conn.unchecked_transaction()?;
            inserted_since_commit = 0;
        }
    }
    tx.commit()?;

    Ok(start.elapsed())
}

fn body_for(i: u64) -> String {
    // 1000件に1件だけ"needle"を含める。検索レイテンシ計測で絞り込みが
    // 効いた状態（全件ヒットではない）を再現するため。
    if i.is_multiple_of(1000) {
        format!("this is benchmark body {i} containing the needle term")
    } else {
        format!("this is generic benchmark body content for message {i}")
    }
}

/// `search_index.commit()`はWindows実機で断続的に(tantivyのマージ/GCワーカースレッドが
/// 異常終了して)失敗することがある。1バッチ分のindex_message+commitをまとめてリトライ
/// することでこの一過性の失敗を吸収する（`SearchIndex::commit`のドキュメント参照）。
/// 原因（おそらくアンチウイルスのリアルタイムスキャン等によるファイルI/O競合）が
/// 数百ms単位で解消することがあるため、指数バックオフを挟む。
const MAX_COMMIT_ATTEMPTS: u32 = 5;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(100);

fn index_messages(conn: &db::Connection, search_index: &SearchIndex) -> anyhow::Result<Duration> {
    let start = Instant::now();
    let mut after_id = 0i64;

    loop {
        let page = messages::list_all_for_reindex(conn, after_id, BATCH_SIZE as u32)?;
        if page.is_empty() {
            break;
        }
        after_id = page.last().expect("checked non-empty above").id;

        let mut last_err = None;
        let mut succeeded = false;
        for attempt in 0..MAX_COMMIT_ATTEMPTS {
            if attempt > 0 {
                println!("  (transient index commit failure, retrying, attempt {})", attempt + 1);
                std::thread::sleep(RETRY_BASE_DELAY * attempt);
            }
            match index_page(search_index, &page) {
                Ok(()) => {
                    succeeded = true;
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        if !succeeded {
            return Err(last_err.expect("loop runs MAX_COMMIT_ATTEMPTS >= 1 times"));
        }
    }

    Ok(start.elapsed())
}

fn index_page(search_index: &SearchIndex, page: &[ferro_core::db::messages::Message]) -> anyhow::Result<()> {
    for message in page {
        // 生成時に埋め込んだ連番をuidlから復元し、本文をその場で作り直す
        // （Maildirを経由しないため、ここで決定的に再現する）。
        let i: u64 = message.uidl.trim_start_matches("bench-").parse()?;
        let from = format!(
            "{} {}",
            message.from_name.as_deref().unwrap_or(""),
            message.from_addr.as_deref().unwrap_or("")
        );
        search_index.index_message(&IndexableMessage {
            id: message.id,
            subject: message.subject.as_deref().unwrap_or(""),
            from: &from,
            body: &body_for(i),
            date_header: message.date_header,
        })?;
    }
    search_index.commit()?;
    Ok(())
}

fn bench_list_recent(
    conn: &db::Connection,
    account_id: i64,
    count: u64,
    base_time: i64,
) -> anyhow::Result<()> {
    let first = time_it(|| messages::list_recent(conn, Some(account_id), None, 50));
    first.1?;
    report_latency("list_recent (first page)", first.0);

    if count > 4 {
        let middle_cursor = base_time - (count / 2) as i64;
        let middle = time_it(|| {
            messages::list_recent(conn, Some(account_id), Some(middle_cursor), 50)
        });
        middle.1?;
        report_latency("list_recent (middle page)", middle.0);
    }

    let last_cursor = base_time - count as i64 + 20;
    let last = time_it(|| messages::list_recent(conn, Some(account_id), Some(last_cursor), 50));
    last.1?;
    report_latency("list_recent (last page)", last.0);

    Ok(())
}

fn bench_search(search_index: &SearchIndex) -> anyhow::Result<()> {
    let (elapsed, result) = time_it(|| search_index.search("needle", 50));
    result?;
    report_latency("search (\"needle\", top 50)", elapsed);
    Ok(())
}

fn time_it<T>(f: impl FnOnce() -> T) -> (Duration, T) {
    let start = Instant::now();
    let result = f();
    (start.elapsed(), result)
}

fn report_throughput(label: &str, count: u64, elapsed: Duration) {
    let per_sec = if elapsed.as_secs_f64() > 0.0 {
        count as f64 / elapsed.as_secs_f64()
    } else {
        f64::INFINITY
    };
    println!(
        "  {label}: {count} rows in {:.2}s ({:.0} rows/s)",
        elapsed.as_secs_f64(),
        per_sec
    );
}

fn report_latency(label: &str, elapsed: Duration) {
    println!("  {label}: {:.2}ms", elapsed.as_secs_f64() * 1000.0);
}
