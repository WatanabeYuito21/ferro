use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Pop3Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("TLS handshake failed: {0}")]
    Tls(#[from] native_tls::Error),

    #[error("TLS handshake failed: {0}")]
    TlsHandshake(#[from] native_tls::HandshakeError<std::net::TcpStream>),

    #[error("server rejected plaintext connection setup: {0}")]
    PlaintextNotAllowed(&'static str),

    #[error("server returned an error response: {0}")]
    ServerError(String),

    #[error("malformed response from server: {0}")]
    Protocol(String),

    /// サーバーが応答を書かずにコネクションを閉じた。実サーバーではRETRを
    /// パイプライン化した際にこれが起きることがあるため、sync側はこれを
    /// 「セッションが切れた、再接続して続きから再開すべき」信号として扱う。
    #[error("connection closed by server")]
    ConnectionClosed,
}

pub type Result<T> = std::result::Result<T, Pop3Error>;
