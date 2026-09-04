use mail_parser::{MessageParser, MimeHeaders};

/// メッセージ詳細画面で一覧表示するための添付ファイルのメタデータ。
/// `index`は同じ生バイト列に対して`extract_attachment_bytes`を呼ぶ際の指定に使う
/// （メッセージ内での添付の出現順。DBには持たず、都度Maildirから読み直して求める）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentInfo {
    pub index: usize,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub size: usize,
}

fn content_type_string(part: &mail_parser::MessagePart) -> Option<String> {
    let ct = part.content_type()?;
    Some(match &ct.c_subtype {
        Some(sub) => format!("{}/{}", ct.c_type, sub),
        None => ct.c_type.to_string(),
    })
}

/// 生メールから添付ファイルの一覧を取り出す（本文には興味がないため中身は返さない）。
pub fn list_attachments(raw: &[u8]) -> Vec<AttachmentInfo> {
    let Some(message) = MessageParser::default().parse(raw) else {
        return Vec::new();
    };

    message
        .attachments()
        .enumerate()
        .map(|(index, part)| AttachmentInfo {
            index,
            filename: part.attachment_name().map(str::to_string),
            content_type: content_type_string(part),
            size: part.len(),
        })
        .collect()
}

/// `list_attachments`が返した`index`に対応する添付の実バイト列（デコード済み）を取り出す。
pub fn extract_attachment_bytes(raw: &[u8], index: usize) -> Option<Vec<u8>> {
    let message = MessageParser::default().parse(raw)?;
    message.attachments().nth(index).map(|part| part.contents().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSAGE_WITH_ATTACHMENT: &[u8] = b"From: a@example.com\r\n\
Content-Type: multipart/mixed; boundary=\"B\"\r\n\
\r\n\
--B\r\n\
Content-Type: text/plain\r\n\
\r\n\
body text\r\n\
--B\r\n\
Content-Type: text/plain; name=\"hello.txt\"\r\n\
Content-Disposition: attachment; filename=\"hello.txt\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
aGVsbG8gd29ybGQ=\r\n\
--B--\r\n";

    #[test]
    fn lists_attachment_metadata() {
        let attachments = list_attachments(MESSAGE_WITH_ATTACHMENT);
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename.as_deref(), Some("hello.txt"));
        assert_eq!(attachments[0].content_type.as_deref(), Some("text/plain"));
        assert_eq!(attachments[0].size, b"hello world".len());
    }

    #[test]
    fn extracts_decoded_attachment_bytes() {
        let bytes = extract_attachment_bytes(MESSAGE_WITH_ATTACHMENT, 0).unwrap();
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn returns_empty_for_message_without_attachments() {
        let raw = b"Subject: hi\r\n\r\nbody";
        assert!(list_attachments(raw).is_empty());
        assert!(extract_attachment_bytes(raw, 0).is_none());
    }
}
