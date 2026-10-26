use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use tempfile::tempdir;

use super::*;
use crate::db::accounts::{self, NewAccount};
use crate::db::labels;
use crate::db::{messages, open_in_memory};
use crate::message_filter::MessageFilter;
use crate::search::SearchIndex;

fn make_search_index() -> SearchIndex {
    SearchIndex::create_in_ram().unwrap()
}

struct FakeMessage {
    uidl: &'static str,
    raw: &'static [u8],
}

fn spawn_server(messages: Vec<FakeMessage>) -> u16 {
    spawn_server_with_drop(messages, None)
}

/// `drop_after_retr`で指定した番号のRETRを受け取った時点で、応答せず接続を
/// 切断する（実サーバーがパイプライン中に切断するケースの再現）。これは
/// 最初の接続でのみ発動し、以降の接続（再接続後）は正常応答する。
fn spawn_server_with_drop(messages: Vec<FakeMessage>, drop_after_retr: Option<u32>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        // テストで必要になるのは最大2接続（1回目の同期 + 再接続後 or 2回目のsync呼び出し）。
        for (i, stream) in listener.incoming().take(2).enumerate() {
            let stream = stream.unwrap();
            let drop_at = if i == 0 { drop_after_retr } else { None };
            handle_session(stream, &messages, drop_at);
        }
    });

    port
}

/// 最初の`early_drops`回の接続は挨拶すら送らずに即座に切断する
/// （レンタルサーバー等で起きた、ハンドシェイク段階での"connection closed by
/// server"の再現）。`early_drops`回を過ぎたら通常どおり応答する。
fn spawn_server_with_early_drops(messages: Vec<FakeMessage>, early_drops: u32) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        for (i, stream) in listener.incoming().take(early_drops as usize + 1).enumerate() {
            let stream = stream.unwrap();
            if (i as u32) < early_drops {
                drop(stream);
                continue;
            }
            handle_session(stream, &messages, None);
        }
    });

    port
}

fn handle_session(mut stream: TcpStream, messages: &[FakeMessage], drop_after_retr: Option<u32>) {
    stream.write_all(b"+OK fake pop3 ready\r\n").unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            return;
        }
        let line = line.trim_end();

        if line.starts_with("USER") || line.starts_with("PASS") {
            stream.write_all(b"+OK\r\n").unwrap();
        } else if line == "UIDL" {
            let mut resp = String::from("+OK\r\n");
            for (i, m) in messages.iter().enumerate() {
                resp.push_str(&format!("{} {}\r\n", i + 1, m.uidl));
            }
            resp.push_str(".\r\n");
            stream.write_all(resp.as_bytes()).unwrap();
        } else if let Some(rest) = line.strip_prefix("RETR ") {
            let num: u32 = rest.trim().parse().unwrap();
            if Some(num) == drop_after_retr {
                return; // 応答せず切断する
            }
            let body = messages[(num - 1) as usize].raw;
            stream
                .write_all(format!("+OK {} octets\r\n", body.len()).as_bytes())
                .unwrap();
            stream.write_all(body).unwrap();
            stream.write_all(b"\r\n.\r\n").unwrap();
        } else if line == "QUIT" {
            stream.write_all(b"+OK bye\r\n").unwrap();
            return;
        } else {
            stream.write_all(b"-ERR unknown command\r\n").unwrap();
        }
    }
}

fn make_account(conn: &Connection, port: u16) -> Account {
    let id = accounts::insert(
        conn,
        &NewAccount {
            name: "test",
            host: "127.0.0.1",
            port,
            username: "user",
            use_tls: false,
        },
    )
    .unwrap();
    accounts::get(conn, id).unwrap().unwrap()
}

#[test]
fn sync_respects_limit_and_resumes_next_call() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let fake_messages = vec![
        FakeMessage {
            uidl: "u1",
            raw: b"Subject: one\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\n\r\nbody1",
        },
        FakeMessage {
            uidl: "u2",
            raw: b"Subject: two\r\nDate: Tue, 2 Jan 2024 00:00:00 +0000\r\n\r\nbody2",
        },
        FakeMessage {
            uidl: "u3",
            raw: b"Subject: three\r\nDate: Wed, 3 Jan 2024 00:00:00 +0000\r\n\r\nbody3",
        },
    ];
    let port = spawn_server(fake_messages);
    let account = make_account(&conn, port);

    let summary = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        Some(2),
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        summary,
        SyncSummary {
            fetched: 2,
            remaining: 1,
            ended_early: false,
            server_deleted: 0,
        }
    );
    assert!(maildir::exists(maildir_base.path(), account.id, "u1"));
    assert!(maildir::exists(maildir_base.path(), account.id, "u2"));
    assert!(!maildir::exists(maildir_base.path(), account.id, "u3"));

    let stored = messages::list_recent(&conn, Some(account.id), None, 10).unwrap();
    assert_eq!(stored.len(), 2);
    let one = stored.iter().find(|m| m.uidl == "u1").unwrap();
    assert_eq!(one.subject.as_deref(), Some("one"));

    // syncで取り込んだメッセージが検索インデックスにも入っていることを確認する。
    assert_eq!(search_index.search("one", 10).unwrap(), vec![one.id]);

    // 残りは次回のsync呼び出しで拾われる。
    let summary2 = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        summary2,
        SyncSummary {
            fetched: 1,
            remaining: 0,
            ended_early: false,
            server_deleted: 0,
        }
    );
    assert!(maildir::exists(maildir_base.path(), account.id, "u3"));

    let three_id = messages::list_recent(&conn, Some(account.id), None, 10)
        .unwrap()
        .into_iter()
        .find(|m| m.uidl == "u3")
        .unwrap()
        .id;
    assert_eq!(search_index.search("three", 10).unwrap(), vec![three_id]);
}

#[test]
fn sync_is_idempotent_when_nothing_new() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let port = spawn_server(vec![FakeMessage {
        uidl: "u1",
        raw: b"Subject: one\r\n\r\nbody1",
    }]);
    let account = make_account(&conn, port);

    sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();
    let summary = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        summary,
        SyncSummary {
            fetched: 0,
            remaining: 0,
            ended_early: false,
            server_deleted: 0,
        }
    );
}

/// CLAUDE.mdに記録されている実サーバーでの既知の挙動: パイプライン化した
/// RETR中にサーバーが接続を切断することがある。切断したら再接続して
/// 続きから再開できることを確認する。
#[test]
fn sync_reconnects_after_mid_pipeline_disconnect() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let fake_messages = vec![
        FakeMessage {
            uidl: "u1",
            raw: b"Subject: one\r\n\r\nbody1",
        },
        FakeMessage {
            uidl: "u2",
            raw: b"Subject: two\r\n\r\nbody2",
        },
    ];
    // 最初の接続でRETR 2を受け取った時点で応答せず切断する。
    let port = spawn_server_with_drop(fake_messages, Some(2));
    let account = make_account(&conn, port);

    let summary = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();

    assert_eq!(summary.fetched, 2);
    assert_eq!(summary.remaining, 0);
    assert!(!summary.ended_early);
    assert!(maildir::exists(maildir_base.path(), account.id, "u1"));
    assert!(maildir::exists(maildir_base.path(), account.id, "u2"));
}

/// 他人のレンタルサーバー環境で実際に"connection closed by server"として
/// 踏んだケース: RETRパイプライン中ではなく、CONNECT/USER/PASS/UIDLの
/// ハンドシェイク段階で接続が切断される場合も、同じ再接続予算内であれば
/// リトライして復旧できることを確認する。
#[test]
fn sync_reconnects_after_disconnect_during_initial_handshake() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let fake_messages = vec![FakeMessage {
        uidl: "u1",
        raw: b"Subject: one\r\n\r\nbody1",
    }];
    // 最初の2回の接続は挨拶を送らず切断される。3回目でようやく成功する
    // （MAX_RECONNECTS=5の予算内）。
    let port = spawn_server_with_early_drops(fake_messages, 2);
    let account = make_account(&conn, port);

    let summary = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();

    assert_eq!(
        summary,
        SyncSummary {
            fetched: 1,
            remaining: 0,
            ended_early: false,
            server_deleted: 0,
        }
    );
    assert!(maildir::exists(maildir_base.path(), account.id, "u1"));
}

/// ハンドシェイク段階での切断が再接続予算(`MAX_RECONNECTS`)を使い切るほど
/// 続く場合は、まだ何も取得できていないためエラーを返す
/// （`ended_early`付きの部分成功では表現できないため）。
#[test]
fn sync_gives_up_after_exhausting_reconnects_during_initial_handshake() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    // MAX_RECONNECTS(5)を超える6回連続で切断させる。
    let port = spawn_server_with_early_drops(vec![], 6);
    let account = make_account(&conn, port);

    let result = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    );

    assert!(matches!(
        result,
        Err(SyncError::Pop3(Pop3Error::ConnectionClosed))
    ));

    let status = crate::db::sync_status::get(&conn, account.id).unwrap().unwrap();
    assert_eq!(status.consecutive_failures, 1);
    assert_eq!(status.last_success_at, None);
    assert!(status.last_error.is_some());
}

/// UIDL非対応（送ると接続を切断する）サーバーを模倣する。実際にレンタルサーバー
/// 宛のアカウントで踏んだ挙動の再現: CONNECT/USER/PASSには正常に応答するが、
/// UIDLの直後だけ応答せず切断する。1回目の接続でのみ発動し、以降の接続
/// （フォールバック後）ではLIST/RETRに正常応答する。`uidl_call_count`で
/// UIDLが呼ばれた回数を記録する（2回目以降の同期呼び出しでフォールバック判定が
/// 効いて二度とUIDLを試さないことを確認するため）。
fn spawn_uidl_unsupported_server(
    messages: Vec<FakeMessage>,
    uidl_call_count: Arc<AtomicUsize>,
) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        // 1回目: UIDLで切断（非対応の再現）。2回目: その直後のフォールバック
        // 再接続でLIST/RETR。3回目: テストの2度目のsync_account_with_limit呼び出し
        // （uidl_supported=false がDBに残っているはずなので、最初からLIST/RETR）。
        for (i, stream) in listener.incoming().take(3).enumerate() {
            let mut stream = stream.unwrap();
            stream.write_all(b"+OK fake pop3 ready\r\n").unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());

            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let line = line.trim_end();

                if line.starts_with("USER") || line.starts_with("PASS") {
                    stream.write_all(b"+OK\r\n").unwrap();
                } else if line == "UIDL" {
                    uidl_call_count.fetch_add(1, Ordering::SeqCst);
                    if i == 0 {
                        break; // 応答せず切断する（UIDL非対応の再現）
                    }
                    // フォールバック判定後の接続でUIDLが呼ばれるのはバグなので、
                    // テストが検出できるよう明示的に-ERRを返す。
                    stream.write_all(b"-ERR should not be called again\r\n").unwrap();
                } else if line == "LIST" {
                    let mut resp = String::from("+OK\r\n");
                    for (idx, m) in messages.iter().enumerate() {
                        resp.push_str(&format!("{} {}\r\n", idx + 1, m.raw.len()));
                    }
                    resp.push_str(".\r\n");
                    stream.write_all(resp.as_bytes()).unwrap();
                } else if let Some(rest) = line.strip_prefix("RETR ") {
                    let num: u32 = rest.trim().parse().unwrap();
                    let body = messages[(num - 1) as usize].raw;
                    stream
                        .write_all(format!("+OK {} octets\r\n", body.len()).as_bytes())
                        .unwrap();
                    stream.write_all(body).unwrap();
                    stream.write_all(b"\r\n.\r\n").unwrap();
                } else if line == "QUIT" {
                    stream.write_all(b"+OK bye\r\n").unwrap();
                    break;
                } else {
                    stream.write_all(b"-ERR unknown command\r\n").unwrap();
                }
            }
        }
    });

    port
}

#[test]
fn sync_falls_back_to_content_hash_when_uidl_is_unsupported() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let fake_messages = vec![
        FakeMessage {
            uidl: "unused-1",
            raw: b"Subject: one\r\n\r\nbody1",
        },
        FakeMessage {
            uidl: "unused-2",
            raw: b"Subject: two\r\n\r\nbody2",
        },
    ];
    let uidl_call_count = Arc::new(AtomicUsize::new(0));
    let port = spawn_uidl_unsupported_server(fake_messages, uidl_call_count.clone());
    let account = make_account(&conn, port);
    assert_eq!(account.uidl_supported, None);

    let summary = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();

    assert_eq!(
        summary,
        SyncSummary {
            fetched: 2,
            remaining: 0,
            ended_early: false,
            server_deleted: 0,
        }
    );
    assert_eq!(
        uidl_call_count.load(Ordering::SeqCst),
        1,
        "UIDL should be tried exactly once before falling back"
    );

    let updated_account = accounts::get(&conn, account.id).unwrap().unwrap();
    assert_eq!(updated_account.uidl_supported, Some(false));

    let stored = messages::list_recent(&conn, Some(account.id), None, 10).unwrap();
    assert_eq!(stored.len(), 2);
    // フォールバック経路のuidlは内容ハッシュなので、サーバー側の(未使用の)uidlとは
    // 別物になる。
    assert!(stored.iter().all(|m| m.uidl.starts_with("ferro-hash-v2-")));

    // 2回目のsync呼び出し（DBに記録された uidl_supported=Some(false) を読み込んだ
    // 新しいAccount）では、UIDLは一切呼ばれず、内容ハッシュが一致するので
    // 新着扱いにもならないはず。
    let summary2 = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &updated_account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        summary2,
        SyncSummary {
            fetched: 0,
            remaining: 0,
            ended_early: false,
            server_deleted: 0,
        }
    );
    assert_eq!(
        uidl_call_count.load(Ordering::SeqCst),
        1,
        "second sync must not try UIDL again"
    );
}

/// 実際にユーザーがファンの唸り/大量のI/Oバイト数として踏んだ回帰テスト:
/// UIDL非対応フォールバック経路は、RETR前には重複かどうか分からないため
/// 全件RETRし直すが、既に保存済みの内容ならMaildirへの書き込み自体を
/// スキップしなければならない（DB側の重複排除だけでは、同期のたびに毎回
/// メールボックス全体をディスクへ書き直すことになってしまう）。
#[test]
fn sync_fallback_does_not_rewrite_already_stored_messages_to_disk() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let fake_messages = vec![FakeMessage {
        uidl: "unused-1",
        raw: b"Subject: one\r\n\r\nbody1",
    }];
    let uidl_call_count = Arc::new(AtomicUsize::new(0));
    let port = spawn_uidl_unsupported_server(fake_messages, uidl_call_count);
    let account = make_account(&conn, port);

    sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();

    let uidl = content_uidl(b"Subject: one\r\n\r\nbody1");
    let path = maildir::message_path(maildir_base.path(), account.id, &uidl);
    let mtime_after_first_sync = fs::metadata(&path).unwrap().modified().unwrap();

    let updated_account = accounts::get(&conn, account.id).unwrap().unwrap();
    sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &updated_account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();

    let mtime_after_second_sync = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(
        mtime_after_first_sync, mtime_after_second_sync,
        "the already-known message must not be rewritten to disk on the second sync"
    );
}

/// 「今すぐ整理する」実行後のsyncでSync数が異常に多い、として実際に踏んだ回帰
/// テスト: 保持期間切れで完全削除した(`message_actions::purge_expired_batch`と
/// 同じ手順で)uidlは、DELEを送っていないためサーバーにはまだ残っているが、
/// `purged_uidls`に記録されていればUIDL差分方式が新着として再取得しないこと。
#[test]
fn sync_does_not_refetch_a_uidl_that_was_already_purged_for_retention() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let fake_messages = vec![
        FakeMessage {
            uidl: "u1",
            raw: b"Subject: one\r\n\r\nbody1",
        },
        FakeMessage {
            uidl: "u2",
            raw: b"Subject: two\r\n\r\nbody2",
        },
    ];
    let port = spawn_server(fake_messages);
    let account = make_account(&conn, port);

    // u1はサーバーにまだ残っているが、保持期間切れで完全削除済み(墓標のみ残る)
    // という状況を再現する。
    crate::db::purged_uidls::mark_purged(&conn, account.id, "u1").unwrap();

    let summary = sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &LabelRulesFile::default(),
        None,
        |_, _| {},
    )
    .unwrap();

    assert_eq!(
        summary,
        SyncSummary {
            fetched: 1,
            remaining: 0,
            ended_early: false,
            server_deleted: 0,
        }
    );
    assert!(
        !maildir::exists(maildir_base.path(), account.id, "u1"),
        "the purged message must not be resurrected on disk"
    );
    assert!(maildir::exists(maildir_base.path(), account.id, "u2"));
    assert!(
        messages::list_recent(&conn, Some(account.id), None, 10)
            .unwrap()
            .iter()
            .all(|m| m.uidl != "u1"),
        "the purged message must not be resurrected in the db"
    );
}

/// 受信時の自動ラベル付け(`label_rules`): syncで新着メッセージを取り込んだその場で、
/// ルールに合致すればラベルが自動的に付くことを確認する。
#[test]
fn sync_applies_label_rules_to_newly_fetched_messages() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();
    let label_id = labels::insert(&conn, "invoices", "#C08A2E").unwrap();

    let fake_messages = vec![
        FakeMessage {
            uidl: "u1",
            raw: b"Subject: Invoice #9012\r\n\r\nbody1",
        },
        FakeMessage {
            uidl: "u2",
            raw: b"Subject: unrelated\r\n\r\nbody2",
        },
    ];
    let port = spawn_server(fake_messages);
    let account = make_account(&conn, port);

    let rules = LabelRulesFile {
        rules: vec![crate::label_rules::LabelRule {
            label_name: "invoices".to_string(),
            filter: MessageFilter {
                subject_contains: Some("invoice".to_string()),
                ..Default::default()
            },
        }],
    };

    sync_account_with_limit(
        &conn,
        maildir_base.path(),
        &account,
        "pw",
        true,
        None,
        &search_index,
        &rules,
        None,
        |_, _| {},
    )
    .unwrap();

    let stored = messages::list_recent(&conn, Some(account.id), None, 10).unwrap();
    let matching = stored.iter().find(|m| m.uidl == "u1").unwrap();
    let unrelated = stored.iter().find(|m| m.uidl == "u2").unwrap();

    let matching_labels = labels::for_message(&conn, matching.id).unwrap();
    assert_eq!(matching_labels.len(), 1);
    assert_eq!(matching_labels[0].id, label_id);
    assert!(labels::for_message(&conn, unrelated.id).unwrap().is_empty());
}

/// DELEを受け取ってもQUITが来るまでは確定させず、QUITで初めて`deleted`に
/// 反映するフェイクサーバー（実POP3のDELEの意味論の再現）。接続は1回だけ受ける。
fn spawn_server_recording_deletes(
    messages: Vec<FakeMessage>,
) -> (u16, std::sync::Arc<std::sync::Mutex<Vec<u32>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let deleted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let deleted_out = deleted.clone();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"+OK fake pop3 ready
").unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut marked: Vec<u32> = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                return;
            }
            let line = line.trim_end();
            if line.starts_with("USER") || line.starts_with("PASS") {
                stream.write_all(b"+OK
").unwrap();
            } else if line == "UIDL" {
                let mut resp = String::from("+OK
");
                for (i, m) in messages.iter().enumerate() {
                    resp.push_str(&format!("{} {}
", i + 1, m.uidl));
                }
                resp.push_str(".
");
                stream.write_all(resp.as_bytes()).unwrap();
            } else if let Some(rest) = line.strip_prefix("DELE ") {
                marked.push(rest.trim().parse().unwrap());
                stream.write_all(b"+OK marked
").unwrap();
            } else if line == "QUIT" {
                deleted.lock().unwrap().extend(marked.drain(..));
                stream.write_all(b"+OK bye
").unwrap();
                return;
            } else {
                stream.write_all(b"-ERR unknown command
").unwrap();
            }
        }
    });

    (port, deleted_out)
}

/// 古い保存済みメールだけがサーバーから消え、新しいメール・ローカルに実体が無い
/// メールは残る。取得対象(pending)が空でも削除とQUITが走ることも兼ねて確認する。
#[test]
fn sync_deletes_old_stored_messages_from_server_after_quit() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let fake_messages = vec![
        FakeMessage { uidl: "old", raw: b"Subject: old
Date: Mon, 1 Jan 2024 00:00:00 +0000

b" },
        FakeMessage { uidl: "new", raw: b"Subject: new
Date: Mon, 1 Jan 2099 00:00:00 +0000

b" },
        FakeMessage { uidl: "nofile", raw: b"Subject: nofile
Date: Mon, 1 Jan 2024 00:00:00 +0000

b" },
    ];
    // 1回目は通常の取得（server_delete_before=None）で3通すべてをローカルに保存する。
    let port = spawn_server(fake_messages.iter().map(|m| FakeMessage { uidl: m.uidl, raw: m.raw }).collect());
    let account = make_account(&conn, port);
    sync_account_with_limit(
        &conn, maildir_base.path(), &account, "pw", true, None,
        &search_index, &LabelRulesFile::default(), None, |_, _| {},
    )
    .unwrap();
    maildir::remove(maildir_base.path(), account.id, "nofile").unwrap();

    // 2回目: pendingは空。2024年より後をカットオフにする。
    let (port2, deleted) = spawn_server_recording_deletes(fake_messages);
    let mut account2 = account.clone();
    account2.port = port2;
    let cutoff = 1_800_000_000; // 2027年頃
    let summary = sync_account_with_limit(
        &conn, maildir_base.path(), &account2, "pw", true, None,
        &search_index, &LabelRulesFile::default(), Some(cutoff), |_, _| {},
    )
    .unwrap();

    assert_eq!(summary.server_deleted, 1);
    assert_eq!(*deleted.lock().unwrap(), vec![1u32]); // "old"のみ
}

/// 実際に踏んだサーバーの再現: UIDL非対応で、(1)1接続あたりRETRを`retr_cap`通
/// 返したら応答せず切断し、(2)RETRのたびに経路上のゲートウェイが
/// `X-WatchGuard-Spam-ID`を付け替える（同じメールでも毎回バイト列が変わる）。
fn spawn_capped_volatile_server(count: usize, retr_cap: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let retr_counter = Arc::new(AtomicUsize::new(0));

    thread::spawn(move || {
        for stream in listener.incoming().take(200) {
            let mut stream = stream.unwrap();
            stream.write_all(b"+OK fake pop3 ready\r\n").unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut retrs_this_session = 0usize;

            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end();

                if line.starts_with("USER") || line.starts_with("PASS") {
                    stream.write_all(b"+OK\r\n").unwrap();
                } else if line == "UIDL" {
                    break; // 応答せず切断（UIDL非対応）
                } else if line == "LIST" {
                    let mut resp = String::from("+OK\r\n");
                    for n in 1..=count {
                        resp.push_str(&format!("{n} 200\r\n"));
                    }
                    resp.push_str(".\r\n");
                    stream.write_all(resp.as_bytes()).unwrap();
                } else if let Some(rest) = line.strip_prefix("RETR ") {
                    if retrs_this_session >= retr_cap {
                        break;
                    }
                    retrs_this_session += 1;
                    let n: usize = rest.trim().parse().unwrap();
                    let volatile = retr_counter.fetch_add(1, Ordering::SeqCst);
                    let body = format!(
                        "X-WatchGuard-Spam-ID: v=2.4 ts={volatile}\r\n\
                         Message-ID: <msg-{n}@example.com>\r\n\
                         Subject: message {n}\r\n\r\nbody {n}"
                    );
                    stream
                        .write_all(format!("+OK {} octets\r\n", body.len()).as_bytes())
                        .unwrap();
                    stream.write_all(body.as_bytes()).unwrap();
                    stream.write_all(b"\r\n.\r\n").unwrap();
                } else if line == "QUIT" {
                    stream.write_all(b"+OK bye\r\n").unwrap();
                    break;
                } else {
                    stream.write_all(b"-ERR unknown command\r\n").unwrap();
                }
            }
        }
    });

    port
}

#[test]
fn content_uidl_ignores_headers_the_gateway_rewrites_on_every_retr() {
    let a = b"X-WatchGuard-Spam-ID: aaa\r\nMessage-ID: <id@x>\r\nSubject: s\r\n\r\nbody";
    let b = b"X-WatchGuard-Spam-ID: bbb\r\nMessage-ID: <id@x>\r\nSubject: s\r\n\r\nbody";
    assert_eq!(content_uidl(a), content_uidl(b));

    // Message-IDが無くても、ゲートウェイが付け替えるヘッダーは無視して本文等で判定する。
    let c = b"X-WatchGuard-Spam-ID: aaa\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\nSubject: s\r\n\r\nbody";
    let d = b"X-WatchGuard-Spam-ID: bbb\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\nSubject: s\r\n\r\nbody";
    let e = b"X-WatchGuard-Spam-ID: aaa\r\nDate: Mon, 1 Jan 2024 00:00:00 +0000\r\nSubject: s\r\n\r\nother";
    assert_eq!(content_uidl(c), content_uidl(d));
    assert_ne!(content_uidl(c), content_uidl(e));
}

/// 回帰テスト: 1接続あたり少数通で切断され、かつRETRのたびにヘッダーが変わる
/// サーバーでも、(1)同じメールが重複保存されず、(2)繰り返しsyncすれば
/// 取得済みの先頭部分に足止めされず最後まで取り切れる。
#[test]
fn sync_fallback_resumes_and_does_not_duplicate_with_volatile_headers() {
    let conn = open_in_memory().unwrap();
    let maildir_base = tempdir().unwrap();
    let search_index = make_search_index();

    let total = 100;
    let port = spawn_capped_volatile_server(total, 25);
    let mut account = make_account(&conn, port);

    let mut done = false;
    for _ in 0..15 {
        let summary = sync_account_with_limit(
            &conn,
            maildir_base.path(),
            &account,
            "pw",
            true,
            None,
            &search_index,
            &LabelRulesFile::default(),
            None,
            |_, _| {},
        )
        .unwrap();
        account = accounts::get(&conn, account.id).unwrap().unwrap();
        if summary.remaining == 0 && !summary.ended_early {
            done = true;
            break;
        }
    }
    assert!(done, "sync should eventually catch up");

    let stored = messages::list_recent(&conn, Some(account.id), None, 1000).unwrap();
    assert_eq!(stored.len(), total, "each message must be stored exactly once");
}
