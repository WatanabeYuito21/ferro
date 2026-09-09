use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use native_tls::TlsConnector;

use super::error::{Pop3Error, Result};
use super::stream::Pop3Stream;

/// ソケットの読み書きタイムアウト。以前は無制限（`TcpStream`にタイムアウトを
/// 設定していなかった）だったため、サーバーがTCP接続は受け付けたまま応答を
/// 返さなくなる（レート制限・輻輳・単純なハング等）と`read`が永久にブロックし、
/// そのまま`write_conn`を握り続けてバックグラウンド同期・検索インデックスの
/// キャッチアップ・GUIの他の操作まで巻き込んで止まったまま二度と直らない、
/// という実害を実際に踏んだ（"syncing…"のまま件数が進まず、新着も一切
/// 増えなくなった）。タイムアウトで`io::Error`を返せば`Pop3Error::Io`経由で
/// `sync::is_disconnect`が再接続対象と判定し、既存の再接続ロジックに乗る。
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

fn apply_socket_timeouts(tcp: &TcpStream) -> Result<()> {
    tcp.set_read_timeout(Some(READ_TIMEOUT))?;
    tcp.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Ok(())
}

/// POP3サーバーとの接続。CLAUDE.mdの方針どおり、
/// `use_tls`とポート番号に基づいて呼び出し側が接続方式を決める。
pub struct Pop3Client {
    reader: BufReader<Pop3Stream>,
}

impl Pop3Client {
    /// アカウント設定に基づいて接続する。
    ///
    /// - `use_tls=true` かつ `port==995`: 暗黙的TLS(POP3S)
    /// - `use_tls=true` かつそれ以外のポート: 平文で接続後STLSでアップグレード
    /// - `use_tls=false`: `allow_plaintext=true`のときのみ完全平文で接続を許可する
    pub fn connect(
        host: &str,
        port: u16,
        use_tls: bool,
        allow_plaintext: bool,
    ) -> Result<Pop3Client> {
        if use_tls {
            if port == 995 {
                Self::connect_implicit_tls(host, port)
            } else {
                Self::connect_plain(host, port)?.stls(host)
            }
        } else {
            if !allow_plaintext {
                return Err(Pop3Error::PlaintextNotAllowed(
                    "plaintext connection requires explicit opt-in (allow_plaintext)",
                ));
            }
            Self::connect_plain(host, port)
        }
    }

    /// 完全平文でTCP接続する。挨拶(greeting)行を読み切ってから返す。
    pub fn connect_plain(host: &str, port: u16) -> Result<Pop3Client> {
        let tcp = TcpStream::connect((host, port))?;
        apply_socket_timeouts(&tcp)?;
        let mut client = Pop3Client {
            reader: BufReader::new(Pop3Stream::Plain(tcp)),
        };
        client.read_status_line()?;
        Ok(client)
    }

    /// 暗黙的TLS(POP3S、通常ポート995)で接続する。
    pub fn connect_implicit_tls(host: &str, port: u16) -> Result<Pop3Client> {
        let tcp = TcpStream::connect((host, port))?;
        apply_socket_timeouts(&tcp)?;
        let connector = TlsConnector::new()?;
        let tls = connector.connect(host, tcp)?;
        let mut client = Pop3Client {
            reader: BufReader::new(Pop3Stream::Tls(Box::new(tls))),
        };
        client.read_status_line()?;
        Ok(client)
    }

    /// 平文接続中のセッションをSTLS(STARTTLS)でTLSにアップグレードする。
    ///
    /// STLS成功直後はサーバーから追加データが送られてくることはないため、
    /// BufReaderの内部バッファには未読データが残っていない前提で生TCPストリームを取り出す。
    fn stls(mut self, host: &str) -> Result<Pop3Client> {
        self.send_command("STLS")?;
        self.read_status_line()?;

        let tcp = self
            .reader
            .into_inner()
            .into_plain_tcp()
            .ok_or_else(|| Pop3Error::Protocol("STLS on an already-TLS connection".into()))?;

        let connector = TlsConnector::new()?;
        let tls = connector.connect(host, tcp)?;
        Ok(Pop3Client {
            reader: BufReader::new(Pop3Stream::Tls(Box::new(tls))),
        })
    }

    fn send_command(&mut self, cmd: &str) -> Result<()> {
        let stream = self.reader.get_mut();
        stream.write_all(cmd.as_bytes())?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
        Ok(())
    }

    fn read_line_raw(&mut self) -> Result<String> {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line)?;
        if n == 0 {
            return Err(Pop3Error::ConnectionClosed);
        }
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(line)
    }

    /// +OK/-ERRの単一行応答を読み、+OKなら本文（"+OK "の後ろ）を返す。
    fn read_status_line(&mut self) -> Result<String> {
        let line = self.read_line_raw()?;
        if let Some(rest) = line.strip_prefix("+OK") {
            Ok(rest.trim_start().to_string())
        } else if let Some(rest) = line.strip_prefix("-ERR") {
            Err(Pop3Error::ServerError(rest.trim_start().to_string()))
        } else {
            Err(Pop3Error::Protocol(format!("unexpected response: {line}")))
        }
    }

    /// マルチライン応答（LIST/UIDL/RETR/CAPA等）を、終端の単独"."まで読み、
    /// ドットスタッフィングを解除した行のリストを返す。
    fn read_multiline(&mut self) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        loop {
            let line = self.read_line_raw()?;
            if line == "." {
                break;
            }
            if let Some(unstuffed) = line.strip_prefix("..") {
                lines.push(format!(".{unstuffed}"));
            } else {
                lines.push(line);
            }
        }
        Ok(lines)
    }

    pub fn capa(&mut self) -> Result<Vec<String>> {
        self.send_command("CAPA")?;
        self.read_status_line()?;
        self.read_multiline()
    }

    pub fn user(&mut self, user: &str) -> Result<()> {
        self.send_command(&format!("USER {user}"))?;
        self.read_status_line()?;
        Ok(())
    }

    pub fn pass(&mut self, password: &str) -> Result<()> {
        self.send_command(&format!("PASS {password}"))?;
        self.read_status_line()?;
        Ok(())
    }

    /// (メッセージ件数, 全メッセージ合計バイト数) を返す。
    pub fn stat(&mut self) -> Result<(u32, u64)> {
        self.send_command("STAT")?;
        let reply = self.read_status_line()?;
        parse_two_numbers(&reply)
    }

    /// 全メッセージの (番号, バイト数) 一覧。
    pub fn list(&mut self) -> Result<Vec<(u32, u64)>> {
        self.send_command("LIST")?;
        self.read_status_line()?;
        self.read_multiline()?
            .iter()
            .map(|line| parse_two_numbers(line))
            .collect()
    }

    /// 全メッセージの (番号, UIDL文字列) 一覧。同期の差分計算に使う。
    pub fn uidl(&mut self) -> Result<Vec<(u32, String)>> {
        self.send_command("UIDL")?;
        self.read_status_line()?;
        self.read_multiline()?
            .iter()
            .map(|line| {
                let mut parts = line.splitn(2, ' ');
                let num = parts
                    .next()
                    .ok_or_else(|| Pop3Error::Protocol(format!("malformed UIDL line: {line}")))?
                    .parse::<u32>()
                    .map_err(|_| Pop3Error::Protocol(format!("malformed UIDL line: {line}")))?;
                let uidl = parts
                    .next()
                    .ok_or_else(|| Pop3Error::Protocol(format!("malformed UIDL line: {line}")))?
                    .to_string();
                Ok((num, uidl))
            })
            .collect()
    }

    /// 1件のメッセージを取得する（生の.eml相当のバイト列、CRLF区切り）。
    pub fn retr(&mut self, msg_num: u32) -> Result<Vec<u8>> {
        self.send_command(&format!("RETR {msg_num}"))?;
        self.read_status_line()?;
        Ok(self.read_multiline()?.join("\r\n").into_bytes())
    }

    /// 複数件のRETRをパイプライン化して取得する。
    ///
    /// POP3に正式なパイプライン拡張はないが、TCP上の厳密に順序付けられた
    /// 1コマンド=1応答ストリームであることを利用し、まとめて送ってから順に読む。
    /// 実サーバーによっては大きなバッチで切断されることがあるため、
    /// 呼び出し側でバッチサイズと再接続を制御する想定（`sync`側の責務）。
    pub fn retr_batch(&mut self, msg_nums: &[u32]) -> Result<Vec<Result<Vec<u8>>>> {
        for &num in msg_nums {
            self.send_command(&format!("RETR {num}"))?;
        }
        let mut results = Vec::with_capacity(msg_nums.len());
        for _ in msg_nums {
            let result = self
                .read_status_line()
                .and_then(|_| self.read_multiline())
                .map(|lines| lines.join("\r\n").into_bytes());
            results.push(result);
        }
        Ok(results)
    }

    pub fn dele(&mut self, msg_num: u32) -> Result<()> {
        self.send_command(&format!("DELE {msg_num}"))?;
        self.read_status_line()?;
        Ok(())
    }

    pub fn quit(&mut self) -> Result<()> {
        self.send_command("QUIT")?;
        self.read_status_line()?;
        Ok(())
    }
}

fn parse_two_numbers(line: &str) -> Result<(u32, u64)> {
    let mut parts = line.split_whitespace();
    let a = parts
        .next()
        .ok_or_else(|| Pop3Error::Protocol(format!("expected two numbers, got: {line}")))?
        .parse::<u32>()
        .map_err(|_| Pop3Error::Protocol(format!("expected a number, got: {line}")))?;
    let b = parts
        .next()
        .ok_or_else(|| Pop3Error::Protocol(format!("expected two numbers, got: {line}")))?
        .parse::<u64>()
        .map_err(|_| Pop3Error::Protocol(format!("expected a number, got: {line}")))?;
    Ok((a, b))
}
