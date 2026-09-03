use std::io::{self, Read, Write};
use std::net::TcpStream;

use native_tls::TlsStream;

/// プレーンTCPと暗黙的TLS/STLS後のTLSを透過的に扱うためのラッパー。
pub enum Pop3Stream {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl Pop3Stream {
    /// STLS/暗黙的TLSの前提となる生のTCPストリームを取り出す。
    /// すでにTLS化済みの場合はNoneを返す（多重にSTLSしないためのガード）。
    pub fn into_plain_tcp(self) -> Option<TcpStream> {
        match self {
            Pop3Stream::Plain(s) => Some(s),
            Pop3Stream::Tls(_) => None,
        }
    }
}

impl Read for Pop3Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Pop3Stream::Plain(s) => s.read(buf),
            Pop3Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Pop3Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Pop3Stream::Plain(s) => s.write(buf),
            Pop3Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Pop3Stream::Plain(s) => s.flush(),
            Pop3Stream::Tls(s) => s.flush(),
        }
    }
}
