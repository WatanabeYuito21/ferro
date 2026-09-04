use mail_parser::{Message, MessageParser};

/// syncがDBの構造化カラムを埋めるために必要な最小限のヘッダー情報。
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

fn headers_from_message(message: &Message) -> ParsedHeaders {
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

/// 生メールのバイト列からヘッダーのみを解析する（本文はデコードしないため高速）。
/// パース自体に失敗した場合（壊れたメール等）も全フィールドNoneでOkを返し、
/// 呼び出し側で件名等が空のメッセージとして扱えるようにする。
pub fn parse_headers(raw: &[u8]) -> ParsedHeaders {
    let Some(message) = MessageParser::default().parse_headers(raw) else {
        return ParsedHeaders::default();
    };
    headers_from_message(&message)
}

/// ヘッダーとプレーンテキスト本文の両方が要る場面（sync時の索引投入等）用に、
/// 生バイト列を一度だけフルパースしてまとめて取り出す。
/// `parse_headers`＋`extract_plain_text_body`を両方呼ぶと二重にパースすることになる。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    pub headers: ParsedHeaders,
    pub body: Option<String>,
}

pub fn parse_full(raw: &[u8]) -> ParsedMessage {
    let Some(message) = MessageParser::default().parse(raw) else {
        return ParsedMessage::default();
    };
    ParsedMessage {
        headers: headers_from_message(&message),
        body: message.body_text(0).map(|body| body.into_owned()),
    }
}

/// 全文検索インデックス投入・本文プレビュー表示用にプレーンテキスト本文を取り出す。
/// text/htmlしか無いメールはタグを剥がしたテキストに変換する
/// （`mail-parser`の`body_text`が変換まで面倒を見てくれる）。
/// ヘッダーも要るなら二重パースを避けるため`parse_full`を使うこと。
pub fn extract_plain_text_body(raw: &[u8]) -> Option<String> {
    let message = MessageParser::default().parse(raw)?;
    message.body_text(0).map(|body| body.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_MESSAGE: &[u8] = b"From: Alice <alice@example.com>\r\n\
                    To: Bob <bob@example.com>\r\n\
                    Subject: Hello\r\n\
                    Date: Mon, 1 Jan 2024 09:00:00 +0900\r\n\
                    Message-ID: <abc@example.com>\r\n\
                    \r\n\
                    body text";

    #[test]
    fn extracts_common_headers() {
        let headers = parse_headers(SAMPLE_MESSAGE);
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

    #[test]
    fn extracts_plain_text_body() {
        let raw = b"Subject: hi\r\nContent-Type: text/plain\r\n\r\nhello world";
        assert_eq!(
            extract_plain_text_body(raw).as_deref(),
            Some("hello world")
        );
    }

    #[test]
    fn falls_back_to_stripped_html_when_no_plain_text_part() {
        let raw = b"Subject: hi\r\nContent-Type: text/html\r\n\r\n<p>hello <b>world</b></p>";
        let body = extract_plain_text_body(raw).expect("should extract a body");
        assert!(body.contains("hello"));
        assert!(!body.contains("<p>"));
    }

    #[test]
    fn parse_full_returns_both_headers_and_body_in_one_pass() {
        let parsed = parse_full(SAMPLE_MESSAGE);
        assert_eq!(parsed.headers.subject.as_deref(), Some("Hello"));
        assert_eq!(parsed.body.as_deref(), Some("body text"));
    }
}
