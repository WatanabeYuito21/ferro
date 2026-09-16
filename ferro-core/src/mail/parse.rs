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

fn headers_from_message(raw: &[u8], message: &Message) -> ParsedHeaders {
    let from = message.from().and_then(|addr| addr.first());
    let to = message.to().and_then(|addr| addr.first());

    let subject = message.subject().map(str::to_string);
    // 件名がRFC2047エンコード(=?charset?...?=)されておらず生のlegacy charsetバイト列
    // （Shift_JIS/EUC-JP/ISO-2022-JP）のまま送られてくる（技術的には仕様違反だが、
    // 古い日本語のcron/監視アラート系メールサーバーで実在する）と、mail-parserは
    // 未対応の生ヘッダーを`String::from_utf8_lossy`で変換するため、不正なバイト列が
    // U+FFFDに置き換わって文字化けする（実際にセキュリティ監視アラートメールの件名で
    // 踏んだ）。`recover_mojibake_header`のドキュメント参照。
    let subject = recover_mojibake_header(raw, "Subject", subject.as_deref()).or(subject);

    ParsedHeaders {
        message_id: message.message_id().map(str::to_string),
        subject,
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
    headers_from_message(raw, &message)
}

/// `decoded`（mail-parserが返した文字列）がU+FFFDを含む場合にのみ、`raw`から
/// `header_name`の生のヘッダー値バイト列を取り出し、legacy日本語charsetとして
/// 再デコードを試みる。ASCII/UTF-8/RFC2047で正しくデコードできている大多数の
/// ケースでは`decoded`にU+FFFDが含まれないため即座にNoneを返し、一切関与しない。
/// 再デコードにも失敗した場合はNoneを返し、呼び出し側はmail-parserの元の
/// （文字化けしたままの）結果をそのまま使う（今より悪化はしない）。
fn recover_mojibake_header(raw: &[u8], header_name: &str, decoded: Option<&str>) -> Option<String> {
    let decoded = decoded?;
    if !decoded.contains('\u{FFFD}') {
        return None;
    }
    let raw_value = raw_header_value(raw, header_name)?;
    try_decode_legacy_japanese(&raw_value)
}

/// Shift_JIS → EUC-JP → ISO-2022-JPの順に試し、変換エラーが無くU+FFFDも残らない
/// 最初の結果を返す（`recover_mojibake_header`のドキュメント参照）。Shift_JISを
/// 最初に試すのは、raw非エンコードのlegacy日本語件名を送ってくる送信元では
/// 最も多いと見られるため。
fn try_decode_legacy_japanese(bytes: &[u8]) -> Option<String> {
    for encoding in [encoding_rs::SHIFT_JIS, encoding_rs::EUC_JP, encoding_rs::ISO_2022_JP] {
        let (decoded, _, had_errors) = encoding.decode(bytes);
        if !had_errors && !decoded.contains('\u{FFFD}') {
            return Some(decoded.into_owned());
        }
    }
    None
}

/// RFC 822のヘッダー折り返し（継続行）を考慮し、`raw`から`name`という名前の
/// ヘッダーの値（先頭のコロン直後の空白は読み飛ばし、折り返しの改行は空白1つに
/// 潰して連結する）を生バイト列のまま取り出す。大文字小文字は区別しない。
/// 同名ヘッダーが複数あれば最初の1つだけを返す。見つからなければNone。
///
/// `recover_mojibake_header`からのみ呼ばれるヒューリスティックな救済経路であり、
/// RFC 822の全ての例外（folding以外の変則的な書式等）を厳密に処理する必要は無い
/// （抽出に失敗すれば単に救済しないだけで、mail-parser側の結果がそのまま使われる
/// ため悪化はしない）。
fn raw_header_value(raw: &[u8], name: &str) -> Option<Vec<u8>> {
    let name_bytes = name.as_bytes();
    let mut pos = 0usize;

    while pos < raw.len() {
        // ヘッダー部の終わり（空行）に達したら、これ以上ヘッダーは無い。
        if raw[pos] == b'\n' || raw[pos..].starts_with(b"\r\n") {
            return None;
        }

        let line_end = find_line_end(raw, pos);
        let colon = raw[pos..line_end].iter().position(|&b| b == b':');
        if let Some(colon) = colon {
            let field_name = trim_ascii(&raw[pos..pos + colon]);
            if field_name.eq_ignore_ascii_case(name_bytes) {
                let mut value_start = pos + colon + 1;
                while value_start < line_end && matches!(raw[value_start], b' ' | b'\t') {
                    value_start += 1;
                }
                let mut value = raw[value_start..line_end].to_vec();

                // 折り返し継続行（次の行が空白/タブで始まる）を、改行を空白1つに
                // 潰しながら連結する。
                let mut next = advance_past_newline(raw, line_end);
                while next < raw.len() && matches!(raw[next], b' ' | b'\t') {
                    let cont_end = find_line_end(raw, next);
                    value.push(b' ');
                    value.extend_from_slice(trim_ascii(&raw[next..cont_end]));
                    next = advance_past_newline(raw, cont_end);
                }
                return Some(value);
            }
        }

        pos = advance_past_newline(raw, line_end);
    }
    None
}

/// `from`以降で最初の改行までの範囲を返す（戻り値は`\r`か`\n`を指すか、
/// 見つからなければ`raw.len()`。`\r\n`の場合は`\r`の位置を返す）。
fn find_line_end(raw: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < raw.len() && raw[i] != b'\n' {
        i += 1;
    }
    if i > from && raw[i - 1] == b'\r' { i - 1 } else { i }
}

/// `find_line_end`が指す位置から、改行(`\r\n`または`\n`)の直後まで進める。
fn advance_past_newline(raw: &[u8], idx: usize) -> usize {
    let mut i = idx;
    if i < raw.len() && raw[i] == b'\r' {
        i += 1;
    }
    if i < raw.len() && raw[i] == b'\n' {
        i += 1;
    }
    i
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(bytes.len());
    let end = bytes.iter().rposition(|b| !b.is_ascii_whitespace()).map_or(start, |p| p + 1);
    &bytes[start..end]
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
        headers: headers_from_message(raw, &message),
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

/// 一覧のプレビュー行用に、本文冒頭を改行を潰した1行に切り詰める。
/// マルチバイト文字境界で壊れないよう`chars()`単位で切る。
///
/// 表示上は一覧の行の`text-overflow: ellipsis`でどのみち1行分しか見えないが、
/// この値は色分けルール（`ColorRule`）のマッチ対象としても使われる（実体を毎回
/// Maildirから読み直すのは「起動時に何も舐めない」方針に反するため、一覧描画時に
/// 使えるのはこのフィールドだけ）。Nagios等の監視アラートメールは本文が長く、
/// 短すぎると肝心のキーワードが切り詰められた先にあって色分けルールが効かない
/// ことがある（実際に踏んだ）ため、単なる表示用プレビューよりやや余裕を持たせている。
pub fn make_preview(body: Option<&str>) -> Option<String> {
    const MAX_CHARS: usize = 500;
    let collapsed: String = body?.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(collapsed.chars().take(MAX_CHARS).collect())
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

    #[test]
    fn make_preview_collapses_whitespace_and_truncates() {
        assert_eq!(make_preview(Some("hello   \n world")).as_deref(), Some("hello world"));
        assert_eq!(make_preview(None), None);
        assert_eq!(make_preview(Some("   ")), None);

        let long_body = "a".repeat(600);
        let preview = make_preview(Some(&long_body)).unwrap();
        assert_eq!(preview.chars().count(), 500);
    }

    /// 実際にセキュリティ監視アラートメールの件名で踏んだ回帰テスト: RFC2047の
    /// `=?charset?...?=`で包まれず、生のShift_JISバイト列がそのままSubjectヘッダーに
    /// 入っているメール（古い日本語のcron/監視アラート系サーバーで実在する）は、
    /// mail-parserの標準経路だと`String::from_utf8_lossy`でU+FFFDに置き換わって
    /// 文字化けする。`recover_mojibake_header`が生ヘッダーバイト列をShift_JISとして
    /// 再デコードし、正しい件名を復元できることを確認する。
    #[test]
    fn recovers_subject_sent_as_raw_unencoded_shift_jis() {
        let subject_text = "悪意のあるSQLもしくは決済系テンプレート等の更新を検知";
        let (shift_jis_bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(subject_text);
        assert!(!had_errors, "test fixture text must be representable in Shift_JIS");

        let mut raw = Vec::new();
        raw.extend_from_slice(b"Subject: ");
        raw.extend_from_slice(&shift_jis_bytes);
        raw.extend_from_slice(b"\r\nFrom: alert@example.com\r\n\r\nbody");

        let headers = parse_headers(&raw);
        assert_eq!(headers.subject.as_deref(), Some(subject_text));
    }

    /// 同じ回帰を`parse_full`（sync時の索引投入経路）でも確認する。
    #[test]
    fn parse_full_also_recovers_raw_shift_jis_subject() {
        let subject_text = "重要なお知らせ";
        let (shift_jis_bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(subject_text);
        assert!(!had_errors);

        let mut raw = Vec::new();
        raw.extend_from_slice(b"Subject: ");
        raw.extend_from_slice(&shift_jis_bytes);
        raw.extend_from_slice(b"\r\n\r\nbody");

        let parsed = parse_full(&raw);
        assert_eq!(parsed.headers.subject.as_deref(), Some(subject_text));
    }

    /// 折り返し（継続行）を含む生ヘッダーからも値を取り出せることを確認する
    /// （`raw_header_value`のfolding処理）。
    #[test]
    fn recovers_subject_split_across_a_folded_continuation_line() {
        let subject_text = "長い件名のテスト継続行";
        let (shift_jis_bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode(subject_text);
        assert!(!had_errors);
        // この文字列はASCIIを含まず全文字が2バイトなので、偶数バイト位置で
        // 割れば必ず文字境界になる（1バイト目と2バイト目の間で割らない）。
        let mid = (shift_jis_bytes.len() / 2) & !1;

        let mut raw = Vec::new();
        raw.extend_from_slice(b"Subject: ");
        raw.extend_from_slice(&shift_jis_bytes[..mid]);
        raw.extend_from_slice(b"\r\n ");
        raw.extend_from_slice(&shift_jis_bytes[mid..]);
        raw.extend_from_slice(b"\r\n\r\nbody");

        let headers = parse_headers(&raw);
        // `raw_header_value`は折り返しの改行を空白1つに潰して連結する
        // （RFC 822のfolding/unfolding仕様どおり）ため、継続行の境界に本来無かった
        // 空白が1つ入る。それを除けば元の文字列と一致するはず。
        assert_eq!(headers.subject.as_deref().unwrap().replace(' ', ""), subject_text);
    }

    /// 正しくデコードできている（U+FFFDを含まない）件名には一切関与しないことを確認する
    /// （ASCII・RFC2047エンコード済みの通常ケースを壊さない）。
    #[test]
    fn does_not_touch_subjects_that_already_decoded_cleanly() {
        let ascii = parse_headers(b"Subject: Hello\r\n\r\nbody");
        assert_eq!(ascii.subject.as_deref(), Some("Hello"));

        let rfc2047 = parse_headers(b"Subject: =?UTF-8?B?44GT44KT44Gr44Gh44Gv?=\r\n\r\nbody");
        assert_eq!(rfc2047.subject.as_deref(), Some("こんにちは"));
    }

    /// legacy charsetのどれで再デコードしても変換エラーが残る（=どの候補にも
    /// 一致しない）場合は、mail-parserの元の結果（文字化けしたまま）を諦めて
    /// そのまま返すことを確認する（悪化させない）。
    #[test]
    fn falls_back_to_the_original_lossy_text_when_no_legacy_charset_decodes_cleanly() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"Subject: ");
        // どのlegacy charsetとしても正しい2バイト文字列にならないよう、単独の
        // 高位バイトを1つだけ置く（有効な2バイトシーケンスを構成しない）。
        raw.push(0x81);
        raw.extend_from_slice(b"\r\n\r\nbody");

        let headers = parse_headers(&raw);
        // 復元できないので、mail-parserのlossy変換結果(U+FFFDを含む)がそのまま残る。
        assert!(headers.subject.as_deref().unwrap().contains('\u{FFFD}'));
    }
}
