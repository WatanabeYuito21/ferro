use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use tempfile::tempdir;

use super::*;
use crate::db::accounts::{self, NewAccount};
use crate::db::{messages, open_in_memory};
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
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        summary,
        SyncSummary {
            fetched: 2,
            remaining: 1,
            ended_early: false
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
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        summary2,
        SyncSummary {
            fetched: 1,
            remaining: 0,
            ended_early: false
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
        |_, _| {},
    )
    .unwrap();
    assert_eq!(
        summary,
        SyncSummary {
            fetched: 0,
            remaining: 0,
            ended_early: false
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
        |_, _| {},
    )
    .unwrap();

    assert_eq!(
        summary,
        SyncSummary {
            fetched: 1,
            remaining: 0,
            ended_early: false
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
        |_, _| {},
    );

    assert!(matches!(
        result,
        Err(SyncError::Pop3(Pop3Error::ConnectionClosed))
    ));
}
