use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use super::client::Pop3Client;

/// テスト用の最小POP3サーバー。実サーバーの挙動を模倣し、
/// USER/PASS/STAT/LIST/UIDL/RETR/DELE/QUIT/CAPAに応答する。
fn spawn_fake_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
    let port = listener.local_addr().unwrap().port();

    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept");
        handle_connection(stream);
    });

    port
}

fn handle_connection(mut stream: TcpStream) {
    stream.write_all(b"+OK fake pop3 server ready\r\n").unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            break;
        }
        let line = line.trim_end();

        if line == "CAPA" {
            stream
                .write_all(b"+OK Capability list follows\r\nUIDL\r\nSTLS\r\n.\r\n")
                .unwrap();
        } else if line.starts_with("USER") || line.starts_with("PASS") {
            stream.write_all(b"+OK\r\n").unwrap();
        } else if line == "STAT" {
            stream.write_all(b"+OK 2 300\r\n").unwrap();
        } else if line == "LIST" {
            stream
                .write_all(b"+OK 2 messages\r\n1 100\r\n2 200\r\n.\r\n")
                .unwrap();
        } else if line == "UIDL" {
            stream
                .write_all(b"+OK\r\n1 uidl-one\r\n2 uidl-two\r\n.\r\n")
                .unwrap();
        } else if line == "RETR 1" {
            // ".dot-leading line" が本文にある場合のドットスタッフィングを検証する。
            stream
                .write_all(b"+OK 100 octets\r\nSubject: hi\r\n\r\n..dot-leading line\r\nbody\r\n.\r\n")
                .unwrap();
        } else if line == "RETR 2" {
            stream
                .write_all(b"+OK 50 octets\r\nSubject: second\r\n\r\nsecond body\r\n.\r\n")
                .unwrap();
        } else if line.starts_with("DELE") {
            stream.write_all(b"+OK message deleted\r\n").unwrap();
        } else if line == "QUIT" {
            stream.write_all(b"+OK bye\r\n").unwrap();
            break;
        } else {
            stream.write_all(b"-ERR unknown command\r\n").unwrap();
        }
    }
}

#[test]
fn full_session_against_fake_server() {
    let port = spawn_fake_server();
    let mut client = Pop3Client::connect_plain("127.0.0.1", port).expect("connect");

    let capa = client.capa().expect("capa");
    assert_eq!(capa, vec!["UIDL".to_string(), "STLS".to_string()]);

    client.user("alice").expect("user");
    client.pass("secret").expect("pass");

    let (count, size) = client.stat().expect("stat");
    assert_eq!((count, size), (2, 300));

    let list = client.list().expect("list");
    assert_eq!(list, vec![(1, 100), (2, 200)]);

    let uidl = client.uidl().expect("uidl");
    assert_eq!(
        uidl,
        vec![(1, "uidl-one".to_string()), (2, "uidl-two".to_string())]
    );

    let msg1 = client.retr(1).expect("retr 1");
    assert_eq!(
        msg1,
        b"Subject: hi\r\n\r\n.dot-leading line\r\nbody".to_vec()
    );

    client.dele(1).expect("dele");
    client.quit().expect("quit");
}

#[test]
fn retr_batch_reads_responses_in_order() {
    let port = spawn_fake_server();
    let mut client = Pop3Client::connect_plain("127.0.0.1", port).expect("connect");

    let results = client.retr_batch(&[1, 2]).expect("retr_batch");
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0].as_ref().unwrap(),
        b"Subject: hi\r\n\r\n.dot-leading line\r\nbody"
    );
    assert_eq!(
        results[1].as_ref().unwrap(),
        b"Subject: second\r\n\r\nsecond body"
    );

    client.quit().expect("quit");
}

#[test]
fn plaintext_connection_is_refused_without_opt_in() {
    let port = spawn_fake_server();
    let result = Pop3Client::connect("127.0.0.1", port, false, false);
    assert!(matches!(
        result,
        Err(super::Pop3Error::PlaintextNotAllowed(_))
    ));
}

#[test]
fn plaintext_connection_succeeds_with_explicit_opt_in() {
    let port = spawn_fake_server();
    let mut client =
        Pop3Client::connect("127.0.0.1", port, false, true).expect("plaintext opt-in connect");
    client.quit().expect("quit");
}
