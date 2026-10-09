//! Outgoing mail domain model and the SMTP transport.
//!
//! [`OutgoingMessage`] is the transport-agnostic description of a message to
//! send; [`SmtpSender`] turns it into a `lettre::Message` and delivers it over
//! the SMTP endpoint configured in `mail-account` (honouring its TLS mode). The
//! [`MailSender`] trait keeps the transport injectable, so the HTTP layer can be
//! exercised without touching the network.
//!
//! Attachment payloads reach this module **already decoded**: base64 decoding is
//! an HTTP concern (see `src/http.rs`), which is also where
//! [`MessageError::InvalidBase64`] and [`MessageError::TooLarge`] originate. The
//! transport therefore only ever sees raw bytes, measured against the configured
//! attachment limit at the boundary.

use std::future::Future;
use std::pin::Pin;

use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

use crate::config::{MailEndpoint, TlsMode};

/// MIME type used when an attachment has no explicit content type.
const DEFAULT_CONTENT_TYPE: &str = "application/octet-stream";

/// An attachment whose content has already been decoded into raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    /// File name presented to the recipient.
    pub filename: String,
    /// Optional MIME content type; defaults to `application/octet-stream`.
    pub content_type: Option<String>,
    /// Raw attachment bytes.
    pub data: Vec<u8>,
}

/// A message to be sent, independent of the transport used to deliver it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OutgoingMessage {
    /// Explicit sender; when `None` the configured SMTP user is used.
    pub from: Option<String>,
    /// Primary recipients (at least one is required).
    pub to: Vec<String>,
    /// Carbon-copy recipients.
    pub cc: Vec<String>,
    /// Blind carbon-copy recipients.
    pub bcc: Vec<String>,
    /// Subject line.
    pub subject: String,
    /// Optional plain-text body.
    pub text: Option<String>,
    /// Optional HTML body.
    pub html: Option<String>,
    /// Attachments.
    pub attachments: Vec<OutgoingAttachment>,
}

/// Errors produced while validating or converting an [`OutgoingMessage`].
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    /// No primary recipient was provided.
    #[error("at least one recipient (`to`) is required")]
    MissingRecipient,
    /// No sender was provided.
    #[error("a sender (`from`) is required")]
    MissingSender,
    /// An address was not a valid mailbox.
    #[error("invalid address in `{field}`: `{value}`")]
    InvalidAddress {
        /// Name of the offending field (`from`, `to`, `cc` or `bcc`).
        field: &'static str,
        /// The offending raw value.
        value: String,
    },
    /// An attachment's `data_base64` was not valid base64.
    #[error("attachment `{filename}` is not valid base64")]
    InvalidBase64 {
        /// File name of the offending attachment.
        filename: String,
    },
    /// The decoded attachments exceed the configured limit.
    #[error("attachments total {total} bytes, exceeding the limit of {limit} bytes")]
    TooLarge {
        /// Total decoded size in bytes.
        total: usize,
        /// Configured limit in bytes.
        limit: usize,
    },
    /// Building the underlying `lettre` message failed.
    #[error("failed to build the message: {0}")]
    Build(#[from] lettre::error::Error),
}

/// Errors produced by the SMTP transport.
#[derive(Debug, thiserror::Error)]
pub enum SmtpError {
    /// The message could not be validated or converted.
    #[error(transparent)]
    Message(#[from] MessageError),
    /// Creating the SMTP transport failed.
    #[error("failed to create the SMTP transport: {0}")]
    Transport(#[from] lettre::transport::smtp::Error),
    /// The SMTP server rejected or failed the delivery.
    #[error("SMTP delivery failed: {0}")]
    Delivery(String),
}

impl SmtpError {
    /// Stable, external-facing message that never carries third-party details.
    ///
    /// The HTTP layer uses this instead of [`Display`](std::fmt::Display) so no
    /// error text coming from `lettre`, rustls or the SMTP server itself (which
    /// may embed rejection banners, host names or addresses) ever reaches a
    /// client. Variants are grouped by failure family, keeping the response
    /// stable across versions of the underlying crates.
    pub fn public_message(&self) -> &'static str {
        match self {
            Self::Message(_) => "the message could not be validated",
            Self::Transport(_) => "failed to create the SMTP transport",
            Self::Delivery(_) => "SMTP delivery failed",
        }
    }

    /// Static label identifying the failure kind, for **logs only**.
    ///
    /// Unlike [`public_message`](Self::public_message) this is intended for
    /// tracing, where a compact, machine-filterable tag is more useful than the
    /// full [`Display`](std::fmt::Display) text. It is deliberately static and
    /// carries no server data.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Message(_) => "message",
            Self::Transport(_) => "transport",
            Self::Delivery(_) => "delivery",
        }
    }
}

/// Boxed future returned by [`MailSender::send`].
///
/// A named alias keeps the trait object-safe without repeating the (otherwise
/// lint-triggering) `Pin<Box<dyn Future..>>` type in every signature.
pub type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), SmtpError>> + Send + 'a>>;

/// A transport able to deliver an [`OutgoingMessage`].
pub trait MailSender: Send + Sync {
    /// Sends `message`, resolving the sender from the transport when the
    /// message itself does not carry a `from`.
    fn send(&self, message: OutgoingMessage) -> SendFuture<'_>;
}

/// SMTP [`MailSender`] backed by `lettre`'s asynchronous transport.
///
/// The transport is built once, at startup; it opens no connection until the
/// first [`send`](MailSender::send) call. It therefore holds the SMTP
/// credentials and must never be formatted verbatim (see `AppState`'s `Debug`).
/// The effective sender is **not** stored here: the HTTP layer resolves it (into
/// [`OutgoingMessage::from`]) from the single source of truth in `AppState`.
pub struct SmtpSender {
    /// Pre-built asynchronous SMTP transport.
    transport: AsyncSmtpTransport<Tokio1Executor>,
}

/// The TLS strategy selected for an SMTP endpoint.
///
/// Kept as a small pure value so the mapping from [`TlsMode`] can be unit-tested
/// without building a transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TlsStrategy {
    /// Implicit TLS (`relay`).
    Implicit,
    /// Opportunistic in-band upgrade (`starttls_relay`).
    StartTls,
    /// No TLS (`builder_dangerous(..).tls(Tls::None)`).
    Plain,
}

/// Maps a configured [`TlsMode`] to the transport strategy to build.
fn tls_strategy(mode: TlsMode) -> TlsStrategy {
    match mode {
        TlsMode::Implicit => TlsStrategy::Implicit,
        TlsMode::StartTls => TlsStrategy::StartTls,
        TlsMode::Plain => TlsStrategy::Plain,
    }
}

impl SmtpSender {
    /// Builds the transport for `endpoint` without opening any connection.
    ///
    /// The TLS mode mirrors `mail-account`: `implicit` uses an implicitly
    /// encrypted connection, `starttls` upgrades in-band, and `none` disables
    /// TLS entirely (with the same warning already emitted while loading the
    /// account). The strategy is chosen via [`tls_strategy`].
    pub fn from_endpoint(endpoint: &MailEndpoint) -> Result<Self, SmtpError> {
        let builder = match tls_strategy(endpoint.tls) {
            TlsStrategy::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&endpoint.host)?,
            TlsStrategy::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&endpoint.host)?
            }
            TlsStrategy::Plain => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&endpoint.host)
                    .tls(Tls::None)
            }
        };
        let transport = builder
            .port(endpoint.port)
            .credentials(Credentials::new(
                endpoint.username.clone(),
                endpoint.password.clone(),
            ))
            .build();
        Ok(Self { transport })
    }
}

impl MailSender for SmtpSender {
    fn send(&self, message: OutgoingMessage) -> SendFuture<'_> {
        Box::pin(async move {
            let email = message.to_lettre()?;
            self.transport
                .send(email)
                .await
                .map_err(|error| SmtpError::Delivery(error.to_string()))?;
            Ok(())
        })
    }
}

/// Parses `value` as a `lettre` mailbox, mapping failures to
/// [`MessageError::InvalidAddress`].
fn parse_mailbox(field: &'static str, value: &str) -> Result<Mailbox, MessageError> {
    value
        .parse::<Mailbox>()
        .map_err(|_| MessageError::InvalidAddress {
            field,
            value: value.to_string(),
        })
}

/// The content type of an attachment, falling back to
/// `application/octet-stream` for an absent or invalid value.
fn attachment_content_type(raw: Option<&str>) -> ContentType {
    raw.and_then(|value| ContentType::parse(value).ok())
        .unwrap_or_else(|| {
            ContentType::parse(DEFAULT_CONTENT_TYPE)
                .expect("`application/octet-stream` is a valid MIME type")
        })
}

impl OutgoingMessage {
    /// Validates that a sender and a recipient exist and that every address is a
    /// valid mailbox.
    pub fn validate(&self) -> Result<(), MessageError> {
        if self.to.is_empty() {
            return Err(MessageError::MissingRecipient);
        }
        let sender = self.from.as_deref().ok_or(MessageError::MissingSender)?;
        parse_mailbox("from", sender)?;
        for value in &self.to {
            parse_mailbox("to", value)?;
        }
        for value in &self.cc {
            parse_mailbox("cc", value)?;
        }
        for value in &self.bcc {
            parse_mailbox("bcc", value)?;
        }
        Ok(())
    }

    /// Converts the domain message into a `lettre::Message`.
    ///
    /// A sender is **required**: callers are expected to have resolved
    /// [`from`](Self::from) already, so a missing sender is a
    /// [`MessageError::MissingSender`] rather than a silent fallback. Bodies are
    /// composed as an `alternative` multipart when both `text` and `html` are
    /// present, and wrapped in a `mixed` multipart when there are attachments.
    pub fn to_lettre(&self) -> Result<Message, MessageError> {
        let sender = self.from.as_deref().ok_or(MessageError::MissingSender)?;
        let mut builder = Message::builder()
            .subject(self.subject.clone())
            .from(parse_mailbox("from", sender)?);
        for value in &self.to {
            builder = builder.to(parse_mailbox("to", value)?);
        }
        for value in &self.cc {
            builder = builder.cc(parse_mailbox("cc", value)?);
        }
        for value in &self.bcc {
            builder = builder.bcc(parse_mailbox("bcc", value)?);
        }

        if self.attachments.is_empty() {
            return match (&self.text, &self.html) {
                (Some(text), Some(html)) => builder
                    .multipart(MultiPart::alternative_plain_html(
                        text.clone(),
                        html.clone(),
                    ))
                    .map_err(MessageError::from),
                (None, Some(html)) => builder
                    .header(ContentType::TEXT_HTML)
                    .body(html.clone())
                    .map_err(MessageError::from),
                (Some(text), None) => builder.body(text.clone()).map_err(MessageError::from),
                (None, None) => builder.body(String::new()).map_err(MessageError::from),
            };
        }

        let mut multipart = match (&self.text, &self.html) {
            (Some(text), Some(html)) => MultiPart::mixed().multipart(
                MultiPart::alternative_plain_html(text.clone(), html.clone()),
            ),
            (None, Some(html)) => MultiPart::mixed().singlepart(SinglePart::html(html.clone())),
            (Some(text), None) => MultiPart::mixed().singlepart(SinglePart::plain(text.clone())),
            (None, None) => MultiPart::mixed().singlepart(SinglePart::plain(String::new())),
        };
        for attachment in &self.attachments {
            let content_type = attachment_content_type(attachment.content_type.as_deref());
            multipart = multipart.singlepart(
                Attachment::new(attachment.filename.clone())
                    .body(attachment.data.clone(), content_type),
            );
        }
        builder.multipart(multipart).map_err(MessageError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a message with the given sender and recipients.
    fn message_with(from: Option<&str>, to: &[&str]) -> OutgoingMessage {
        OutgoingMessage {
            from: from.map(str::to_string),
            to: to.iter().map(|value| value.to_string()).collect(),
            subject: "hello".to_string(),
            ..OutgoingMessage::default()
        }
    }

    /// Renders the wire form of a message for header assertions.
    fn render(message: &OutgoingMessage) -> String {
        let email = message.to_lettre().expect("message should build");
        String::from_utf8_lossy(&email.formatted()).into_owned()
    }

    /// An SMTP endpoint for the given TLS mode.
    fn endpoint(mode: TlsMode) -> MailEndpoint {
        MailEndpoint {
            host: "smtp.test.example".to_string(),
            port: 587,
            tls: mode,
            username: "user@test.example".to_string(),
            password: "secret".to_string(),
        }
    }

    #[test]
    fn explicit_sender_is_used() {
        let message = message_with(Some("explicit@test.example"), &["to@test.example"]);
        let raw = render(&message);
        assert!(raw.contains("explicit@test.example"), "{raw}");
    }

    #[test]
    fn missing_sender_is_rejected() {
        let message = message_with(None, &["to@test.example"]);
        let err = message.to_lettre().expect_err("missing sender must fail");
        assert!(matches!(err, MessageError::MissingSender));
    }

    #[test]
    fn text_and_html_use_an_alternative_multipart() {
        let mut message = message_with(Some("from@test.example"), &["to@test.example"]);
        message.text = Some("plain body".to_string());
        message.html = Some("<p>html body</p>".to_string());
        let raw = render(&message);
        assert!(raw.contains("multipart/alternative"), "{raw}");
        assert!(raw.contains("plain body"), "{raw}");
        assert!(raw.contains("html body"), "{raw}");
    }

    #[test]
    fn attachment_keeps_filename_and_content_type() {
        let mut message = message_with(Some("from@test.example"), &["to@test.example"]);
        message.attachments.push(OutgoingAttachment {
            filename: "note.txt".to_string(),
            content_type: Some("text/plain".to_string()),
            data: b"contents".to_vec(),
        });
        let raw = render(&message);
        assert!(raw.contains("multipart/mixed"), "{raw}");
        assert!(raw.contains("note.txt"), "{raw}");
        assert!(raw.contains("text/plain"), "{raw}");
    }

    #[test]
    fn attachment_defaults_to_octet_stream() {
        let mut message = message_with(Some("from@test.example"), &["to@test.example"]);
        message.attachments.push(OutgoingAttachment {
            filename: "blob.bin".to_string(),
            content_type: None,
            data: vec![0, 1, 2, 3],
        });
        let raw = render(&message);
        assert!(raw.contains(DEFAULT_CONTENT_TYPE), "{raw}");
    }

    #[test]
    fn invalid_recipient_is_rejected() {
        let message = message_with(Some("from@test.example"), &["not an address"]);
        let err = message
            .to_lettre()
            .expect_err("invalid recipient must fail");
        match err {
            MessageError::InvalidAddress { field, value } => {
                assert_eq!(field, "to");
                assert_eq!(value, "not an address");
            }
            other => panic!("expected InvalidAddress, got {other:?}"),
        }
    }

    #[test]
    fn invalid_sender_is_rejected() {
        let message = message_with(Some("not-an-address"), &["to@test.example"]);
        let err = message.to_lettre().expect_err("invalid sender must fail");
        match err {
            MessageError::InvalidAddress { field, .. } => assert_eq!(field, "from"),
            other => panic!("expected InvalidAddress, got {other:?}"),
        }
    }

    #[test]
    fn validate_requires_a_recipient() {
        let err = OutgoingMessage::default()
            .validate()
            .expect_err("no recipient must fail");
        assert!(matches!(err, MessageError::MissingRecipient));
    }

    #[test]
    fn validate_requires_a_sender() {
        let message = message_with(None, &["to@test.example"]);
        let err = message.validate().expect_err("no sender must fail");
        assert!(matches!(err, MessageError::MissingSender));
    }

    #[test]
    fn validate_accepts_a_well_formed_message() {
        let mut message = message_with(Some("from@test.example"), &["to@test.example"]);
        message.cc = vec!["cc@test.example".to_string()];
        message.bcc = vec!["bcc@test.example".to_string()];
        message
            .validate()
            .expect("well-formed message should validate");
    }

    #[test]
    fn tls_strategy_maps_each_mode() {
        assert_eq!(tls_strategy(TlsMode::Implicit), TlsStrategy::Implicit);
        assert_eq!(tls_strategy(TlsMode::StartTls), TlsStrategy::StartTls);
        assert_eq!(tls_strategy(TlsMode::Plain), TlsStrategy::Plain);
    }

    #[test]
    fn from_endpoint_builds_for_every_tls_mode() {
        for mode in [TlsMode::Implicit, TlsMode::StartTls, TlsMode::Plain] {
            SmtpSender::from_endpoint(&endpoint(mode))
                .expect("transport should build without opening a connection");
        }
    }

    #[test]
    fn message_error_variants_render_their_context() {
        assert!(
            MessageError::MissingRecipient
                .to_string()
                .contains("recipient")
        );
        assert!(MessageError::MissingSender.to_string().contains("sender"));

        let base64 = MessageError::InvalidBase64 {
            filename: "a.bin".to_string(),
        };
        assert!(base64.to_string().contains("a.bin"), "{base64}");

        let too_large = MessageError::TooLarge {
            total: 20,
            limit: 10,
        };
        let rendered = too_large.to_string();
        assert!(
            rendered.contains("20") && rendered.contains("10"),
            "{rendered}"
        );
    }

    #[test]
    fn public_message_and_kind_map_every_variant_without_leaking_details() {
        // Third-party text (host + banner) that the raw `Display` may carry but
        // `public_message` must never expose.
        let message = SmtpError::Message(MessageError::InvalidAddress {
            field: "to",
            value: "leaked@internal.example".to_string(),
        });
        let delivery = SmtpError::Delivery("smtp.internal.example said: 550 rejected".to_string());

        for (error, public, kind) in [
            (&message, "the message could not be validated", "message"),
            (&delivery, "SMTP delivery failed", "delivery"),
        ] {
            assert_eq!(error.public_message(), public, "{error:?}");
            assert_eq!(error.kind(), kind, "{error:?}");
            assert!(
                !error.public_message().contains("internal.example"),
                "the public message leaked a host name: {}",
                error.public_message()
            );
            assert!(
                !error.public_message().contains("550 rejected"),
                "the public message leaked a server banner: {}",
                error.public_message()
            );
        }
    }

    #[test]
    fn delivery_display_carries_the_raw_text_but_public_message_does_not() {
        // Guards the contract: the diagnostic `Display` keeps the upstream text
        // (so the leak is real if it were ever used for an HTTP body), while
        // `public_message` is the sanitised, external face.
        let delivery = SmtpError::Delivery("smtp.internal.example said: 550 rejected".to_string());
        assert!(
            delivery.to_string().contains("smtp.internal.example"),
            "the Display is expected to keep the raw upstream text: {delivery}"
        );
        assert_eq!(delivery.public_message(), "SMTP delivery failed");
        assert!(!delivery.public_message().contains("internal.example"));
    }

    // `SmtpError::Transport(lettre::transport::smtp::Error)` is intentionally not
    // covered here: `lettre`'s SMTP error constructors are `pub(crate)`, so it
    // cannot be built from an integration/unit test. Its mapping is still
    // exhaustively matched in `public_message`/`kind`, so a future lettre change
    // can only fail at compile time.
}
