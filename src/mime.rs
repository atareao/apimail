//! Pure MIME parsing of a raw RFC 5322 message.
//!
//! This module interprets the bytes a mailbox hands back for a single message —
//! its plain-text and HTML bodies and its attachments — with no I/O, no network
//! and no global state. Everything is converted to owned types (`String`,
//! `Vec<u8>`) so no borrow of the source buffer outlives the call.

use mail_parser::{MessageParser, MimeHeaders};

/// A parsed message: its derived bodies and its attachments.
///
/// `text` and `html` are the message's plain-text and HTML bodies. Following
/// RFC 8621 §4.1.4, `mail-parser` derives the missing alternative from the one
/// that is present; a message with no displayable part reports `None` for both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedMessage {
    /// Plain-text body, or `None` when the message has no displayable part.
    pub text: Option<String>,
    /// HTML body, or `None` when the message has no displayable part.
    pub html: Option<String>,
    /// Attachments, in the positional order reported by `mail-parser`.
    pub attachments: Vec<ParsedAttachment>,
}

impl ParsedMessage {
    /// The attachment with the positional `id` (0-based), or `None` when the
    /// `id` is out of range.
    pub fn attachment(&self, id: u32) -> Option<&ParsedAttachment> {
        self.attachments
            .iter()
            .find(|attachment| attachment.id == id)
    }
}

/// One attachment: its metadata plus its decoded content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttachment {
    /// Positional identifier: the index within the parsed attachment list.
    pub id: u32,
    /// Decoded file name, if the server reported one (RFC 2047/2231 resolved).
    pub filename: Option<String>,
    /// `"{type}/{subtype}"`, or just `"{type}"` when there is no subtype, or
    /// `None` when the part carries no `Content-Type`.
    pub content_type: Option<String>,
    /// Length of the decoded content, in bytes.
    pub size: usize,
    /// Whether the part is marked `inline` by its `Content-Disposition`.
    pub inline: bool,
    /// `Content-ID`, if the part reported one.
    pub content_id: Option<String>,
    /// The decoded content (base64/quoted-printable already resolved).
    pub content: Vec<u8>,
}

/// Errors produced while parsing a raw message.
#[derive(Debug, thiserror::Error)]
pub enum MimeError {
    /// `MessageParser` found no usable headers in the raw bytes.
    #[error("message cannot be parsed")]
    Unparsable,
}

/// Parses the raw RFC 5322 message `raw`.
///
/// Returns [`MimeError::Unparsable`] when `MessageParser` cannot find any
/// headers. The parser is best-effort and never panics.
pub fn parse_message(raw: &[u8]) -> Result<ParsedMessage, MimeError> {
    let message = MessageParser::default()
        .parse(raw)
        .ok_or(MimeError::Unparsable)?;

    let text = message.body_text(0).map(|value| value.into_owned());
    let html = message.body_html(0).map(|value| value.into_owned());

    let attachments = message
        .attachments()
        .enumerate()
        .map(|(index, part)| ParsedAttachment {
            id: index as u32,
            filename: part.attachment_name().map(String::from),
            content_type: part
                .content_type()
                .map(|content_type| match content_type.subtype() {
                    Some(subtype) => format!("{}/{}", content_type.c_type, subtype),
                    None => content_type.c_type.to_string(),
                }),
            size: part.len(),
            inline: part
                .content_disposition()
                .is_some_and(|disposition| disposition.is_inline()),
            content_id: part.content_id().map(String::from),
            content: part.contents().to_vec(),
        })
        .collect();

    Ok(ParsedMessage {
        text,
        html,
        attachments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A multipart message carrying a text/plain part, a text/html part, a
    /// binary attachment with an RFC 2047 name and an inline image with an
    /// RFC 2231 name and a `Content-ID`.
    const MULTIPART: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Multipart test
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary=\"MIX\"

--MIX
Content-Type: multipart/alternative; boundary=\"ALT\"

--ALT
Content-Type: text/plain; charset=\"utf-8\"

Plain body
--ALT
Content-Type: text/html; charset=\"utf-8\"

<p>Html body</p>
--ALT--

--MIX
Content-Type: application/octet-stream
Content-Disposition: attachment; filename=\"=?UTF-8?B?Y2Fmw6kudHh0?=\"
Content-Transfer-Encoding: base64

SGVsbG8sIGJpbmFyeSE=
--MIX
Content-Type: image/png
Content-Disposition: inline; filename*=UTF-8''logo%20caf%C3%A9.png
Content-ID: <logo@example.com>
Content-Transfer-Encoding: base64

AAECAwQ=
--MIX--
";

    /// A message that only carries a text/plain part.
    const TEXT_ONLY: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Text only
MIME-Version: 1.0
Content-Type: text/plain; charset=\"utf-8\"

Hello plain world
";

    /// A message that only carries a text/html part.
    const HTML_ONLY: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Html only
MIME-Version: 1.0
Content-Type: text/html; charset=\"utf-8\"

<p>Hello <b>html</b> world</p>
";

    /// A message without a displayable body: a single binary attachment.
    const ATTACHMENT_ONLY: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Binary only
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary=\"B\"

--B
Content-Type: application/octet-stream
Content-Disposition: attachment; filename=\"data.bin\"
Content-Transfer-Encoding: base64

AAECAwQ=
--B--
";

    #[test]
    fn multipart_lists_attachments_in_positional_order_with_metadata() {
        let parsed = parse_message(MULTIPART.as_bytes()).expect("multipart must parse");

        assert_eq!(parsed.attachments.len(), 2, "{:?}", parsed.attachments);

        let first = parsed.attachment(0).expect("first attachment");
        assert_eq!(first.id, 0);
        assert_eq!(first.filename.as_deref(), Some("café.txt"));
        assert_eq!(
            first.content_type.as_deref(),
            Some("application/octet-stream")
        );
        assert_eq!(first.size, 14);
        assert_eq!(first.content, b"Hello, binary!");
        assert!(!first.inline, "an `attachment` disposition is not inline");
        assert_eq!(first.content_id, None);

        let second = parsed.attachment(1).expect("second attachment");
        assert_eq!(second.id, 1);
        assert_eq!(second.filename.as_deref(), Some("logo café.png"));
        assert_eq!(second.content_type.as_deref(), Some("image/png"));
        assert_eq!(second.size, 5);
        assert_eq!(second.content, [0x00, 0x01, 0x02, 0x03, 0x04]);
        assert!(second.inline, "the inline disposition must be reported");
        assert_eq!(second.content_id.as_deref(), Some("logo@example.com"));
    }

    #[test]
    fn multipart_exposes_the_text_and_html_bodies() {
        let parsed = parse_message(MULTIPART.as_bytes()).expect("multipart must parse");
        let text = parsed.text.expect("a text body must be derived");
        let html = parsed.html.expect("an html body must be derived");
        assert!(text.contains("Plain body"), "{text}");
        assert!(html.contains("<p>Html body</p>"), "{html}");
    }

    #[test]
    fn text_only_message_derives_the_html_alternative() {
        let parsed = parse_message(TEXT_ONLY.as_bytes()).expect("text-only must parse");
        let text = parsed.text.expect("the text body must be present");
        assert!(text.contains("Hello plain world"), "{text}");
        let html = parsed.html.expect("the html alternative must be derived");
        assert!(html.contains("Hello plain world"), "{html}");
        assert!(parsed.attachments.is_empty(), "{:?}", parsed.attachments);
    }

    #[test]
    fn html_only_message_derives_the_text_alternative() {
        let parsed = parse_message(HTML_ONLY.as_bytes()).expect("html-only must parse");
        let html = parsed.html.expect("the html body must be present");
        assert!(html.contains("<b>html</b>"), "{html}");
        let text = parsed.text.expect("the text alternative must be derived");
        assert!(text.contains("Hello"), "{text}");
        assert!(parsed.attachments.is_empty(), "{:?}", parsed.attachments);
    }

    #[test]
    fn message_without_a_displayable_body_has_no_bodies_but_one_attachment() {
        let parsed = parse_message(ATTACHMENT_ONLY.as_bytes()).expect("attachment-only must parse");
        assert_eq!(parsed.text, None);
        assert_eq!(parsed.html, None);
        assert_eq!(parsed.attachments.len(), 1, "{:?}", parsed.attachments);
        assert_eq!(parsed.attachments[0].filename.as_deref(), Some("data.bin"));
    }

    #[test]
    fn attachment_lookup_is_by_position_and_out_of_range_is_none() {
        let parsed = parse_message(MULTIPART.as_bytes()).expect("multipart must parse");
        assert_eq!(parsed.attachment(0).expect("0").id, 0);
        assert_eq!(parsed.attachment(1).expect("1").id, 1);
        assert!(parsed.attachment(2).is_none(), "2 is out of range");
        assert!(parsed.attachment(99).is_none(), "99 is out of range");
    }

    #[test]
    fn headerless_bytes_are_unparsable() {
        // `mail-parser` is best-effort: only bytes that yield neither a part nor
        // any parsed header (here, none at all) make it return `None`.
        let error = parse_message(b"").expect_err("empty bytes must fail");
        assert!(
            matches!(error, MimeError::Unparsable),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn mime_error_message_is_stable_and_free_of_third_party_text() {
        assert_eq!(
            MimeError::Unparsable.to_string(),
            "message cannot be parsed"
        );
    }
}
