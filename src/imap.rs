//! Incoming mail domain model and the IMAP connection layer.
//!
//! This module establishes, reuses and reconnects the IMAP session against the
//! endpoint configured in `mail-account`. The connection is **lazy**: nothing is
//! opened at startup; the session is established on first use, kept alive, and
//! replaced with a bounded exponential backoff when it is found dead.
//!
//! The connection is injectable through the [`ImapConnector`] trait, so the HTTP
//! layer can be exercised without touching the network. The real implementation,
//! [`TokioImapConnector`], performs TCP + TLS + `LOGIN` with `async-imap` and
//! **rustls** (ring provider), honouring the three TLS modes defined in
//! `mail-account` (`implicit`, `starttls`, `none`).
//!
//! Secrets (the IMAP password) never leave this module: [`ConnectionManager`]
//! implements [`Debug`](std::fmt::Debug) manually and redacts the connector and
//! the cached session.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_imap::Client;
use async_imap::types::{Fetch, Flag, Mailbox, NameAttribute};
use futures_util::TryStreamExt;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::config::{Config, MailEndpoint, TlsMode};
use crate::mime::{MimeError, ParsedMessage};

/// Boxed future returned by the [`ImapSession`] and [`ImapConnector`] traits.
///
/// The traits stay object-safe without `async-trait`, mirroring the
/// [`MailSender`](crate::smtp::MailSender) pattern.
pub type SendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Errors produced while establishing or using an IMAP session.
#[derive(Debug, thiserror::Error)]
pub enum ImapError {
    /// The TCP connection could not be opened.
    #[error("failed to connect to the IMAP server: {0}")]
    Tcp(#[source] std::io::Error),
    /// The TLS handshake failed.
    #[error("TLS handshake failed: {0}")]
    Tls(String),
    /// Building the rustls client configuration failed.
    #[error("failed to build the TLS configuration: {0}")]
    TlsConfig(String),
    /// The configured host is not a valid TLS server name.
    #[error("invalid IMAP server name `{0}`")]
    InvalidServerName(String),
    /// The IMAP `LOGIN` was rejected.
    #[error("IMAP login failed: {0}")]
    Login(String),
    /// The requested mailbox does not exist on the server (`NO` on `SELECT`).
    #[error("mailbox not found")]
    MailboxNotFound,
    /// The requested message does not exist in the selected mailbox.
    #[error("message not found")]
    MessageNotFound,
    /// The server does not announce the IMAP extension the operation needs.
    #[error("the server does not support this operation")]
    CapabilityNotSupported,
    /// The supplied flag update is not a valid allowlisted request.
    #[error("invalid flags")]
    InvalidFlags,
    /// The connection ended before a usable state was reached.
    #[error("IMAP server unavailable: {0}")]
    Unavailable(String),
    /// The whole connection attempt exceeded the configured timeout.
    #[error("IMAP operation timed out after {0:?}")]
    Timeout(Duration),
    /// A protocol-level error reported by `async-imap`.
    #[error(transparent)]
    Imap(#[from] async_imap::error::Error),
    /// A generic I/O error while talking to the server.
    #[error("IMAP I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl ImapError {
    /// Stable, external-facing message that never carries third-party details.
    ///
    /// The HTTP layer uses this instead of [`Display`](std::fmt::Display) so no
    /// error text coming from `async-imap`, rustls or the operating system (which
    /// may embed host names, certificates or addresses) ever reaches a client. It
    /// groups variants by failure family, keeping the response stable across
    /// versions of the underlying crates.
    pub fn public_message(&self) -> &'static str {
        match self {
            Self::Tcp(_) | Self::Timeout(_) | Self::Unavailable(_) => {
                "could not connect to the IMAP server"
            }
            Self::Tls(_) | Self::TlsConfig(_) | Self::InvalidServerName(_) => {
                "IMAP TLS handshake failed"
            }
            Self::Login(_) | Self::Imap(_) => "IMAP authentication or command failed",
            Self::MailboxNotFound => "mailbox not found",
            Self::MessageNotFound => "message not found",
            Self::CapabilityNotSupported => "the server does not support this operation",
            Self::InvalidFlags => "invalid flags",
            Self::Io(_) => "IMAP I/O error",
        }
    }

    /// Static label identifying the failure kind, for **logs only**.
    ///
    /// Unlike [`public_message`](Self::public_message) this is intended for
    /// tracing, where a compact, machine-filterable tag is more useful than the
    /// full [`Display`](std::fmt::Display) text.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Tcp(_) => "tcp",
            Self::Tls(_) | Self::TlsConfig(_) | Self::InvalidServerName(_) => "tls",
            Self::Login(_) => "login",
            Self::MailboxNotFound => "not_found",
            Self::MessageNotFound => "not_found",
            Self::CapabilityNotSupported => "unsupported",
            Self::InvalidFlags => "invalid_flags",
            Self::Unavailable(_) => "unavailable",
            Self::Timeout(_) => "timeout",
            Self::Imap(_) => "imap",
            Self::Io(_) => "io",
        }
    }

    /// Whether the failure indicates the cached session is no longer usable.
    ///
    /// Transport-level failures (the socket, TLS, a timeout, a dead server or an
    /// `async-imap` I/O/connection-lost error) mean the session must be dropped
    /// and rebuilt on the next request. Logical failures (rejected credentials,
    /// a missing mailbox, a `BAD`/`NO`/parse error) do **not** invalidate the
    /// session. The classification intentionally sits in one place so the
    /// `ConnectionManager` can act on it uniformly.
    pub fn is_connection(&self) -> bool {
        match self {
            Self::Tcp(_)
            | Self::Tls(_)
            | Self::TlsConfig(_)
            | Self::InvalidServerName(_)
            | Self::Timeout(_)
            | Self::Unavailable(_)
            | Self::Io(_) => true,
            Self::Login(_) | Self::MailboxNotFound | Self::MessageNotFound => false,
            Self::CapabilityNotSupported => false,
            Self::InvalidFlags => false,
            Self::Imap(inner) => matches!(
                inner,
                async_imap::error::Error::Io(_) | async_imap::error::Error::ConnectionLost
            ),
        }
    }
}

/// Maps a flag-validation failure to its own logical IMAP failure.
///
/// A [`FlagError`] means the caller composed an invalid update (for example an
/// empty one). It is a **logical** error, not a transport failure, so it maps to
/// [`ImapError::InvalidFlags`] — `is_connection()` stays `false` and a live
/// session is kept. The fixed, credential-free message carries no `FlagError`
/// text.
impl From<FlagError> for ImapError {
    fn from(_error: FlagError) -> Self {
        Self::InvalidFlags
    }
}

/// Errors produced while fetching and parsing a single message.
///
/// Every variant is stable and free of third-party error text: [`Imap`] keeps
/// the classification of [`ImapError`] so the HTTP layer can reuse the existing
/// mapping, [`TooLarge`] reports the size guard and [`Unparsable`] the parser's
/// refusal.
#[derive(Debug, thiserror::Error)]
pub enum ParsedMessageError {
    /// The IMAP layer failed.
    #[error(transparent)]
    Imap(#[from] ImapError),
    /// The message exceeded the configured size limit.
    #[error("message too large")]
    TooLarge {
        /// Reported or fetched message size, in bytes.
        ///
        /// `usize` (not `u32`) so the fetched length, which is a `usize`, is
        /// never truncated when it is reported back.
        size: usize,
        /// Configured maximum size, in bytes.
        limit: usize,
    },
    /// The message could not be parsed as MIME.
    #[error("message cannot be parsed")]
    Unparsable,
}

impl From<MimeError> for ParsedMessageError {
    fn from(_error: MimeError) -> Self {
        Self::Unparsable
    }
}

/// Classifies whether an operation error must discard the cached session.
///
/// [`ConnectionManager::run_once`] is expressed over this trait so the same
/// reconnect policy serves both the plain [`ImapError`] commands and
/// [`fetch_parsed`](ConnectionManager::fetch_parsed), which returns a
/// [`ParsedMessageError`].
trait ConnectionFailure {
    /// Whether the cached session should be dropped after this error.
    fn is_connection(&self) -> bool;
}

impl ConnectionFailure for ImapError {
    fn is_connection(&self) -> bool {
        ImapError::is_connection(self)
    }
}

impl ConnectionFailure for ParsedMessageError {
    fn is_connection(&self) -> bool {
        matches!(self, ParsedMessageError::Imap(error) if error.is_connection())
    }
}

/// Public description of a mailbox, as reported by `LIST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxInfo {
    /// Mailbox name (the raw, server-provided string).
    pub name: String,
    /// Hierarchy delimiter, or `None` for a flat namespace.
    pub delimiter: Option<String>,
    /// Mailbox attributes, rendered in IMAP style (`\NoSelect`, …).
    pub attributes: Vec<String>,
}

/// Mailbox status reported by `SELECT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxStatus {
    /// Number of messages in the mailbox (`EXISTS`).
    pub exists: u32,
    /// Number of messages with the `\Recent` flag (`RECENT`).
    pub recent: u32,
    /// Sequence number of the first unseen message (`UNSEEN`), if reported.
    pub unseen: Option<u32>,
    /// UID validity value (`UIDVALIDITY`), if reported.
    pub uid_validity: Option<u32>,
    /// Next unique identifier (`UIDNEXT`), if reported.
    pub uid_next: Option<u32>,
    /// Defined message flags, rendered in IMAP style (`\Seen`, …).
    pub flags: Vec<String>,
}

/// Errors produced while building a `SEARCH` query.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// The supplied date is not a valid `YYYY-MM-DD` calendar date.
    #[error("invalid date")]
    InvalidDate,
}

/// A calendar date used in `SINCE`/`BEFORE` search keys.
///
/// The fields are private so every instance is guaranteed to be a valid calendar
/// date: [`SearchDate::parse`] is the only constructor and validates the range of
/// each component (including leap years). [`SearchDate::to_imap`] renders the
/// canonical `DD-Mon-YYYY` form, which the server parses unambiguously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchDate {
    /// Day of the month (`1..=days_in_month`).
    day: u8,
    /// Month of the year (`1..=12`).
    month: u8,
    /// Four-digit year.
    year: u32,
}

impl SearchDate {
    /// Parses a strict `YYYY-MM-DD` date, validating the calendar.
    ///
    /// The format is exact (four-digit year, two-digit month and day separated by
    /// hyphens); the month must be `1..=12` and the day must be within the month,
    /// honouring leap years.
    pub fn parse(value: &str) -> Result<Self, QueryError> {
        let bytes = value.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return Err(QueryError::InvalidDate);
        }
        let digits = |slice: &[u8]| -> Result<u32, QueryError> {
            if slice.iter().all(u8::is_ascii_digit) {
                // The slice is at most four ASCII digits, so the value fits `u32`.
                Ok(slice
                    .iter()
                    .fold(0, |acc, digit| acc * 10 + u32::from(digit - b'0')))
            } else {
                Err(QueryError::InvalidDate)
            }
        };
        let year = digits(&bytes[0..4])?;
        let month = digits(&bytes[5..7])?;
        let day = digits(&bytes[8..10])?;

        if !(1..=12).contains(&month) {
            return Err(QueryError::InvalidDate);
        }
        if year == 0 {
            // IMAP dates carry a four-digit year; `0000` is not a usable date.
            return Err(QueryError::InvalidDate);
        }
        if day < 1 || day > days_in_month(month, year) {
            return Err(QueryError::InvalidDate);
        }

        // Every component above is in range, so the casts are lossless.
        Ok(Self {
            day: day as u8,
            month: month as u8,
            year,
        })
    }

    /// Renders the canonical IMAP `DD-Mon-YYYY` form (for example `05-Jan-2024`).
    pub fn to_imap(&self) -> String {
        let name = MONTH_NAMES[(self.month - 1) as usize];
        format!("{:02}-{name}-{:04}", self.day, self.year)
    }
}

/// English month abbreviations used by IMAP dates, indexed from 0.
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Number of days in `month` (1-based) of `year`, honouring leap years.
fn days_in_month(month: u32, year: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(year: u32) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// Renders `value` as an IMAP quoted string that cannot break the command line.
///
/// `CR`, `LF` and `NUL` are **dropped** (defence in depth against command
/// injection through the unvalidated `UID SEARCH`/`UID FETCH` strings), and the
/// backslash and double quote are escaped. The result is always wrapped in double
/// quotes, so it is a single IMAP `quoted` argument.
fn quote_search_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '\r' | '\n' | '\0' => {}
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Criteria translated into an IMAP `SEARCH` key.
///
/// Each `Some` field renders exactly one search key; an empty criteria renders
/// `ALL`. Textual values go through [`quote_search_string`] so no user input can
/// inject a second command, and dates render in canonical `DD-Mon-YYYY` form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchCriteria {
    /// Matches the `From` header.
    pub from: Option<String>,
    /// Matches the `To` header.
    pub to: Option<String>,
    /// Matches the `Subject` header.
    pub subject: Option<String>,
    /// Matches text anywhere in the message (`BODY`).
    pub text: Option<String>,
    /// Matches messages received on or after this date (`SINCE`).
    pub since: Option<SearchDate>,
    /// Matches messages received before this date (`BEFORE`).
    pub before: Option<SearchDate>,
    /// `Some(true)` selects `SEEN`, `Some(false)` selects `UNSEEN`.
    pub seen: Option<bool>,
    /// `Some(true)` selects `FLAGGED`, `Some(false)` selects `UNFLAGGED`.
    pub flagged: Option<bool>,
}

impl SearchCriteria {
    /// Renders the criteria as the argument of `UID SEARCH`, or `ALL` when empty.
    pub fn imap_key(&self) -> String {
        let mut keys: Vec<String> = Vec::new();
        if let Some(from) = &self.from {
            keys.push(format!("FROM {}", quote_search_string(from)));
        }
        if let Some(to) = &self.to {
            keys.push(format!("TO {}", quote_search_string(to)));
        }
        if let Some(subject) = &self.subject {
            keys.push(format!("SUBJECT {}", quote_search_string(subject)));
        }
        if let Some(text) = &self.text {
            keys.push(format!("BODY {}", quote_search_string(text)));
        }
        if let Some(since) = &self.since {
            keys.push(format!("SINCE {}", since.to_imap()));
        }
        if let Some(before) = &self.before {
            keys.push(format!("BEFORE {}", before.to_imap()));
        }
        match self.seen {
            Some(true) => keys.push("SEEN".to_string()),
            Some(false) => keys.push("UNSEEN".to_string()),
            None => {}
        }
        match self.flagged {
            Some(true) => keys.push("FLAGGED".to_string()),
            Some(false) => keys.push("UNFLAGGED".to_string()),
            None => {}
        }

        if keys.is_empty() {
            "ALL".to_string()
        } else {
            keys.join(" ")
        }
    }
}

/// How much of a message `UID FETCH` should return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchFormat {
    /// Metadata and envelope only.
    Summary,
    /// Metadata, envelope and the raw header block.
    Headers,
    /// Metadata, envelope and the raw full RFC822 source.
    Full,
}

impl FetchFormat {
    /// The exact `FETCH` data-items query for this format.
    ///
    /// Body sections are requested with `BODY.PEEK`, so fetching never sets the
    /// `\Seen` flag on the server.
    pub fn query(&self) -> &'static str {
        match self {
            Self::Summary => "(UID FLAGS RFC822.SIZE INTERNALDATE ENVELOPE)",
            Self::Headers => "(UID FLAGS RFC822.SIZE INTERNALDATE ENVELOPE BODY.PEEK[HEADER])",
            Self::Full => "(UID FLAGS RFC822.SIZE INTERNALDATE ENVELOPE BODY.PEEK[])",
        }
    }
}

/// An email address extracted from a message envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    /// Display name, if the server reported one.
    pub name: Option<String>,
    /// `mailbox@host`, or just the mailbox when no host was reported.
    pub address: Option<String>,
}

/// The subset of an envelope exposed by the API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageEnvelope {
    /// `From` addresses.
    pub from: Vec<Address>,
    /// `To` addresses.
    pub to: Vec<Address>,
    /// `Cc` addresses.
    pub cc: Vec<Address>,
    /// Raw `Subject` header, if reported.
    pub subject: Option<String>,
    /// Raw `Date` header, if reported.
    pub date: Option<String>,
    /// `Message-ID`, if reported.
    pub message_id: Option<String>,
}

/// A message as returned by `UID FETCH`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Unique identifier within the mailbox.
    pub uid: u32,
    /// Sequence number within the selected mailbox.
    pub seq: u32,
    /// Flags set on the message, in IMAP style (`\Seen`, …).
    pub flags: Vec<String>,
    /// `RFC822.SIZE`, if reported.
    pub size: Option<u32>,
    /// `INTERNALDATE` in RFC 3339 form, if reported.
    pub internal_date: Option<String>,
    /// Parsed envelope, if requested and reported.
    pub envelope: Option<MessageEnvelope>,
    /// Raw header block, only for [`FetchFormat::Headers`].
    pub headers: Option<Vec<u8>>,
    /// Raw full RFC822 source, only for [`FetchFormat::Full`].
    pub body: Option<Vec<u8>>,
}

/// One page of messages plus the pagination metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    /// Total number of messages matching the search.
    pub total: u32,
    /// Requested page size.
    pub limit: u32,
    /// Requested page offset.
    pub offset: u32,
    /// The messages in this page.
    pub messages: Vec<Message>,
}

/// Renders a message flag in its IMAP textual form.
///
/// `imap-proto` implements neither `Display` nor an accessor for the original
/// name, so the system flags are mapped explicitly and a user/server keyword is
/// returned verbatim (as IMAP keywords carry no leading backslash). `\*`
/// ([`Flag::MayCreate`]) is the only system flag that is not a named flag.
fn flag_label(flag: &Flag<'_>) -> String {
    match flag {
        Flag::Seen => "\\Seen".to_string(),
        Flag::Answered => "\\Answered".to_string(),
        Flag::Flagged => "\\Flagged".to_string(),
        Flag::Deleted => "\\Deleted".to_string(),
        Flag::Draft => "\\Draft".to_string(),
        Flag::Recent => "\\Recent".to_string(),
        Flag::MayCreate => "\\*".to_string(),
        Flag::Custom(name) => name.to_string(),
    }
}

/// A system flag from the closed allowlist the API accepts.
///
/// Only these five flags may ever reach a `UID STORE` query; the parser is
/// case-insensitive but the `\` prefix is mandatory, and [`to_imap`] renders the
/// canonical capitalised IMAP spelling.
///
/// [`to_imap`]: SystemFlag::to_imap
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemFlag {
    /// `\Seen`.
    Seen,
    /// `\Answered`.
    Answered,
    /// `\Flagged`.
    Flagged,
    /// `\Draft`.
    Draft,
    /// `\Deleted`.
    Deleted,
}

impl SystemFlag {
    /// Parses a standard flag name, case-insensitively and requiring a leading
    /// backslash.
    pub fn parse(value: &str) -> Result<Self, FlagError> {
        let Some(name) = value.strip_prefix('\\') else {
            return Err(FlagError::Unknown);
        };
        if name.eq_ignore_ascii_case("seen") {
            Ok(Self::Seen)
        } else if name.eq_ignore_ascii_case("answered") {
            Ok(Self::Answered)
        } else if name.eq_ignore_ascii_case("flagged") {
            Ok(Self::Flagged)
        } else if name.eq_ignore_ascii_case("draft") {
            Ok(Self::Draft)
        } else if name.eq_ignore_ascii_case("deleted") {
            Ok(Self::Deleted)
        } else {
            Err(FlagError::Unknown)
        }
    }

    /// The canonical IMAP spelling (for example `\Seen`).
    pub fn to_imap(&self) -> &'static str {
        match self {
            Self::Seen => "\\Seen",
            Self::Answered => "\\Answered",
            Self::Flagged => "\\Flagged",
            Self::Draft => "\\Draft",
            Self::Deleted => "\\Deleted",
        }
    }
}

/// Errors produced while parsing or composing a flag update.
#[derive(Debug, thiserror::Error)]
pub enum FlagError {
    /// The supplied value is not a known system flag.
    #[error("unknown flag")]
    Unknown,
    /// Both the additions and the removals were empty.
    #[error("empty flag update")]
    Empty,
}

/// A validated `STORE` query composed **only** from the [`SystemFlag`] allowlist
/// and the fixed `.SILENT` items.
///
/// [`new`](FlagQuery::new) is the only constructor, so arbitrary user text can
/// never reach the interpolated `UID STORE` query. The rendered form is, for
/// example, `+FLAGS.SILENT (\Seen \Flagged) -FLAGS.SILENT (\Deleted)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagQuery(String);

impl FlagQuery {
    /// Builds a `STORE` query from the flags to add and remove.
    ///
    /// At least one of `add`/`remove` must be non-empty; the flags are rendered
    /// in the given order, with the fixed `+FLAGS.SILENT`/`-FLAGS.SILENT` items.
    pub fn new(add: &[SystemFlag], remove: &[SystemFlag]) -> Result<Self, FlagError> {
        if add.is_empty() && remove.is_empty() {
            return Err(FlagError::Empty);
        }
        let mut parts: Vec<String> = Vec::new();
        if !add.is_empty() {
            parts.push(format!("+FLAGS.SILENT ({})", render_flags(add)));
        }
        if !remove.is_empty() {
            parts.push(format!("-FLAGS.SILENT ({})", render_flags(remove)));
        }
        Ok(Self(parts.join(" ")))
    }

    /// The validated query string, ready to interpolate into `UID STORE`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Renders a space-separated list of canonical IMAP flag names.
fn render_flags(flags: &[SystemFlag]) -> String {
    flags
        .iter()
        .map(SystemFlag::to_imap)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The IMAP extensions the flag commands depend on, as announced by `CAPABILITY`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// The server announces the `MOVE` extension (RFC 6851).
    pub has_move: bool,
    /// The server announces the `UIDPLUS` extension (RFC 4315).
    pub has_uidplus: bool,
}

/// Decodes raw envelope bytes with a lossy UTF-8 conversion.
///
/// The server's envelope fields are opaque byte strings; a lossy conversion keeps
/// the message readable without failing on malformed input.
fn decode_bytes(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Joins a raw mailbox and host into `mailbox@host`.
///
/// An address without a mailbox is not meaningful and maps to `None`.
fn mailbox_at_host(mailbox: Option<&[u8]>, host: Option<&[u8]>) -> Option<String> {
    let mailbox = mailbox?;
    let mailbox = String::from_utf8_lossy(mailbox);
    match host {
        Some(host) => Some(format!("{mailbox}@{}", String::from_utf8_lossy(host))),
        None => Some(mailbox.into_owned()),
    }
}

/// Maps a raw `imap-proto` address to the API [`Address`].
fn address_to_dto(address: &async_imap::imap_proto::Address<'_>) -> Address {
    Address {
        name: address.name.as_deref().map(decode_bytes),
        address: mailbox_at_host(address.mailbox.as_deref(), address.host.as_deref()),
    }
}

/// Maps a list of raw addresses, defaulting a missing list to empty.
fn addresses_to_dto(addresses: Option<&[async_imap::imap_proto::Address<'_>]>) -> Vec<Address> {
    addresses
        .unwrap_or_default()
        .iter()
        .map(address_to_dto)
        .collect()
}

/// Maps a raw `imap-proto` envelope to the API [`MessageEnvelope`].
fn envelope_to_dto(envelope: &async_imap::imap_proto::Envelope<'_>) -> MessageEnvelope {
    MessageEnvelope {
        from: addresses_to_dto(envelope.from.as_deref()),
        to: addresses_to_dto(envelope.to.as_deref()),
        cc: addresses_to_dto(envelope.cc.as_deref()),
        subject: envelope.subject.as_deref().map(decode_bytes),
        date: envelope.date.as_deref().map(decode_bytes),
        message_id: envelope.message_id.as_deref().map(decode_bytes),
    }
}

/// Maps a raw `async-imap` [`Fetch`] to an API [`Message`].
///
/// Returns `None` for a response without a UID (which cannot be addressed by the
/// API), so the caller can skip it.
fn fetch_to_message(fetch: &Fetch, format: FetchFormat) -> Option<Message> {
    let uid = fetch.uid?;
    let mut message = Message {
        uid,
        seq: fetch.message,
        flags: fetch.flags().map(|flag| flag_label(&flag)).collect(),
        size: fetch.size,
        internal_date: fetch.internal_date().map(|date| date.to_rfc3339()),
        envelope: fetch.envelope().map(envelope_to_dto),
        headers: None,
        body: None,
    };
    match format {
        FetchFormat::Summary => {}
        FetchFormat::Headers => message.headers = fetch.header().map(<[u8]>::to_vec),
        FetchFormat::Full => message.body = fetch.body().map(<[u8]>::to_vec),
    }
    Some(message)
}

/// Ensures `name` carries exactly one leading backslash.
///
/// `imap-proto` reports the known attributes and the extension names **with**
/// the leading backslash already included, but a future variant (or a different
/// `Debug` rendering) could expose it without one. Stripping any existing
/// prefix before re-adding a single one keeps the output stable in every case
/// and never produces a doubled `\\`.
fn with_attribute_prefix(name: &str) -> String {
    format!("\\{}", name.trim_start_matches('\\'))
}

/// Renders a `LIST` mailbox attribute in its IMAP textual form.
///
/// `imap-proto` does not implement `Display`; each known variant maps to the
/// canonical `\Name` spelling. Attributes from extensions the crate does not
/// model arrive as [`NameAttribute::Extension`] **already carrying** the leading
/// backslash (for example `\HasNoChildren`), so it is normalised here rather
/// than prefixed again. The enum is `#[non_exhaustive]`, so a defensive arm
/// keeps the mapping total if a future release adds a variant; it too is
/// normalised so it can never emit a doubled backslash.
fn attribute_label(attr: &NameAttribute<'_>) -> String {
    match attr {
        NameAttribute::NoInferiors => "\\NoInferiors".to_string(),
        NameAttribute::NoSelect => "\\NoSelect".to_string(),
        NameAttribute::Marked => "\\Marked".to_string(),
        NameAttribute::Unmarked => "\\Unmarked".to_string(),
        NameAttribute::All => "\\All".to_string(),
        NameAttribute::Archive => "\\Archive".to_string(),
        NameAttribute::Drafts => "\\Drafts".to_string(),
        NameAttribute::Flagged => "\\Flagged".to_string(),
        NameAttribute::Junk => "\\Junk".to_string(),
        NameAttribute::Sent => "\\Sent".to_string(),
        NameAttribute::Trash => "\\Trash".to_string(),
        NameAttribute::Extension(name) => with_attribute_prefix(name),
        other => with_attribute_prefix(&format!("{other:?}")),
    }
}

/// A live, authenticated IMAP session able to run commands.
///
/// The trait exposes the commands the mailbox layer needs: a `NOOP` liveness
/// probe, `LIST` (as [`list_mailboxes`](ImapSession::list_mailboxes)) and
/// `SELECT` (as [`select`](ImapSession::select)). It stays object-safe without
/// `async-trait` by returning boxed futures.
pub trait ImapSession: Send {
    /// Sends a `NOOP`, used to check that the session is still alive.
    fn noop(&mut self) -> SendFuture<'_, Result<(), ImapError>>;

    /// Lists the account's mailboxes with `LIST`.
    fn list_mailboxes(&mut self) -> SendFuture<'_, Result<Vec<MailboxInfo>, ImapError>>;

    /// Selects `mailbox` with `SELECT`, returning its status.
    ///
    /// An unknown mailbox is reported as [`ImapError::MailboxNotFound`].
    fn select(&mut self, mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>>;

    /// Runs `UID SEARCH` with `criteria`, returning matching UIDs newest-first.
    ///
    /// `async-imap` returns a `HashSet`, so the implementation sorts the UIDs in
    /// descending order before returning them.
    fn search(&mut self, criteria: SearchCriteria) -> SendFuture<'_, Result<Vec<u32>, ImapError>>;

    /// Runs `UID FETCH` over `uids` in the requested `format`.
    ///
    /// An empty `uids` list returns an empty result **without sending a command**
    /// (an empty UID set is not a valid IMAP command).
    fn fetch(
        &mut self,
        uids: Vec<u32>,
        format: FetchFormat,
    ) -> SendFuture<'_, Result<Vec<Message>, ImapError>>;

    /// Applies a validated flag `query` to the single message `uid` with
    /// `UID STORE`, draining the response stream so the command completes.
    fn store(&mut self, uid: u32, query: FlagQuery) -> SendFuture<'_, Result<(), ImapError>>;

    /// Copies the single message `uid` to `mailbox` with `UID COPY`.
    fn copy(&mut self, uid: u32, mailbox: String) -> SendFuture<'_, Result<(), ImapError>>;

    /// Moves the single message `uid` to `mailbox` with `UID MOVE`.
    fn move_message(&mut self, uid: u32, mailbox: String) -> SendFuture<'_, Result<(), ImapError>>;

    /// Purges only the `\Deleted` message `uid` with `UID EXPUNGE`, draining the
    /// response stream so the command completes.
    fn uid_expunge(&mut self, uid: u32) -> SendFuture<'_, Result<(), ImapError>>;

    /// Queries the server capabilities with `CAPABILITY`, mapping the extensions
    /// the flag commands depend on.
    fn capabilities(&mut self) -> SendFuture<'_, Result<Capabilities, ImapError>>;
}

/// A factory able to open a fresh authenticated IMAP session.
pub trait ImapConnector: Send + Sync {
    /// Establishes a new session, connecting and authenticating.
    fn connect(&self) -> SendFuture<'_, Result<Box<dyn ImapSession>, ImapError>>;
}

/// A network stream that is either a plain TCP connection or a TLS one.
///
/// `async-imap` (with the `runtime-tokio` feature) requires the stream to
/// implement `tokio::io::{AsyncRead, AsyncWrite} + Unpin + Debug + Send`. This
/// enum adapts the two shapes used by the three TLS modes. `AsyncRead` and
/// `AsyncWrite` are forwarded to the inner, `Unpin` stream; `Debug` is manual so
/// the TLS internals are not printed.
pub enum ImapStream {
    /// Cleartext TCP stream (`starttls` before the upgrade and `none`).
    Plain(TcpStream),
    /// TLS-encrypted stream (`implicit` or after a `starttls` upgrade).
    Tls(Box<TlsStream<TcpStream>>),
}

impl fmt::Debug for ImapStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain(_) => f.write_str("ImapStream::Plain"),
            Self::Tls(_) => f.write_str("ImapStream::Tls"),
        }
    }
}

impl AsyncRead for ImapStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ImapStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_write(cx, buf),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Self::Tls(stream) => Pin::new(stream.as_mut()).poll_shutdown(cx),
        }
    }
}

/// The TLS strategy selected for an IMAP endpoint.
///
/// Kept as a small pure value so the mapping from [`TlsMode`] can be unit-tested
/// without opening a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TlsStrategy {
    /// Implicit TLS from the first byte.
    Implicit,
    /// Cleartext connection upgraded in-band with `STARTTLS`.
    StartTls,
    /// No TLS at all.
    Plain,
}

/// Maps a configured [`TlsMode`] to the connection strategy to use.
fn tls_strategy(mode: TlsMode) -> TlsStrategy {
    match mode {
        TlsMode::Implicit => TlsStrategy::Implicit,
        TlsMode::StartTls => TlsStrategy::StartTls,
        TlsMode::Plain => TlsStrategy::Plain,
    }
}

/// The real [`ImapConnector`], backed by `async-imap` over TCP + rustls.
///
/// Built once from the [`Config`] without opening any connection. The rustls
/// client configuration uses the **ring** provider and the Mozilla root store
/// from `webpki-roots`, explicitly pinning the crypto backend instead of relying
/// on the process default.
pub struct TokioImapConnector {
    /// IMAP endpoint (host, port, TLS mode and credentials).
    endpoint: MailEndpoint,
    /// Timeout applied to the whole connection attempt.
    timeout: Duration,
    /// Pre-built rustls client configuration.
    tls_config: Arc<ClientConfig>,
}

impl TokioImapConnector {
    /// Builds the connector from the loaded [`Config`] **without opening any
    /// connection**.
    pub fn from_config(config: &Config) -> Result<Self, ImapError> {
        Ok(Self {
            endpoint: config.account.imap.clone(),
            timeout: config.imap_timeout,
            tls_config: build_tls_config()?,
        })
    }

    /// Runs the whole connection and authentication sequence under a single
    /// timeout.
    async fn connect_inner(&self) -> Result<Box<dyn ImapSession>, ImapError> {
        let session = connect_and_login(&self.endpoint, &self.tls_config).await?;
        Ok(Box::new(SessionHandle { session }))
    }
}

/// Builds the rustls client configuration shared by the shared session and the
/// dedicated IDLE connection.
///
/// Explicitly pins the **ring** crypto provider and the Mozilla root store from
/// `webpki-roots` instead of relying on the process default.
pub(crate) fn build_tls_config() -> Result<Arc<ClientConfig>, ImapError> {
    let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let tls_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|error| ImapError::TlsConfig(error.to_string()))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(tls_config))
}

/// Performs the TLS handshake over an already-connected TCP stream.
pub(crate) async fn upgrade(
    tcp: TcpStream,
    host: &str,
    tls_config: &Arc<ClientConfig>,
) -> Result<TlsStream<TcpStream>, ImapError> {
    let server_name = ServerName::try_from(host)
        .map_err(|_| ImapError::InvalidServerName(host.to_string()))?
        .to_owned();
    let connector = TlsConnector::from(Arc::clone(tls_config));
    connector
        .connect(server_name, tcp)
        .await
        .map_err(|error| ImapError::Tls(error.to_string()))
}

/// Connects, negotiates TLS according to the endpoint's [`TlsMode`] and runs
/// `LOGIN`, returning the **raw** authenticated session.
///
/// Shared by [`TokioImapConnector`] (which wraps it in a
/// [`SessionHandle`]) and the dedicated IDLE connection, so both go through the
/// exact same connection and authentication routine.
pub(crate) async fn connect_and_login(
    endpoint: &MailEndpoint,
    tls_config: &Arc<ClientConfig>,
) -> Result<async_imap::Session<ImapStream>, ImapError> {
    let strategy = tls_strategy(endpoint.tls);
    let tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .await
        .map_err(ImapError::Tcp)?;

    let stream = match strategy {
        TlsStrategy::Implicit => {
            let tls = upgrade(tcp, &endpoint.host, tls_config).await?;
            ImapStream::Tls(Box::new(tls))
        }
        TlsStrategy::Plain => ImapStream::Plain(tcp),
        TlsStrategy::StartTls => {
            let mut client = Client::new(ImapStream::Plain(tcp));
            read_greeting(&mut client).await?;
            client
                .run_command_and_check_ok("STARTTLS", None)
                .await
                .map_err(ImapError::Imap)?;
            let tcp = match client.into_inner() {
                ImapStream::Plain(tcp) => tcp,
                ImapStream::Tls(_) => {
                    return Err(ImapError::Tls(
                        "STARTTLS did not return a cleartext stream".to_string(),
                    ));
                }
            };
            let tls = upgrade(tcp, &endpoint.host, tls_config).await?;
            ImapStream::Tls(Box::new(tls))
        }
    };

    let mut client = Client::new(stream);
    // STARTTLS already consumed the greeting; the other modes must read it
    // before authenticating.
    if !matches!(strategy, TlsStrategy::StartTls) {
        read_greeting(&mut client).await?;
    }

    let session = client
        .login(endpoint.username.as_str(), endpoint.password.as_str())
        .await
        .map_err(|(error, _client)| ImapError::Login(error.to_string()))?;
    Ok(session)
}

impl ImapConnector for TokioImapConnector {
    fn connect(&self) -> SendFuture<'_, Result<Box<dyn ImapSession>, ImapError>> {
        Box::pin(async move {
            tokio::time::timeout(self.timeout, self.connect_inner())
                .await
                .map_err(|_| ImapError::Timeout(self.timeout))?
        })
    }
}

/// Reads and discards the server greeting that precedes authentication.
async fn read_greeting(client: &mut Client<ImapStream>) -> Result<(), ImapError> {
    match client.read_response().await.map_err(ImapError::Io)? {
        Some(_) => Ok(()),
        None => Err(ImapError::Unavailable(
            "connection closed before the IMAP greeting".to_string(),
        )),
    }
}

/// Maps an `async-imap` [`Mailbox`] to the API [`MailboxStatus`].
///
/// Shared by the shared session and the dedicated IDLE connection so both report
/// the mailbox status identically.
pub(crate) fn mailbox_status(mailbox: &Mailbox) -> MailboxStatus {
    MailboxStatus {
        exists: mailbox.exists,
        recent: mailbox.recent,
        unseen: mailbox.unseen,
        uid_validity: mailbox.uid_validity,
        uid_next: mailbox.uid_next,
        flags: mailbox.flags.iter().map(flag_label).collect(),
    }
}

/// Runs `UID FETCH` with the raw `uid_set` and maps every response to a
/// [`Message`].
///
/// The caller is responsible for supplying a valid UID set (for example `"42"`,
/// `"7,9"` or `"10:*"`); an empty set is not a valid IMAP command.
pub(crate) async fn fetch_messages(
    session: &mut async_imap::Session<ImapStream>,
    uid_set: String,
    format: FetchFormat,
) -> Result<Vec<Message>, ImapError> {
    let stream = session
        .uid_fetch(uid_set, format.query())
        .await
        .map_err(ImapError::from)?;
    let fetches: Vec<Fetch> = stream.try_collect().await?;
    Ok(fetches
        .iter()
        .filter_map(|fetch| fetch_to_message(fetch, format))
        .collect())
}

/// A real [`ImapSession`] wrapping an `async-imap` [`Session`](async_imap::Session).
struct SessionHandle {
    /// Authenticated session over the [`ImapStream`].
    session: async_imap::Session<ImapStream>,
}

impl ImapSession for SessionHandle {
    fn noop(&mut self) -> SendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move { self.session.noop().await.map_err(ImapError::from) })
    }

    fn list_mailboxes(&mut self) -> SendFuture<'_, Result<Vec<MailboxInfo>, ImapError>> {
        Box::pin(async move {
            // `None` as the pattern would send `LIST "" ""`, which answers with
            // the hierarchy delimiter only and does **not** enumerate the
            // mailboxes. `Some("*")` sends `LIST "" "*"` (the RFC 3501 wildcard
            // matching every name) so the whole mailbox list is returned.
            let names = self
                .session
                .list(None, Some("*"))
                .await
                .map_err(ImapError::from)?;
            let mailboxes = names
                .map_ok(|name| MailboxInfo {
                    name: name.name().to_string(),
                    delimiter: name.delimiter().map(str::to_string),
                    attributes: name.attributes().iter().map(attribute_label).collect(),
                })
                .try_collect()
                .await?;
            Ok(mailboxes)
        })
    }

    fn select(&mut self, mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>> {
        Box::pin(async move {
            let mailbox = self
                .session
                .select(mailbox)
                .await
                .map_err(|error| match error {
                    async_imap::error::Error::No(_) => ImapError::MailboxNotFound,
                    other => ImapError::Imap(other),
                })?;
            Ok(mailbox_status(&mailbox))
        })
    }

    fn search(&mut self, criteria: SearchCriteria) -> SendFuture<'_, Result<Vec<u32>, ImapError>> {
        Box::pin(async move {
            let mut uids: Vec<u32> = self
                .session
                .uid_search(criteria.imap_key())
                .await
                .map_err(ImapError::from)?
                .into_iter()
                .collect();
            // `uid_search` returns a `HashSet`, so impose the newest-first order
            // the API promises.
            uids.sort_unstable_by(|a, b| b.cmp(a));
            Ok(uids)
        })
    }

    fn fetch(
        &mut self,
        uids: Vec<u32>,
        format: FetchFormat,
    ) -> SendFuture<'_, Result<Vec<Message>, ImapError>> {
        Box::pin(async move {
            if uids.is_empty() {
                // An empty UID set is not a valid IMAP command; skip it entirely.
                return Ok(Vec::new());
            }
            let set = uids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");
            fetch_messages(&mut self.session, set, format).await
        })
    }

    fn store(&mut self, uid: u32, query: FlagQuery) -> SendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move {
            let stream = self
                .session
                .uid_store(uid.to_string(), query.as_str())
                .await
                .map_err(ImapError::from)?;
            // `.SILENT` usually yields no FETCH, but the stream must be driven to
            // completion for the command to finish.
            let _: Vec<Fetch> = stream.try_collect().await?;
            Ok(())
        })
    }

    fn copy(&mut self, uid: u32, mailbox: String) -> SendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move {
            self.session
                .uid_copy(uid.to_string(), mailbox)
                .await
                .map_err(ImapError::from)
        })
    }

    fn move_message(&mut self, uid: u32, mailbox: String) -> SendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move {
            self.session
                .uid_mv(uid.to_string(), mailbox)
                .await
                .map_err(ImapError::from)
        })
    }

    fn uid_expunge(&mut self, uid: u32) -> SendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move {
            let stream = self
                .session
                .uid_expunge(uid.to_string())
                .await
                .map_err(ImapError::from)?;
            // The stream carries the expunged UIDs and must be drained.
            let _: Vec<u32> = stream.try_collect().await?;
            Ok(())
        })
    }

    fn capabilities(&mut self) -> SendFuture<'_, Result<Capabilities, ImapError>> {
        Box::pin(async move {
            let capabilities = self.session.capabilities().await.map_err(ImapError::from)?;
            Ok(Capabilities {
                has_move: capabilities.has_str("MOVE"),
                has_uidplus: capabilities.has_str("UIDPLUS"),
            })
        })
    }
}

/// Retry policy with bounded exponential backoff.
///
/// The defaults target production (3 attempts, 250 ms base, factor 2, 4 s cap).
/// Tests inject `base = 0` so retries are instantaneous and deterministic.
#[derive(Debug, Clone, Copy)]
pub struct Backoff {
    /// Maximum number of connection attempts.
    pub attempts: u32,
    /// Delay before the first retry.
    pub base: Duration,
    /// Multiplier applied to the delay on each retry.
    pub factor: u32,
    /// Upper bound for the delay.
    pub max: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            attempts: 3,
            base: Duration::from_millis(250),
            factor: 2,
            max: Duration::from_secs(4),
        }
    }
}

impl Backoff {
    /// Delay before the retry that follows attempt number `attempt` (1-based).
    ///
    /// The delay grows as `base * factor^(attempt - 1)`, saturating at `max`. A
    /// `base` of zero yields a zero delay, which callers may skip sleeping for.
    pub fn delay(&self, attempt: u32) -> Duration {
        let max_millis = self.max.as_millis();
        let mut millis = self.base.as_millis();
        for _ in 1..attempt {
            millis = millis.saturating_mul(u128::from(self.factor));
            if millis >= max_millis {
                return self.max;
            }
        }
        Duration::from_millis(millis.min(max_millis) as u64)
    }
}

/// Owns the cached IMAP session and (re)establishes it on demand.
///
/// `status()` is the single entry point used by the HTTP layer: it verifies the
/// cached session with `NOOP`, discards a dead one, and reconnects with bounded
/// exponential backoff. The session is guarded by an async mutex so concurrent
/// calls cannot open more than one connection.
pub struct ConnectionManager {
    /// Factory used to open new sessions.
    connector: Arc<dyn ImapConnector>,
    /// Cached live session, if any.
    session: tokio::sync::Mutex<Option<Box<dyn ImapSession>>>,
    /// Retry policy applied when (re)connecting.
    backoff: Backoff,
}

impl fmt::Debug for ConnectionManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Neither the connector nor the session is printed: both hold
        // credentials.
        f.debug_struct("ConnectionManager")
            .field("connector", &"***")
            .field("session", &"***")
            .field("backoff", &self.backoff)
            .finish()
    }
}

impl ConnectionManager {
    /// Creates a manager over `connector` using `backoff` for retries.
    ///
    /// `attempts` is normalized to at least one, so the manager always makes at
    /// least a single connection attempt regardless of a zeroed policy.
    pub fn new(connector: Arc<dyn ImapConnector>, backoff: Backoff) -> Self {
        let backoff = Backoff {
            attempts: backoff.attempts.max(1),
            ..backoff
        };
        Self {
            connector,
            session: tokio::sync::Mutex::new(None),
            backoff,
        }
    }

    /// Ensures the cached slot holds a live session, (re)connecting as needed.
    ///
    /// If a session is cached, it is probed with `NOOP`; a failure discards it.
    /// Then a fresh session is attempted up to `backoff.attempts` times, sleeping
    /// the backoff delay between attempts. Returns `Ok(())` once a live session is
    /// cached, or the last error after exhausting the attempts.
    async fn ensure_session(
        &self,
        guard: &mut Option<Box<dyn ImapSession>>,
    ) -> Result<(), ImapError> {
        if let Some(session) = guard.as_mut() {
            match session.noop().await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    tracing::warn!(kind = error.kind(), "discarding dead IMAP session");
                    *guard = None;
                }
            }
        }

        let mut last_error = None;
        for attempt in 1..=self.backoff.attempts {
            match self.connector.connect().await {
                Ok(session) => {
                    *guard = Some(session);
                    return Ok(());
                }
                Err(error) => {
                    last_error = Some(error);
                    if attempt < self.backoff.attempts {
                        let delay = self.backoff.delay(attempt);
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            ImapError::Unavailable("no connection attempt was configured".to_string())
        }))
    }

    /// Ensures a live session, connecting or reconnecting as needed.
    ///
    /// Delegates to [`ensure_session`](Self::ensure_session); this is the entry
    /// point used by the IMAP status endpoint.
    pub async fn status(&self) -> Result<(), ImapError> {
        let mut guard = self.session.lock().await;
        self.ensure_session(&mut guard).await
    }

    /// Runs `command` exactly once on a live session.
    ///
    /// The shared policy for the mailbox commands: lock the session, guarantee it
    /// with [`ensure_session`](Self::ensure_session), run `command` **once** (no
    /// in-place retry, since future commands may not be idempotent), and discard
    /// the cached session only when the failure is a connection error so the next
    /// request reconnects. The `for<'a>` bound ties the future returned by
    /// `command` to the session borrow, which is what lets the guard be reused
    /// after the await without cloning the session.
    async fn run_once<T, E, F>(&self, command: F) -> Result<T, E>
    where
        E: From<ImapError> + ConnectionFailure,
        F: for<'a> FnOnce(&'a mut Box<dyn ImapSession>) -> SendFuture<'a, Result<T, E>>,
    {
        let mut guard = self.session.lock().await;
        self.ensure_session(&mut guard).await.map_err(E::from)?;

        let outcome = match guard.as_mut() {
            Some(session) => command(session).await,
            None => Err(E::from(ImapError::Unavailable(
                "no IMAP session available".to_string(),
            ))),
        };

        if let Err(error) = &outcome
            && error.is_connection()
        {
            *guard = None;
        }
        outcome
    }

    /// Lists the account's mailboxes, reusing the live session when possible.
    ///
    /// Delegates to [`run_once`](Self::run_once): the command runs once on a
    /// session guaranteed by [`ensure_session`](Self::ensure_session), and a
    /// connection failure discards the cached session for the next request.
    pub async fn list_mailboxes(&self) -> Result<Vec<MailboxInfo>, ImapError> {
        self.run_once(|session| session.list_mailboxes()).await
    }

    /// Selects `mailbox` on the live session, returning its status.
    ///
    /// Follows the same single-attempt policy as
    /// [`list_mailboxes`](Self::list_mailboxes): a connection failure discards
    /// the cached session, while a logical failure (for example, an unknown
    /// mailbox) keeps it.
    pub async fn select_mailbox(&self, mailbox: &str) -> Result<MailboxStatus, ImapError> {
        let mailbox = mailbox.to_string();
        self.run_once(move |session| session.select(mailbox)).await
    }

    /// Lists the messages of `mailbox` matching `criteria`, paginated.
    ///
    /// Composes the whole operation on a single live session (one lock): selects
    /// the mailbox, runs the search, reports the full match count in `total`,
    /// trims the requested window and fetches its summary metadata. An empty page
    /// never sends a `UID FETCH`.
    pub async fn list_messages(
        &self,
        mailbox: &str,
        criteria: SearchCriteria,
        limit: u32,
        offset: u32,
    ) -> Result<MessagePage, ImapError> {
        let mailbox = mailbox.to_string();
        self.run_once(move |session| {
            Box::pin(async move {
                session.select(mailbox).await?;
                let uids = session.search(criteria).await?;
                let total = uids.len() as u32;
                let page_uids: Vec<u32> = uids
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect();
                let mut messages = session.fetch(page_uids, FetchFormat::Summary).await?;
                // The protocol allows the FETCH responses in any order, so impose
                // the newest-first order the API promises instead of trusting the
                // server's response order.
                messages.sort_unstable_by_key(|message| std::cmp::Reverse(message.uid));
                Ok(MessagePage {
                    total,
                    limit,
                    offset,
                    messages,
                })
            })
        })
        .await
    }

    /// Fetches a single message by `uid` in the requested `format`.
    ///
    /// Selects the mailbox and runs a `UID FETCH` for the one UID; a fetch that
    /// yields no message is reported as [`ImapError::MessageNotFound`].
    pub async fn fetch_message(
        &self,
        mailbox: &str,
        uid: u32,
        format: FetchFormat,
    ) -> Result<Message, ImapError> {
        let mailbox = mailbox.to_string();
        self.run_once(move |session| {
            Box::pin(async move {
                session.select(mailbox).await?;
                let mut messages = session.fetch(vec![uid], format).await?;
                messages.pop().ok_or(ImapError::MessageNotFound)
            })
        })
        .await
    }

    /// Fetches the message `uid` and parses its MIME content.
    ///
    /// The network work is confined to a single lock on the live session:
    /// `SELECT` → summary `UID FETCH` (existence plus `RFC822.SIZE`) → size
    /// guard → full `UID FETCH`, returning **only the raw bytes**. The
    /// CPU-bound [`parse_message`](crate::mime::parse_message) runs **after**
    /// the guard is released, so it neither occupies an async runtime worker
    /// for a potentially large parse nor keeps the IMAP session locked (which
    /// would block every other request that needs it meanwhile).
    ///
    /// # Memory bound
    ///
    /// The reported-size guard is only as good as the server's `RFC822.SIZE`:
    /// if the server omits it or understates it, `async-imap` materialises the
    /// whole `BODY.PEEK[]` response **before** the fetched length can be
    /// checked. The limit therefore does **not** strictly cap the memory the
    /// fetch itself allocates; the second check on `body.len()`, run outside
    /// the lock and before any parsing, is the real defence.
    pub async fn fetch_parsed(
        &self,
        mailbox: &str,
        uid: u32,
        max_bytes: usize,
    ) -> Result<ParsedMessage, ParsedMessageError> {
        let mailbox = mailbox.to_string();
        let body = self
            .run_once(move |session| {
                Box::pin(async move {
                    session.select(mailbox).await?;

                    let mut summaries = session.fetch(vec![uid], FetchFormat::Summary).await?;
                    let summary = summaries.pop().ok_or(ImapError::MessageNotFound)?;
                    if let Some(size) = summary.size
                        && size as usize > max_bytes
                    {
                        return Err(ParsedMessageError::TooLarge {
                            size: size as usize,
                            limit: max_bytes,
                        });
                    }

                    let mut messages = session.fetch(vec![uid], FetchFormat::Full).await?;
                    messages
                        .pop()
                        .and_then(|message| message.body)
                        .ok_or(ImapError::MessageNotFound)
                        .map_err(ParsedMessageError::from)
                })
            })
            .await?;

        // Second, authoritative guard: `RFC822.SIZE` may be missing or
        // understated, so reject the actually fetched length too. This runs
        // outside the session lock.
        if body.len() > max_bytes {
            return Err(ParsedMessageError::TooLarge {
                size: body.len(),
                limit: max_bytes,
            });
        }

        // Parsing is CPU-bound and walks a large, untrusted buffer. Hand it to a
        // blocking worker: it must not stall a runtime worker, and the IMAP lock
        // is already released, so other requests can reuse the session now. A
        // panic in the worker (`JoinError`) is reported as `Unparsable` rather
        // than propagated.
        tokio::task::spawn_blocking(move || crate::mime::parse_message(&body))
            .await
            .map_err(|_| ParsedMessageError::Unparsable)?
            .map_err(ParsedMessageError::from)
    }

    /// Applies `add`/`remove` flags to the single message `uid` and returns the
    /// resulting flags.
    ///
    /// Composes `SELECT`, an existence check, the allowlisted `UID STORE` and a
    /// summary `UID FETCH` on a single live session. A `uid` that is absent from
    /// the mailbox is reported as [`ImapError::MessageNotFound`] **before** any
    /// `STORE` is sent (consistently with the other mutation operations).
    pub async fn update_flags(
        &self,
        mailbox: &str,
        uid: u32,
        add: Vec<SystemFlag>,
        remove: Vec<SystemFlag>,
    ) -> Result<Vec<String>, ImapError> {
        let mailbox = mailbox.to_string();
        self.run_once(move |session| {
            Box::pin(async move {
                let query = FlagQuery::new(&add, &remove)?;
                session.select(mailbox).await?;
                ensure_message_exists(session, uid).await?;
                session.store(uid, query).await?;
                let mut messages = session.fetch(vec![uid], FetchFormat::Summary).await?;
                messages
                    .pop()
                    .map(|message| message.flags)
                    .ok_or(ImapError::MessageNotFound)
            })
        })
        .await
    }

    /// Copies the single message `uid` from `mailbox` to `to`.
    ///
    /// `UID COPY` is part of IMAP4rev1, so no capability is required. The message
    /// is checked for existence before the copy; a missing one is reported as
    /// [`ImapError::MessageNotFound`].
    pub async fn copy_message(&self, mailbox: &str, uid: u32, to: &str) -> Result<(), ImapError> {
        let mailbox = mailbox.to_string();
        let to = to.to_string();
        self.run_once(move |session| {
            Box::pin(async move {
                session.select(mailbox).await?;
                ensure_message_exists(session, uid).await?;
                session.copy(uid, to).await
            })
        })
        .await
    }

    /// Moves the single message `uid` from `mailbox` to `to`.
    ///
    /// Uses `UID MOVE` when the server announces `MOVE`; otherwise, when it
    /// announces `UIDPLUS`, emulates the move with `UID COPY`, then
    /// `UID STORE +FLAGS.SILENT (\Deleted)`, then `UID EXPUNGE` (copying before
    /// deleting so mail is never lost). When neither capability is announced the
    /// operation fails with [`ImapError::CapabilityNotSupported`] and sends no
    /// command. The message's existence is checked **before** the capability
    /// test, so a missing `uid` is reported as [`ImapError::MessageNotFound`].
    pub async fn move_message(&self, mailbox: &str, uid: u32, to: &str) -> Result<(), ImapError> {
        let mailbox = mailbox.to_string();
        let to = to.to_string();
        self.run_once(move |session| {
            Box::pin(async move {
                session.select(mailbox).await?;
                ensure_message_exists(session, uid).await?;
                let capabilities = session.capabilities().await?;
                if capabilities.has_move {
                    session.move_message(uid, to).await
                } else if capabilities.has_uidplus {
                    session.copy(uid, to).await?;
                    let query = FlagQuery::new(&[SystemFlag::Deleted], &[])?;
                    session.store(uid, query).await?;
                    session.uid_expunge(uid).await
                } else {
                    Err(ImapError::CapabilityNotSupported)
                }
            })
        })
        .await
    }

    /// Deletes the single message `uid` from `mailbox`.
    ///
    /// Requires the server to announce `UIDPLUS` so `UID EXPUNGE` can target only
    /// that message; without it the operation fails with
    /// [`ImapError::CapabilityNotSupported`] and **never** falls back to a global
    /// `EXPUNGE`. The message's existence is checked **before** the capability
    /// test, so a missing `uid` is reported as [`ImapError::MessageNotFound`].
    /// The message is marked `\Deleted` and then expunged.
    pub async fn delete_message(&self, mailbox: &str, uid: u32) -> Result<(), ImapError> {
        let mailbox = mailbox.to_string();
        self.run_once(move |session| {
            Box::pin(async move {
                session.select(mailbox).await?;
                ensure_message_exists(session, uid).await?;
                let capabilities = session.capabilities().await?;
                if !capabilities.has_uidplus {
                    return Err(ImapError::CapabilityNotSupported);
                }
                let query = FlagQuery::new(&[SystemFlag::Deleted], &[])?;
                session.store(uid, query).await?;
                session.uid_expunge(uid).await
            })
        })
        .await
    }
}

/// Fails with [`ImapError::MessageNotFound`] when `uid` is absent from the
/// selected mailbox, using a summary `UID FETCH`.
async fn ensure_message_exists(
    session: &mut Box<dyn ImapSession>,
    uid: u32,
) -> Result<(), ImapError> {
    let messages = session.fetch(vec![uid], FetchFormat::Summary).await?;
    if messages.is_empty() {
        Err(ImapError::MessageNotFound)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_imap::imap_proto;

    use super::*;

    /// Scripted outcome of a `select` call on a [`FakeSession`].
    #[derive(Clone)]
    enum SelectScript {
        /// Return this status.
        Status(MailboxStatus),
        /// Answer `NO`, as a server does for an unknown mailbox.
        NotFound,
        /// Fail as a connection error.
        Connection,
    }

    /// A fake session whose `NOOP`, `list_mailboxes`, `select`, `search` and
    /// `fetch` outcomes are scripted.
    #[derive(Clone)]
    struct FakeSession {
        /// When `true`, every `noop` fails.
        dead: bool,
        /// Mailboxes returned by `list_mailboxes`.
        mailboxes: Vec<MailboxInfo>,
        /// Outcome of `select`.
        select: SelectScript,
        /// UIDs returned by `search`.
        uids: Vec<u32>,
        /// Messages returned by `fetch`.
        messages: Vec<Message>,
        /// When set, a `FetchFormat::Full` fetch attaches these bytes as `body`
        /// to every returned message.
        full_body: Option<Vec<u8>>,
        /// UID sets received by `fetch`, in call order.
        fetch_uids: Arc<Mutex<Vec<Vec<u32>>>>,
        /// Capabilities reported by `capabilities`.
        capabilities: Capabilities,
        /// Every mutating command received, in call order, as rendered strings.
        ops: Arc<Mutex<Vec<String>>>,
    }

    impl FakeSession {
        /// A healthy session with an empty mailbox list.
        fn healthy() -> Self {
            Self {
                dead: false,
                mailboxes: Vec::new(),
                select: SelectScript::Status(default_status()),
                uids: Vec::new(),
                messages: Vec::new(),
                full_body: None,
                fetch_uids: Arc::new(Mutex::new(Vec::new())),
                capabilities: Capabilities::default(),
                ops: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// A session whose `noop` fails.
        fn dead() -> Self {
            Self {
                dead: true,
                mailboxes: Vec::new(),
                select: SelectScript::Connection,
                uids: Vec::new(),
                messages: Vec::new(),
                full_body: None,
                fetch_uids: Arc::new(Mutex::new(Vec::new())),
                capabilities: Capabilities::default(),
                ops: Arc::new(Mutex::new(Vec::new())),
            }
        }

        /// Sets the UIDs returned by `search`.
        fn with_uids(mut self, uids: Vec<u32>) -> Self {
            self.uids = uids;
            self
        }

        /// Sets the messages returned by `fetch`.
        fn with_messages(mut self, messages: Vec<Message>) -> Self {
            self.messages = messages;
            self
        }

        /// Serves `body = Some(bytes)` for a `FetchFormat::Full` fetch.
        fn with_full_body(mut self, body: Vec<u8>) -> Self {
            self.full_body = Some(body);
            self
        }

        /// Shares the UID-set log so a test can assert the fetched window.
        fn with_fetch_uids_log(mut self, log: Arc<Mutex<Vec<Vec<u32>>>>) -> Self {
            self.fetch_uids = log;
            self
        }

        /// Sets the capabilities reported by `capabilities`.
        fn with_capabilities(mut self, capabilities: Capabilities) -> Self {
            self.capabilities = capabilities;
            self
        }

        /// Sets the mailboxes returned by `list_mailboxes`.
        fn with_mailboxes(mut self, mailboxes: Vec<MailboxInfo>) -> Self {
            self.mailboxes = mailboxes;
            self
        }

        /// Sets the status returned by `select`.
        fn with_status(mut self, status: MailboxStatus) -> Self {
            self.select = SelectScript::Status(status);
            self
        }

        /// Makes `select` answer `NO`.
        fn not_found(mut self) -> Self {
            self.select = SelectScript::NotFound;
            self
        }

        /// Makes `select` fail with a connection error.
        fn select_connection_error(mut self) -> Self {
            self.select = SelectScript::Connection;
            self
        }
    }

    /// A representative mailbox status used by the fakes.
    fn default_status() -> MailboxStatus {
        MailboxStatus {
            exists: 0,
            recent: 0,
            unseen: None,
            uid_validity: None,
            uid_next: None,
            flags: Vec::new(),
        }
    }

    /// A representative mailbox.
    fn inbox() -> MailboxInfo {
        MailboxInfo {
            name: "INBOX".to_string(),
            delimiter: Some("/".to_string()),
            attributes: vec!["\\HasNoChildren".to_string()],
        }
    }

    /// A representative message with the given UID.
    fn message(uid: u32) -> Message {
        Message {
            uid,
            seq: uid,
            flags: vec!["\\Seen".to_string()],
            size: Some(1024),
            internal_date: Some("2024-02-05T10:00:00+00:00".to_string()),
            envelope: None,
            headers: None,
            body: None,
        }
    }

    /// Builds a raw `imap-proto` address for the mapping tests.
    fn raw_address(
        name: Option<&[u8]>,
        mailbox: Option<&[u8]>,
        host: Option<&[u8]>,
    ) -> imap_proto::Address<'static> {
        imap_proto::Address {
            name: name.map(|value| Cow::Owned(value.to_vec())),
            adl: None,
            mailbox: mailbox.map(|value| Cow::Owned(value.to_vec())),
            host: host.map(|value| Cow::Owned(value.to_vec())),
        }
    }

    impl ImapSession for FakeSession {
        fn noop(&mut self) -> SendFuture<'_, Result<(), ImapError>> {
            let dead = self.dead;
            Box::pin(async move {
                if dead {
                    Err(ImapError::Unavailable("simulated dead session".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn list_mailboxes(&mut self) -> SendFuture<'_, Result<Vec<MailboxInfo>, ImapError>> {
            let mailboxes = self.mailboxes.clone();
            Box::pin(async move { Ok(mailboxes) })
        }

        fn select(&mut self, _mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>> {
            let script = self.select.clone();
            Box::pin(async move {
                match script {
                    SelectScript::Status(status) => Ok(status),
                    SelectScript::NotFound => Err(ImapError::MailboxNotFound),
                    SelectScript::Connection => {
                        Err(ImapError::Unavailable("simulated dead session".to_string()))
                    }
                }
            })
        }

        fn search(
            &mut self,
            _criteria: SearchCriteria,
        ) -> SendFuture<'_, Result<Vec<u32>, ImapError>> {
            let uids = self.uids.clone();
            Box::pin(async move { Ok(uids) })
        }

        fn fetch(
            &mut self,
            uids: Vec<u32>,
            format: FetchFormat,
        ) -> SendFuture<'_, Result<Vec<Message>, ImapError>> {
            self.fetch_uids
                .lock()
                .expect("fetch uids log poisoned")
                .push(uids.clone());
            // Only the requested messages are returned, in the scripted order, so
            // a wrong window or a missing sort in the caller is observable.
            let mut messages: Vec<Message> = self
                .messages
                .iter()
                .filter(|message| uids.contains(&message.uid))
                .cloned()
                .collect();
            // The full body is only served for a `Full` fetch; a `Summary` keeps
            // the scripted `size` and no body.
            if format == FetchFormat::Full
                && let Some(body) = &self.full_body
            {
                for message in &mut messages {
                    message.body = Some(body.clone());
                }
            }
            Box::pin(async move { Ok(messages) })
        }

        fn store(&mut self, uid: u32, query: FlagQuery) -> SendFuture<'_, Result<(), ImapError>> {
            self.ops
                .lock()
                .expect("ops log poisoned")
                .push(format!("store {uid} {}", query.as_str()));
            Box::pin(async move { Ok(()) })
        }

        fn copy(&mut self, uid: u32, mailbox: String) -> SendFuture<'_, Result<(), ImapError>> {
            self.ops
                .lock()
                .expect("ops log poisoned")
                .push(format!("copy {uid} {mailbox}"));
            Box::pin(async move { Ok(()) })
        }

        fn move_message(
            &mut self,
            uid: u32,
            mailbox: String,
        ) -> SendFuture<'_, Result<(), ImapError>> {
            self.ops
                .lock()
                .expect("ops log poisoned")
                .push(format!("move {uid} {mailbox}"));
            Box::pin(async move { Ok(()) })
        }

        fn uid_expunge(&mut self, uid: u32) -> SendFuture<'_, Result<(), ImapError>> {
            self.ops
                .lock()
                .expect("ops log poisoned")
                .push(format!("expunge {uid}"));
            Box::pin(async move { Ok(()) })
        }

        fn capabilities(&mut self) -> SendFuture<'_, Result<Capabilities, ImapError>> {
            let capabilities = self.capabilities;
            Box::pin(async move { Ok(capabilities) })
        }
    }

    /// A scripted outcome for a single `connect` call.
    enum Outcome {
        /// Hands back the given fake session.
        Session(FakeSession),
    }

    /// A fake [`ImapConnector`] that counts calls and follows a script.
    #[derive(Clone, Default)]
    struct FakeConnector {
        /// Number of `connect` calls so far.
        connects: Arc<AtomicUsize>,
        /// Remaining scripted outcomes; an empty queue always fails.
        script: Arc<Mutex<VecDeque<Outcome>>>,
    }

    impl FakeConnector {
        /// Builds a connector with the given scripted outcomes.
        fn scripted(outcomes: impl IntoIterator<Item = Outcome>) -> Self {
            Self {
                connects: Arc::new(AtomicUsize::new(0)),
                script: Arc::new(Mutex::new(outcomes.into_iter().collect())),
            }
        }

        /// Number of `connect` calls observed.
        fn connects(&self) -> usize {
            self.connects.load(Ordering::SeqCst)
        }
    }

    impl ImapConnector for FakeConnector {
        fn connect(&self) -> SendFuture<'_, Result<Box<dyn ImapSession>, ImapError>> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            let next = self
                .script
                .lock()
                .expect("script lock poisoned")
                .pop_front();
            Box::pin(async move {
                match next {
                    Some(Outcome::Session(session)) => {
                        Ok(Box::new(session) as Box<dyn ImapSession>)
                    }
                    _ => Err(ImapError::Unavailable(
                        "simulated connect failure".to_string(),
                    )),
                }
            })
        }
    }

    /// A [`Config`] with the given IMAP TLS mode and password.
    fn config(mode: TlsMode) -> Config {
        Config::from_lookup(move |key| match key {
            "APIMAIL_API_KEY" => Some("test-key".to_string()),
            "APIMAIL_IMAP_HOST" => Some("imap.test.example".to_string()),
            "APIMAIL_IMAP_USER" => Some("user@test.example".to_string()),
            "APIMAIL_IMAP_PASSWORD" => Some("imap-secret".to_string()),
            "APIMAIL_IMAP_TLS" => Some(mode.as_str().to_string()),
            "APIMAIL_SMTP_HOST" => Some("smtp.test.example".to_string()),
            "APIMAIL_SMTP_USER" => Some("smtp-user@test.example".to_string()),
            "APIMAIL_SMTP_PASSWORD" => Some("smtp-secret".to_string()),
            _ => None,
        })
        .expect("valid test config")
    }

    /// A production-like manager whose retries do not sleep.
    fn manager(connector: FakeConnector) -> ConnectionManager {
        ConnectionManager::new(
            Arc::new(connector),
            Backoff {
                attempts: 3,
                base: Duration::ZERO,
                factor: 2,
                max: Duration::from_secs(4),
            },
        )
    }

    #[test]
    fn tls_strategy_maps_each_mode() {
        assert_eq!(tls_strategy(TlsMode::Implicit), TlsStrategy::Implicit);
        assert_eq!(tls_strategy(TlsMode::StartTls), TlsStrategy::StartTls);
        assert_eq!(tls_strategy(TlsMode::Plain), TlsStrategy::Plain);
    }

    #[test]
    fn backoff_grows_and_saturates_at_max() {
        let backoff = Backoff {
            attempts: 5,
            base: Duration::from_millis(100),
            factor: 2,
            max: Duration::from_millis(350),
        };
        assert_eq!(backoff.delay(1), Duration::from_millis(100));
        assert_eq!(backoff.delay(2), Duration::from_millis(200));
        assert_eq!(backoff.delay(3), Duration::from_millis(350));
        assert_eq!(backoff.delay(10), Duration::from_millis(350));

        let zero = Backoff {
            base: Duration::ZERO,
            ..Backoff::default()
        };
        assert_eq!(zero.delay(1), Duration::ZERO);
    }

    #[test]
    fn public_message_and_kind_map_every_variant_without_leaking_details() {
        let tcp = ImapError::Tcp(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connect to 10.0.0.5:993 refused",
        ));
        let tls = ImapError::Tls("certificate for internal.example rejected".to_string());
        let tls_config = ImapError::TlsConfig("ring provider unavailable".to_string());
        let invalid_name = ImapError::InvalidServerName("10.0.0.5".to_string());
        let login = ImapError::Login("user@test.example rejected".to_string());
        let unavailable = ImapError::Unavailable("10.0.0.5 closed the connection".to_string());
        let timeout = ImapError::Timeout(Duration::from_secs(30));
        let io = ImapError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "socket 10.0.0.5",
        ));
        let not_found = ImapError::MailboxNotFound;
        let message_not_found = ImapError::MessageNotFound;
        let unsupported = ImapError::CapabilityNotSupported;
        let invalid_flags = ImapError::InvalidFlags;

        for (error, message, kind) in [
            (&tcp, "could not connect to the IMAP server", "tcp"),
            (&timeout, "could not connect to the IMAP server", "timeout"),
            (
                &unavailable,
                "could not connect to the IMAP server",
                "unavailable",
            ),
            (&tls, "IMAP TLS handshake failed", "tls"),
            (&tls_config, "IMAP TLS handshake failed", "tls"),
            (&invalid_name, "IMAP TLS handshake failed", "tls"),
            (&login, "IMAP authentication or command failed", "login"),
            (&io, "IMAP I/O error", "io"),
            (&not_found, "mailbox not found", "not_found"),
            (&message_not_found, "message not found", "not_found"),
            (
                &unsupported,
                "the server does not support this operation",
                "unsupported",
            ),
            (&invalid_flags, "invalid flags", "invalid_flags"),
        ] {
            assert_eq!(error.public_message(), message, "{error:?}");
            assert_eq!(error.kind(), kind, "{error:?}");
            assert!(
                !error.public_message().contains("10.0.0.5"),
                "the public message leaked a network detail: {}",
                error.public_message()
            );
            assert!(
                !error.public_message().contains("internal.example"),
                "the public message leaked a host name: {}",
                error.public_message()
            );
        }
    }

    #[tokio::test]
    async fn zero_attempts_is_normalized_to_one() {
        let connector = FakeConnector::scripted([]);
        let manager = ConnectionManager::new(
            Arc::new(connector.clone()),
            Backoff {
                attempts: 0,
                base: Duration::ZERO,
                ..Backoff::default()
            },
        );

        assert_eq!(manager.backoff.attempts, 1, "attempts must be normalized");

        let error = manager
            .status()
            .await
            .expect_err("the single attempt must fail");
        assert!(matches!(error, ImapError::Unavailable(_)), "{error:?}");
        assert_eq!(
            connector.connects(),
            1,
            "a zeroed policy must still make exactly one attempt"
        );
    }

    #[test]
    fn from_config_builds_without_opening_network() {
        for mode in [TlsMode::Implicit, TlsMode::StartTls, TlsMode::Plain] {
            let connector = TokioImapConnector::from_config(&config(mode))
                .expect("building the connector must not touch the network");
            assert_eq!(connector.endpoint.tls, mode);
            assert_eq!(connector.timeout, Duration::from_secs(30));
        }
    }

    #[tokio::test]
    async fn first_status_connects_and_second_reuses_the_session() {
        let connector = FakeConnector::scripted([Outcome::Session(FakeSession::healthy())]);
        let manager = manager(connector.clone());

        manager.status().await.expect("first use should connect");
        assert_eq!(connector.connects(), 1, "first use should open one session");

        manager
            .status()
            .await
            .expect("second use should reuse the live session");
        assert_eq!(
            connector.connects(),
            1,
            "a live session must be reused, not reconnected"
        );
    }

    #[tokio::test]
    async fn dead_session_is_discarded_and_reconnected() {
        // First connect yields a session whose `noop` fails; the retry yields a
        // healthy one.
        let connector = FakeConnector::scripted([
            Outcome::Session(FakeSession::dead()),
            Outcome::Session(FakeSession::healthy()),
        ]);
        let manager = manager(connector.clone());

        manager.status().await.expect("first use should connect");
        manager
            .status()
            .await
            .expect("the dead session should be replaced");
        assert_eq!(
            connector.connects(),
            2,
            "the dead session must be discarded and one reconnect attempted"
        );
    }

    #[tokio::test]
    async fn always_failing_connector_gives_up_after_bounded_attempts() {
        let connector = FakeConnector::scripted([]);
        let manager = manager(connector.clone());

        let error = manager.status().await.expect_err("all attempts must fail");
        assert!(matches!(error, ImapError::Unavailable(_)), "{error:?}");
        assert_eq!(
            connector.connects(),
            3,
            "the manager must stop after `attempts` connections"
        );
    }

    #[test]
    fn manager_debug_does_not_leak_credentials() {
        let connector = Arc::new(
            TokioImapConnector::from_config(&config(TlsMode::Implicit)).expect("valid connector"),
        );
        let manager = ConnectionManager::new(connector, Backoff::default());
        let rendered = format!("{manager:?}");

        assert!(
            !rendered.contains("imap-secret"),
            "manager Debug leaked the password: {rendered}"
        );
        assert!(
            !rendered.contains("user@test.example"),
            "manager Debug leaked the username: {rendered}"
        );
        assert!(
            rendered.contains("***"),
            "manager Debug is missing the redaction marker: {rendered}"
        );
        assert!(
            rendered.contains("Backoff"),
            "manager Debug should still expose the retry policy: {rendered}"
        );
    }

    #[test]
    fn flag_label_renders_system_flags_and_keywords() {
        assert_eq!(flag_label(&Flag::Seen), "\\Seen");
        assert_eq!(flag_label(&Flag::Answered), "\\Answered");
        assert_eq!(flag_label(&Flag::Flagged), "\\Flagged");
        assert_eq!(flag_label(&Flag::Deleted), "\\Deleted");
        assert_eq!(flag_label(&Flag::Draft), "\\Draft");
        assert_eq!(flag_label(&Flag::Recent), "\\Recent");
        assert_eq!(flag_label(&Flag::MayCreate), "\\*");
        assert_eq!(
            flag_label(&Flag::Custom(Cow::Borrowed("$Forwarded"))),
            "$Forwarded"
        );
    }

    #[test]
    fn attribute_label_renders_known_attributes_and_extensions() {
        for (attribute, label) in [
            (NameAttribute::NoInferiors, "\\NoInferiors"),
            (NameAttribute::NoSelect, "\\NoSelect"),
            (NameAttribute::Marked, "\\Marked"),
            (NameAttribute::Unmarked, "\\Unmarked"),
            (NameAttribute::All, "\\All"),
            (NameAttribute::Archive, "\\Archive"),
            (NameAttribute::Drafts, "\\Drafts"),
            (NameAttribute::Flagged, "\\Flagged"),
            (NameAttribute::Junk, "\\Junk"),
            (NameAttribute::Sent, "\\Sent"),
            (NameAttribute::Trash, "\\Trash"),
        ] {
            assert_eq!(attribute_label(&attribute), label, "{attribute:?}");
        }
        // `imap-proto` parses the extension with the leading backslash already
        // included, so the label must not add a second one.
        assert_eq!(
            attribute_label(&NameAttribute::Extension(Cow::Borrowed("\\HasNoChildren"))),
            "\\HasNoChildren"
        );
        assert_eq!(
            attribute_label(&NameAttribute::Extension(Cow::Borrowed("\\HasChildren"))),
            "\\HasChildren"
        );
        // A future variant exposing the name without a backslash is normalised
        // to exactly one.
        assert_eq!(
            attribute_label(&NameAttribute::Extension(Cow::Borrowed("HasChildren"))),
            "\\HasChildren"
        );
    }

    #[tokio::test]
    async fn list_mailboxes_returns_scripted_mailboxes_and_reuses_the_session() {
        let connector = FakeConnector::scripted([Outcome::Session(
            FakeSession::healthy().with_mailboxes(vec![inbox()]),
        )]);
        let manager = manager(connector.clone());

        let first = manager
            .list_mailboxes()
            .await
            .expect("listing should succeed");
        assert_eq!(first, vec![inbox()]);

        let second = manager
            .list_mailboxes()
            .await
            .expect("second listing should succeed");
        assert_eq!(second, vec![inbox()]);
        assert_eq!(
            connector.connects(),
            1,
            "a live session must be reused across listings"
        );
    }

    #[tokio::test]
    async fn select_mailbox_returns_the_scripted_status() {
        let status = MailboxStatus {
            exists: 42,
            recent: 1,
            unseen: Some(3),
            uid_validity: Some(7),
            uid_next: Some(100),
            flags: vec!["\\Seen".to_string(), "\\Flagged".to_string()],
        };
        let connector = FakeConnector::scripted([Outcome::Session(
            FakeSession::healthy().with_status(status.clone()),
        )]);
        let manager = manager(connector.clone());

        let observed = manager
            .select_mailbox("INBOX")
            .await
            .expect("selection should succeed");
        assert_eq!(observed, status);
        assert_eq!(connector.connects(), 1);
    }

    #[tokio::test]
    async fn select_unknown_mailbox_is_not_found_and_keeps_the_session() {
        let connector =
            FakeConnector::scripted([Outcome::Session(FakeSession::healthy().not_found())]);
        let manager = manager(connector.clone());

        let error = manager
            .select_mailbox("Missing")
            .await
            .expect_err("an unknown mailbox must fail");
        assert!(matches!(error, ImapError::MailboxNotFound), "{error:?}");
        assert!(!error.is_connection(), "a `NO` is not a connection error");
        assert_eq!(connector.connects(), 1);

        // The session is kept, so the next operation reuses it.
        manager
            .list_mailboxes()
            .await
            .expect("the live session must be kept");
        assert_eq!(
            connector.connects(),
            1,
            "a `NO` must not discard the live session"
        );
    }

    #[tokio::test]
    async fn dead_session_is_replaced_for_mailbox_operations() {
        // First connect yields a session whose `noop` fails; when the next
        // operation probes it, it is discarded and replaced with a healthy one.
        let connector = FakeConnector::scripted([
            Outcome::Session(FakeSession::dead()),
            Outcome::Session(FakeSession::healthy().with_mailboxes(vec![inbox()])),
        ]);
        let manager = manager(connector.clone());

        manager
            .status()
            .await
            .expect("the first use connects the (soon-to-be-dead) session");

        let mailboxes = manager
            .list_mailboxes()
            .await
            .expect("the dead session should be replaced and the listing run");
        assert_eq!(mailboxes, vec![inbox()]);
        assert_eq!(
            connector.connects(),
            2,
            "the dead session must be discarded and one reconnect attempted"
        );
    }

    #[tokio::test]
    async fn connection_error_on_a_command_discards_the_session() {
        let connector = FakeConnector::scripted([
            Outcome::Session(FakeSession::healthy().select_connection_error()),
            Outcome::Session(FakeSession::healthy()),
        ]);
        let manager = manager(connector.clone());

        let error = manager
            .select_mailbox("INBOX")
            .await
            .expect_err("the command must surface the connection error");
        assert!(error.is_connection(), "{error:?}");

        // The cached session was discarded, so the next use reconnects.
        manager
            .status()
            .await
            .expect("a fresh session should be established");
        assert_eq!(
            connector.connects(),
            2,
            "a connection error on a command must discard the session"
        );
    }

    #[test]
    fn is_connection_classifies_transport_and_protocol_errors() {
        let connection = [
            ImapError::Tcp(std::io::Error::new(std::io::ErrorKind::TimedOut, "tcp")),
            ImapError::Tls("tls".to_string()),
            ImapError::TlsConfig("tls config".to_string()),
            ImapError::InvalidServerName("name".to_string()),
            ImapError::Timeout(Duration::from_secs(1)),
            ImapError::Unavailable("down".to_string()),
            ImapError::Io(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "io")),
            ImapError::Imap(async_imap::error::Error::ConnectionLost),
            ImapError::Imap(async_imap::error::Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "io",
            ))),
        ];
        for error in &connection {
            assert!(
                error.is_connection(),
                "expected a connection error: {error:?}"
            );
        }

        let logical = [
            ImapError::Login("nope".to_string()),
            ImapError::MailboxNotFound,
            ImapError::MessageNotFound,
            ImapError::CapabilityNotSupported,
            ImapError::InvalidFlags,
            ImapError::Imap(async_imap::error::Error::No("no".to_string())),
            ImapError::Imap(async_imap::error::Error::Bad("bad".to_string())),
        ];
        for error in &logical {
            assert!(
                !error.is_connection(),
                "expected a non-connection error: {error:?}"
            );
        }
    }

    #[test]
    fn quote_search_string_escapes_and_strips_control_characters() {
        assert_eq!(quote_search_string("plain"), "\"plain\"");
        assert_eq!(quote_search_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(quote_search_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(quote_search_string("a\r\nb"), "\"ab\"");
        assert_eq!(quote_search_string("a\0b"), "\"ab\"");

        // The quoted string must never carry a raw control character.
        let quoted = quote_search_string("x\r\ny\n\0z");
        assert!(!quoted.contains('\r'), "{quoted:?}");
        assert!(!quoted.contains('\n'), "{quoted:?}");
        assert!(!quoted.contains('\0'), "{quoted:?}");
    }

    #[test]
    fn search_criteria_without_filters_is_all() {
        assert_eq!(SearchCriteria::default().imap_key(), "ALL");
    }

    #[test]
    fn search_criteria_renders_text_keys_quoted() {
        let criteria = SearchCriteria {
            from: Some("boss@example.com".to_string()),
            to: Some("me@example.com".to_string()),
            subject: Some("hola \"mundo\"".to_string()),
            text: Some("cuerpo".to_string()),
            ..SearchCriteria::default()
        };
        assert_eq!(
            criteria.imap_key(),
            "FROM \"boss@example.com\" TO \"me@example.com\" \
             SUBJECT \"hola \\\"mundo\\\"\" BODY \"cuerpo\""
        );
    }

    #[test]
    fn search_criteria_renders_dates_and_flags() {
        let since = SearchDate::parse("2024-01-05").expect("valid date");
        let before = SearchDate::parse("2024-12-31").expect("valid date");
        let criteria = SearchCriteria {
            since: Some(since),
            before: Some(before),
            seen: Some(false),
            flagged: Some(true),
            ..SearchCriteria::default()
        };
        assert_eq!(
            criteria.imap_key(),
            "SINCE 05-Jan-2024 BEFORE 31-Dec-2024 UNSEEN FLAGGED"
        );

        let criteria = SearchCriteria {
            seen: Some(true),
            flagged: Some(false),
            ..SearchCriteria::default()
        };
        assert_eq!(criteria.imap_key(), "SEEN UNFLAGGED");
    }

    #[test]
    fn search_criteria_drops_control_characters() {
        let criteria = SearchCriteria {
            subject: Some("x\r\nSEARCH ALL".to_string()),
            ..SearchCriteria::default()
        };
        let key = criteria.imap_key();
        assert_eq!(key, "SUBJECT \"xSEARCH ALL\"");
        assert!(!key.contains('\n'), "{key:?}");
    }

    #[test]
    fn search_date_parse_accepts_valid_and_rejects_invalid() {
        assert_eq!(
            SearchDate::parse("2024-02-29")
                .expect("2024 is a leap year")
                .to_imap(),
            "29-Feb-2024"
        );
        assert_eq!(
            SearchDate::parse("2023-12-01")
                .expect("valid date")
                .to_imap(),
            "01-Dec-2023"
        );

        for invalid in [
            "2024-2-9",
            "2024-02-9",
            "2024-2-09",
            "24-02-09",
            "2024-13-01",
            "2024-00-01",
            "2024-01-00",
            "2024-01-32",
            "2023-02-29",
            "0000-01-01",
            "2024/02/09",
            "not-a-date",
            "",
            "2024-01-05T00:00:00",
        ] {
            assert!(
                SearchDate::parse(invalid).is_err(),
                "`{invalid}` should be rejected"
            );
        }
    }

    #[test]
    fn fetch_format_queries_are_the_exact_constants() {
        assert_eq!(
            FetchFormat::Summary.query(),
            "(UID FLAGS RFC822.SIZE INTERNALDATE ENVELOPE)"
        );
        assert_eq!(
            FetchFormat::Headers.query(),
            "(UID FLAGS RFC822.SIZE INTERNALDATE ENVELOPE BODY.PEEK[HEADER])"
        );
        assert_eq!(
            FetchFormat::Full.query(),
            "(UID FLAGS RFC822.SIZE INTERNALDATE ENVELOPE BODY.PEEK[])"
        );
        // Bodies are always requested with `PEEK`, so no `\Seen` is set.
        let summary = FetchFormat::Summary.query();
        assert!(!summary.contains("BODY["), "{summary}");
        assert!(FetchFormat::Headers.query().contains("BODY.PEEK[HEADER]"));
        assert!(FetchFormat::Full.query().contains("BODY.PEEK[]"));
    }

    #[test]
    fn address_to_dto_builds_mailbox_at_host() {
        assert_eq!(
            address_to_dto(&raw_address(
                Some(b"Jane Doe"),
                Some(b"jane"),
                Some(b"example.com")
            )),
            Address {
                name: Some("Jane Doe".to_string()),
                address: Some("jane@example.com".to_string()),
            }
        );
        assert_eq!(
            address_to_dto(&raw_address(None, Some(b"jane"), None)),
            Address {
                name: None,
                address: Some("jane".to_string()),
            }
        );
        assert_eq!(
            address_to_dto(&raw_address(None, None, Some(b"example.com"))),
            Address {
                name: None,
                address: None,
            }
        );
    }

    #[test]
    fn envelope_to_dto_maps_addresses_and_text_fields() {
        let envelope = imap_proto::Envelope {
            date: Some(Cow::Borrowed(b"Mon, 05 Feb 2024 10:00:00 +0000")),
            subject: Some(Cow::Borrowed(b"Hi")),
            from: Some(vec![raw_address(
                Some(b"Jane"),
                Some(b"jane"),
                Some(b"example.com"),
            )]),
            sender: None,
            reply_to: None,
            to: Some(vec![raw_address(None, Some(b"me"), Some(b"example.com"))]),
            cc: None,
            bcc: None,
            in_reply_to: None,
            message_id: Some(Cow::Borrowed(b"<id@example.com>")),
        };

        let dto = envelope_to_dto(&envelope);
        assert_eq!(dto.subject.as_deref(), Some("Hi"));
        assert_eq!(dto.date.as_deref(), Some("Mon, 05 Feb 2024 10:00:00 +0000"));
        assert_eq!(dto.message_id.as_deref(), Some("<id@example.com>"));
        assert_eq!(
            dto.from,
            vec![Address {
                name: Some("Jane".to_string()),
                address: Some("jane@example.com".to_string()),
            }]
        );
        assert_eq!(
            dto.to,
            vec![Address {
                name: None,
                address: Some("me@example.com".to_string()),
            }]
        );
        assert!(dto.cc.is_empty(), "a missing cc must map to an empty list");
    }

    #[tokio::test]
    async fn list_messages_reports_total_window_and_descending_order() {
        let fetch_uids = Arc::new(Mutex::new(Vec::new()));
        let connector = FakeConnector::scripted([Outcome::Session(
            FakeSession::healthy()
                .with_uids(vec![9, 7, 5, 3, 1])
                // Deliberately not newest-first: the manager must impose the order.
                .with_messages(vec![message(5), message(7)])
                .with_fetch_uids_log(Arc::clone(&fetch_uids)),
        )]);
        let manager = manager(connector.clone());

        let page = manager
            .list_messages("INBOX", SearchCriteria::default(), 2, 1)
            .await
            .expect("listing should succeed");
        assert_eq!(page.total, 5, "total must count every matching message");
        assert_eq!(page.limit, 2);
        assert_eq!(page.offset, 1);
        assert_eq!(
            page.messages,
            vec![message(7), message(5)],
            "messages must be newest-first regardless of the FETCH response order"
        );
        assert_eq!(
            fetch_uids.lock().expect("fetch uids log poisoned").clone(),
            vec![vec![7, 5]],
            "the FETCH must request exactly the paginated window"
        );
        assert_eq!(connector.connects(), 1);
    }

    #[tokio::test]
    async fn list_messages_past_the_end_is_empty() {
        let fetch_uids = Arc::new(Mutex::new(Vec::new()));
        let connector = FakeConnector::scripted([Outcome::Session(
            FakeSession::healthy()
                .with_uids(vec![3, 1])
                .with_fetch_uids_log(Arc::clone(&fetch_uids)),
        )]);
        let manager = manager(connector.clone());

        let page = manager
            .list_messages("INBOX", SearchCriteria::default(), 10, 5)
            .await
            .expect("listing should succeed");
        assert_eq!(page.total, 2);
        assert_eq!(page.offset, 5);
        assert!(page.messages.is_empty(), "{:?}", page.messages);
        assert_eq!(
            fetch_uids.lock().expect("fetch uids log poisoned").clone(),
            vec![Vec::<u32>::new()],
            "an empty page must still be a well-formed (empty) window"
        );
    }

    #[tokio::test]
    async fn fetch_message_returns_the_first_message() {
        let connector = FakeConnector::scripted([Outcome::Session(
            FakeSession::healthy().with_messages(vec![message(42)]),
        )]);
        let manager = manager(connector.clone());

        let fetched = manager
            .fetch_message("INBOX", 42, FetchFormat::Full)
            .await
            .expect("the message must be found");
        assert_eq!(fetched.uid, 42);
    }

    #[tokio::test]
    async fn fetch_message_missing_uid_is_not_found() {
        let connector = FakeConnector::scripted([Outcome::Session(FakeSession::healthy())]);
        let manager = manager(connector.clone());

        let error = manager
            .fetch_message("INBOX", 42, FetchFormat::Summary)
            .await
            .expect_err("a missing message must fail");
        assert!(matches!(error, ImapError::MessageNotFound), "{error:?}");
        assert!(
            !error.is_connection(),
            "a missing message keeps the session"
        );
        assert_eq!(error.public_message(), "message not found");
        assert_eq!(error.kind(), "not_found");
    }

    // --- Flag domain additions (imap-flags) ---

    #[test]
    fn system_flag_parse_is_case_insensitive_and_requires_a_backslash() {
        for (value, expected) in [
            ("\\Seen", SystemFlag::Seen),
            ("\\seen", SystemFlag::Seen),
            ("\\ANSWERED", SystemFlag::Answered),
            ("\\Flagged", SystemFlag::Flagged),
            ("\\draft", SystemFlag::Draft),
            ("\\Deleted", SystemFlag::Deleted),
        ] {
            assert_eq!(
                SystemFlag::parse(value).expect("a known flag must parse"),
                expected,
                "`{value}`"
            );
        }

        for invalid in [
            "Seen",
            "\\Unknown",
            "\\Seen ",
            " \\Seen",
            "",
            "\\",
            "\\seen\\",
            "\\Seen\\Seen",
        ] {
            assert!(
                SystemFlag::parse(invalid).is_err(),
                "`{invalid}` must be rejected"
            );
        }
    }

    #[test]
    fn system_flag_to_imap_is_the_canonical_form() {
        assert_eq!(SystemFlag::Seen.to_imap(), "\\Seen");
        assert_eq!(SystemFlag::Answered.to_imap(), "\\Answered");
        assert_eq!(SystemFlag::Flagged.to_imap(), "\\Flagged");
        assert_eq!(SystemFlag::Draft.to_imap(), "\\Draft");
        assert_eq!(SystemFlag::Deleted.to_imap(), "\\Deleted");
    }

    #[test]
    fn flag_query_requires_at_least_one_flag() {
        assert!(matches!(FlagQuery::new(&[], &[]), Err(FlagError::Empty)));
    }

    #[test]
    fn flag_query_composes_fixed_items_and_allowlisted_flags_only() {
        let query = FlagQuery::new(
            &[SystemFlag::Seen, SystemFlag::Flagged],
            &[SystemFlag::Deleted],
        )
        .expect("a non-empty query must build");
        assert_eq!(
            query.as_str(),
            "+FLAGS.SILENT (\\Seen \\Flagged) -FLAGS.SILENT (\\Deleted)"
        );

        assert_eq!(
            FlagQuery::new(&[SystemFlag::Seen], &[])
                .expect("an add-only query must build")
                .as_str(),
            "+FLAGS.SILENT (\\Seen)"
        );
        assert_eq!(
            FlagQuery::new(&[], &[SystemFlag::Deleted])
                .expect("a remove-only query must build")
                .as_str(),
            "-FLAGS.SILENT (\\Deleted)"
        );
    }

    #[test]
    fn capability_error_is_unsupported_and_not_a_connection_error() {
        let error = ImapError::CapabilityNotSupported;
        assert_eq!(
            error.public_message(),
            "the server does not support this operation"
        );
        assert_eq!(error.kind(), "unsupported");
        assert!(!error.is_connection());
    }

    #[test]
    fn flag_error_maps_to_invalid_flags_without_leaking_internal_detail() {
        let error: ImapError = FlagError::Empty.into();
        assert!(matches!(&error, ImapError::InvalidFlags), "{error:?}");
        assert_eq!(error.public_message(), "invalid flags");
        assert_eq!(error.kind(), "invalid_flags");
        assert!(
            !error.is_connection(),
            "a validation failure must not discard a live session"
        );
        assert!(
            !error.public_message().contains("empty flag update"),
            "the mapped error leaked the internal FlagError text: {}",
            error.public_message()
        );
    }

    #[tokio::test]
    async fn update_flags_stores_then_returns_the_resulting_flags() {
        let session = FakeSession::healthy().with_messages(vec![Message {
            flags: vec!["\\Seen".to_string(), "\\Flagged".to_string()],
            ..message(42)
        }]);
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        let flags = manager
            .update_flags(
                "INBOX",
                42,
                vec![SystemFlag::Seen, SystemFlag::Flagged],
                Vec::new(),
            )
            .await
            .expect("the update must succeed");

        assert_eq!(flags, vec!["\\Seen".to_string(), "\\Flagged".to_string()]);
        assert_eq!(
            ops.lock().expect("ops log poisoned").as_slice(),
            ["store 42 +FLAGS.SILENT (\\Seen \\Flagged)"],
            "the STORE query must carry only fixed items and allowlisted flags"
        );
        assert_eq!(connector.connects(), 1);
    }

    #[tokio::test]
    async fn update_flags_missing_message_is_not_found() {
        let session = FakeSession::healthy();
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        let error = manager
            .update_flags("INBOX", 42, vec![SystemFlag::Seen], Vec::new())
            .await
            .expect_err("a missing message must fail");
        assert!(matches!(error, ImapError::MessageNotFound), "{error:?}");
        assert!(
            ops.lock().expect("ops log poisoned").is_empty(),
            "no STORE may be sent for a missing message"
        );
    }

    #[tokio::test]
    async fn copy_message_checks_existence_then_copies() {
        let session = FakeSession::healthy().with_messages(vec![message(42)]);
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        manager
            .copy_message("INBOX", 42, "Archive")
            .await
            .expect("the copy must succeed");

        assert_eq!(
            ops.lock().expect("ops log poisoned").as_slice(),
            ["copy 42 Archive"]
        );
        assert_eq!(connector.connects(), 1);
    }

    #[tokio::test]
    async fn copy_message_missing_message_is_not_found_and_sends_nothing() {
        let session = FakeSession::healthy();
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        let error = manager
            .copy_message("INBOX", 42, "Archive")
            .await
            .expect_err("a missing message must fail");
        assert!(matches!(error, ImapError::MessageNotFound), "{error:?}");
        assert!(
            ops.lock().expect("ops log poisoned").is_empty(),
            "no COPY must be sent for a missing message"
        );
    }

    #[tokio::test]
    async fn move_message_uses_move_when_supported() {
        let session = FakeSession::healthy()
            .with_messages(vec![message(42)])
            .with_capabilities(Capabilities {
                has_move: true,
                has_uidplus: false,
            });
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        manager
            .move_message("INBOX", 42, "Archive")
            .await
            .expect("the move must succeed");

        assert_eq!(
            ops.lock().expect("ops log poisoned").as_slice(),
            ["move 42 Archive"],
            "a server announcing MOVE must receive a single UID MOVE"
        );
    }

    #[tokio::test]
    async fn move_message_emulates_with_copy_store_expunge_when_only_uidplus() {
        let session = FakeSession::healthy()
            .with_messages(vec![message(42)])
            .with_capabilities(Capabilities {
                has_move: false,
                has_uidplus: true,
            });
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        manager
            .move_message("INBOX", 42, "Archive")
            .await
            .expect("the emulated move must succeed");

        assert_eq!(
            ops.lock().expect("ops log poisoned").as_slice(),
            [
                "copy 42 Archive",
                "store 42 +FLAGS.SILENT (\\Deleted)",
                "expunge 42"
            ],
            "the emulation must copy first, then mark, then expunge"
        );
    }

    #[tokio::test]
    async fn move_message_without_capabilities_is_unsupported_and_sends_nothing() {
        let session = FakeSession::healthy().with_messages(vec![message(42)]);
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        let error = manager
            .move_message("INBOX", 42, "Archive")
            .await
            .expect_err("an unsupported move must fail");
        assert!(
            matches!(error, ImapError::CapabilityNotSupported),
            "{error:?}"
        );
        assert!(
            ops.lock().expect("ops log poisoned").is_empty(),
            "nothing must be sent when neither MOVE nor UIDPLUS is announced"
        );
    }

    #[tokio::test]
    async fn move_message_missing_message_is_not_found_and_sends_nothing() {
        let session = FakeSession::healthy().with_capabilities(Capabilities {
            has_move: true,
            has_uidplus: false,
        });
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        let error = manager
            .move_message("INBOX", 42, "Archive")
            .await
            .expect_err("a missing message must fail");
        assert!(matches!(error, ImapError::MessageNotFound), "{error:?}");
        assert!(
            ops.lock().expect("ops log poisoned").is_empty(),
            "no MOVE must be sent for a missing message"
        );
    }

    #[tokio::test]
    async fn delete_message_requires_uidplus_and_sends_nothing_when_absent() {
        let session = FakeSession::healthy().with_messages(vec![message(42)]);
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        let error = manager
            .delete_message("INBOX", 42)
            .await
            .expect_err("deletion without UIDPLUS must fail");
        assert!(
            matches!(error, ImapError::CapabilityNotSupported),
            "{error:?}"
        );
        assert!(
            ops.lock().expect("ops log poisoned").is_empty(),
            "no EXPUNGE (global or targeted) must be sent without UIDPLUS"
        );
    }

    #[tokio::test]
    async fn delete_message_marks_deleted_then_uid_expunges() {
        let session = FakeSession::healthy()
            .with_messages(vec![message(42)])
            .with_capabilities(Capabilities {
                has_move: false,
                has_uidplus: true,
            });
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        manager
            .delete_message("INBOX", 42)
            .await
            .expect("the deletion must succeed");

        assert_eq!(
            ops.lock().expect("ops log poisoned").as_slice(),
            ["store 42 +FLAGS.SILENT (\\Deleted)", "expunge 42"]
        );
    }

    #[tokio::test]
    async fn delete_message_missing_message_is_not_found_and_sends_nothing() {
        let session = FakeSession::healthy().with_capabilities(Capabilities {
            has_move: false,
            has_uidplus: true,
        });
        let ops = Arc::clone(&session.ops);
        let connector = FakeConnector::scripted([Outcome::Session(session)]);
        let manager = manager(connector.clone());

        let error = manager
            .delete_message("INBOX", 42)
            .await
            .expect_err("a missing message must fail");
        assert!(matches!(error, ImapError::MessageNotFound), "{error:?}");
        assert!(
            ops.lock().expect("ops log poisoned").is_empty(),
            "a missing message must not be marked or expunged"
        );
    }

    // --- MIME parsing additions (mime-parsing) ---

    /// A minimal multipart message: a text body plus one decoded attachment.
    const RFC822: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Parsed
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary=\"B\"

--B
Content-Type: text/plain; charset=\"utf-8\"

Hello inbox
--B
Content-Type: application/octet-stream
Content-Disposition: attachment; filename=\"note.txt\"
Content-Transfer-Encoding: base64

SGVsbG8=
--B--
";

    #[tokio::test]
    async fn fetch_parsed_returns_the_parsed_message() {
        let connector = FakeConnector::scripted([Outcome::Session(
            FakeSession::healthy()
                .with_messages(vec![Message {
                    size: Some(RFC822.len() as u32),
                    ..message(42)
                }])
                .with_full_body(RFC822.as_bytes().to_vec()),
        )]);
        let manager = manager(connector.clone());

        let parsed = manager
            .fetch_parsed("INBOX", 42, 1024 * 1024)
            .await
            .expect("the message must be parsed");

        assert!(
            parsed
                .text
                .as_deref()
                .is_some_and(|text| text.contains("Hello inbox")),
            "{parsed:?}"
        );
        assert_eq!(parsed.attachments.len(), 1, "{parsed:?}");
        assert_eq!(parsed.attachments[0].filename.as_deref(), Some("note.txt"));
        assert_eq!(parsed.attachments[0].content, b"Hello");
        assert_eq!(connector.connects(), 1);
    }

    #[tokio::test]
    async fn fetch_parsed_missing_uid_is_message_not_found() {
        let connector = FakeConnector::scripted([Outcome::Session(FakeSession::healthy())]);
        let manager = manager(connector.clone());

        let error = manager
            .fetch_parsed("INBOX", 42, 1024)
            .await
            .expect_err("a missing message must fail");
        assert!(
            matches!(error, ParsedMessageError::Imap(ImapError::MessageNotFound)),
            "{error:?}"
        );
        assert!(
            !error.is_connection(),
            "a missing message must keep the cached session"
        );
    }

    #[tokio::test]
    async fn fetch_parsed_oversized_is_too_large_without_fetching_full() {
        let fetch_uids = Arc::new(Mutex::new(Vec::new()));
        let connector = FakeConnector::scripted([Outcome::Session(
            FakeSession::healthy()
                .with_messages(vec![Message {
                    size: Some(50),
                    ..message(42)
                }])
                .with_full_body(RFC822.as_bytes().to_vec())
                .with_fetch_uids_log(Arc::clone(&fetch_uids)),
        )]);
        let manager = manager(connector.clone());

        let error = manager
            .fetch_parsed("INBOX", 42, 10)
            .await
            .expect_err("an oversized message must fail");
        match error {
            ParsedMessageError::TooLarge { size, limit } => {
                assert_eq!(size, 50);
                assert_eq!(limit, 10);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        assert_eq!(
            fetch_uids.lock().expect("fetch uids log poisoned").len(),
            1,
            "only the Summary fetch may run: no Full fetch after a size rejection"
        );
    }

    #[tokio::test]
    async fn fetch_parsed_unparsable_body_is_unparsable() {
        let connector = FakeConnector::scripted([Outcome::Session(
            FakeSession::healthy()
                .with_messages(vec![Message {
                    size: Some(0),
                    ..message(42)
                }])
                .with_full_body(Vec::new()),
        )]);
        let manager = manager(connector.clone());

        let error = manager
            .fetch_parsed("INBOX", 42, 1024)
            .await
            .expect_err("an empty body must fail to parse");
        assert!(matches!(error, ParsedMessageError::Unparsable), "{error:?}");
    }

    #[test]
    fn parsed_message_error_has_stable_messages_and_classification() {
        let from_mime: ParsedMessageError = MimeError::Unparsable.into();
        assert!(matches!(from_mime, ParsedMessageError::Unparsable));
        assert_eq!(from_mime.to_string(), "message cannot be parsed");

        let too_large = ParsedMessageError::TooLarge {
            size: 50,
            limit: 10,
        };
        assert_eq!(too_large.to_string(), "message too large");

        assert!(!ParsedMessageError::Unparsable.is_connection());
        assert!(!too_large.is_connection());
        assert!(!ParsedMessageError::Imap(ImapError::MessageNotFound).is_connection());
        assert!(
            ParsedMessageError::Imap(ImapError::Unavailable("dead".to_string())).is_connection()
        );
    }
}
