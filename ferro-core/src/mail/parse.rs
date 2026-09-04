use mail_parser::MessageParser;

/// syncがDBの構造化カラムを埋めるために必要な最小限のヘッダー情報。
/// 本文・添付ファイルの解析は行わない（一覧表示専用の軽量パス）。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedHeaders {
    pub message_id: Option<String>,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub from_addr: Option<String>,
    pub to_addr: Option<String>,
    /// DateヘッダーをパースしたUnixエポック秒。ヘッダーが無い/パース不能ならNone。
    pub date_header: Option<i64>,
}

/// 生メールのバイト列からヘッダーのみを解析する（本文はデコードしないため高速）。
/// パース自体に失敗した場合（壊れたメール等）も全フィールドNoneでOkを返し、
/// 呼び出し側で件名等が空のメッセージとして扱えるようにする。
pub fn parse_headers(raw: &[u8]) -> ParsedHeaders {
    let Some(message) = MessageParser::default().parse_headers(raw) else {
        return ParsedHeaders::default();
    };

    let from = message.from().and_then(|addr| addr.first());
    let to = message.to().and_then(|addr| addr.first());

    ParsedHeaders {
        message_id: message.message_id().map(str::to_string),
        subject: message.subject().map(str::to_string),
        from_name: from.and_then(|a| a.name()).map(str::to_string),
        from_addr: from.and_then(|a| a.address()).map(str::to_string),
        to_addr: to.and_then(|a| a.address()).map(str::to_string),
        date_header: message.date().map(|d| d.to_timestamp()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_common_headers() {
        let raw = b"From: Alice <alice@example.com>\r\n\
                    To: Bob <bob@example.com>\r\n\
                    Subject: Hello\r\n\
                    Date: Mon, 1 Jan 2024 09:00:00 +0900\r\n\
                    Message-ID: <abc@example.com>\r\n\
                    \r\n\
                    body";

        let headers = parse_headers(raw);
        assert_eq!(headers.subject.as_deref(), Some("Hello"));
        assert_eq!(headers.from_name.as_deref(), Some("Alice"));
        assert_eq!(headers.from_addr.as_deref(), Some("alice@example.com"));
        assert_eq!(headers.to_addr.as_deref(), Some("bob@example.com"));
        assert_eq!(headers.message_id.as_deref(), Some("abc@example.com"));
        assert!(headers.date_header.is_some());
    }

    #[test]
    fn handles_missing_headers_gracefully() {
        let raw = b"Subject: only subject\r\n\r\nbody";
        let headers = parse_headers(raw);
        assert_eq!(headers.subject.as_deref(), Some("only subject"));
        assert_eq!(headers.from_addr, None);
        assert_eq!(headers.date_header, None);
    }
}
