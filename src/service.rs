//! Application service layer.
//!
//! This module owns the application's **operations** and the state they run
//! against, so that every transport shares one single source of truth instead
//! of duplicating logic: today the HTTP API in [`crate::http`], tomorrow the MCP
//! tools. It owns:
//!
//! - [`AppState`]: the process-wide configuration and services, built once at
//!   startup and cloned (it is a handle) into every request.
//! - The **response DTOs** of the operations implemented here — what a caller
//!   gets on success. They are also the output schema of the future MCP tools.
//! - [`ServiceError`]: the domain failure model.
//!
//! [`crate::http`] is the *transport*, and it owns what is purely HTTP: the
//! routes, the request DTOs (query and body), the `StatusCode` and the shared
//! `{"error":…,"message":…}` envelope. Handlers there are thin wrappers over the
//! operations of this module **as they migrate**, one operation at a time: today
//! `whoami`, `account`, `imap_status`, `list_mailboxes`, both IDLE reads
//! (`idle_status`, `idle_queue`) and the five message reads (`list_messages`,
//! `fetch_message`, `message_body`, `list_attachments`, `attachment`); the rest
//! still hold their own logic and their own per-route error type in
//! [`crate::http`].
//!
//! # Published HTTP error table
//!
//! This table is the contract of the API and is **unchanged** by the migration
//! of operations into this module. Its third column is the **frontier**: the
//! layer that *owns* each code — the one that must produce it, for every
//! transport, once its operation has migrated. A code is owned by the transport
//! when the condition that produces it exists only in that transport; it is
//! owned by the service when it is a domain outcome, so that HTTP and MCP get
//! one single producer instead of two implementations.
//!
//! Until an operation migrates, its code is still produced by that operation's
//! per-route error type in [`crate::http`] and [`ServiceError`] has no variant
//! for it yet: variants are added **with their operation**, never ahead of it,
//! so this table carries no speculative row.
//!
//! | HTTP | `error`                    | Owner               |
//! |------|----------------------------|---------------------|
//! | 401  | `unauthorized`             | transport           |
//! | 400  | `invalid_request`          | transport **and** service |
//! | 404  | `mailbox_not_found`        | service             |
//! | 404  | `message_not_found`        | service             |
//! | 404  | `attachment_not_found`     | service             |
//! | 413  | `message_too_large`        | service             |
//! | 413  | `payload_too_large`        | transport **and** service |
//! | 422  | `message_not_parsable`     | service             |
//! | 501  | `capability_not_supported` | service             |
//! | 501  | `idle_not_configured`      | service             |
//! | 502  | `smtp_error`               | service             |
//! | 503  | `imap_unavailable`         | service             |
//!
//! ## What the transport owns, and why
//!
//! - `unauthorized` (`401`): the auth middleware is the only place that sees the
//!   `Authorization` header, and the header is HTTP dialect.
//! - The `invalid_request` (`400`) that comes from **parsing** the input — the
//!   `serde`, query-string, base64 and type decoding of a request. A malformed
//!   request is not a domain failure: it means the caller did not speak the
//!   transport's dialect. MCP has its own input schema and reports its own.
//! - The `payload_too_large` (`413`) of the **HTTP request body limit**
//!   ([`AppState::message_body_limit`] plus `DefaultBodyLimit::disable()`, which
//!   hands the size check to the send handler so it answers with this JSON
//!   envelope). That limit exists only because an HTTP request has a body; an
//!   MCP call has none and can never produce this rejection.
//!
//! ## What the service owns, and why
//!
//! Everything else — the `404`s (`mailbox_not_found`, `message_not_found`,
//! `attachment_not_found`), the `413` of the **decoded** attachments
//! (`message_too_large`), `422` (`message_not_parsable`), both `501`s
//! (`capability_not_supported`, `idle_not_configured`), `502` (`smtp_error`) and
//! `503` (`imap_unavailable`) — depends on the mail account and on the server,
//! not on how the caller was written. It is a domain outcome, so it is produced
//! once, here.
//!
//! ## Codes with two producers
//!
//! `invalid_request` and `payload_too_large` are produced by **both** layers,
//! and that is deliberate: the transport produces the ones that only exist in a
//! transport (a malformed body or query string; the raw HTTP body limit) while
//! the service produces the **domain** ones, so that an operation never
//! re-implements a rule a second transport would have to re-implement as well.
//! [`ServiceError`] gains its `InvalidRequest` variant with the first operation
//! that needs it.
//!
//! This is a conscious deviation from the audit's recommendation that the
//! service only ever receive *already validated* types and that
//! `invalid_request` stay entirely in the transport: the **domain rules** — a
//! non-empty mailbox, `uid != 0`, the total decoded attachment size, the set of
//! known flags — are validated **here**, precisely so MCP inherits them instead
//! of reproducing them.
//!
//! The `message` of a domain failure is a `&'static str` from the service's own
//! vocabulary: never text taken from the request, never a value read from the
//! configuration.
//!
//! Two rows share the code `imap_unavailable` but not the same body: the 503 of
//! `GET /api/imap/status` also carries `"connected": false`, which is that
//! route's own rendering, so it stays in the transport.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::Config;
use crate::config::TlsMode;
use crate::idle::{
    HttpWebhookSender, IdleConnector, IdleSettings, IdleStatus, IdleSupervisor, TokioIdleConnector,
    WebhookSender,
};
use crate::imap::{
    Address, Backoff, ConnectionManager, FetchFormat, ImapConnector, ImapError, MailboxInfo,
    Message, MessageEnvelope, ParsedMessageError, SearchCriteria, TokioImapConnector,
};
use crate::mime::ParsedAttachment;
use crate::queue::{NotificationQueue, QueueError, QueueStats, WebhookQueue};
use crate::smtp::{MailSender, SmtpError, SmtpSender};

/// Application name, resolved from `Cargo.toml`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
/// Application version, resolved from `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Slack added to the computed body limit, on top of the base64 expansion of the
/// attachment bytes, to cover the rest of the JSON envelope.
const BODY_LIMIT_SLACK: usize = 64 * 1024;

/// Errors produced while building [`AppState`].
///
/// It wraps the service constructors and the delivery queue so a single
/// `Result` covers the real SMTP transport, the IMAP connector and the queue
/// file, none of which opens a connection or performs I/O beyond the queue file
/// at construction time.
#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    /// Building the SMTP transport failed.
    #[error(transparent)]
    Smtp(#[from] SmtpError),
    /// Building the IMAP connector failed.
    #[error(transparent)]
    Imap(#[from] ImapError),
    /// Building the delivery queue failed (an unusable `APIMAIL_QUEUE_PATH`).
    #[error(transparent)]
    Queue(#[from] QueueError),
}

impl AppStateError {
    /// Stable, log-safe description; delegates to the wrapped error's public
    /// message.
    ///
    /// The wrapped [`SmtpError`]/[`ImapError`] already group their variants into
    /// stable, credential-free families, so this never propagates the raw
    /// [`Display`](std::fmt::Display) text produced by `lettre`, `async-imap`,
    /// rustls or the operating system. A queue failure names the variable but
    /// never the path.
    pub fn public_message(&self) -> String {
        match self {
            Self::Smtp(error) => error.public_message().to_string(),
            Self::Imap(error) => error.public_message().to_string(),
            Self::Queue(_) => {
                "invalid APIMAIL_QUEUE_PATH: the queue file could not be created or written"
                    .to_string()
            }
        }
    }

    /// Stable machine-filterable tag, for **logs only**.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Smtp(_) => "smtp",
            Self::Imap(_) => "imap",
            Self::Queue(_) => "queue",
        }
    }
}

/// Shared state exposed to the operations of this module and to the transports
/// that call them.
///
/// The configured API key is stored only as its SHA-256 digest and the field is
/// private, so the plaintext secret can neither be read nor formatted:
/// `AppState::verify_token` is the only way to compare a token against it. The
/// manual [`Debug`] implementation redacts the digest, the default sender, the
/// mailer and the connection manager, which hold the SMTP and IMAP credentials.
#[derive(Clone)]
pub struct AppState {
    /// Application name.
    pub name: &'static str,
    /// Application version.
    pub version: &'static str,
    /// SHA-256 digest of the expected API key.
    ///
    /// Private, and no further: the comparison against an incoming token happens
    /// in [`AppState::verify_token`], so no other module needs to reach it.
    api_key_hash: [u8; 32],
    /// Non-sensitive view of the configured mail account (host and port only).
    account: AccountView,
    /// Outgoing mail transport (real at runtime, fake in tests).
    ///
    /// Crate-visible **while the handlers of `src/http.rs` are not yet
    /// migrated**; it goes back to private when the last one is. This is a
    /// migration artefact, **not** the target pattern: operations must reach the
    /// transport through a method of this type, the way [`AppState::idle`] does.
    pub(crate) mailer: Arc<dyn MailSender>,
    /// Maximum total size, in bytes, of the decoded attachments of a message.
    max_attachment_bytes: usize,
    /// Maximum size, in bytes, of a message that will be fetched and parsed.
    max_message_bytes: usize,
    /// Sender used when a message omits `from` (the configured SMTP user).
    ///
    /// Crate-visible **while the handlers of `src/http.rs` are not yet
    /// migrated**; it goes back to private when the last one is. This is a
    /// migration artefact, **not** the target pattern.
    pub(crate) default_from: String,
    /// Lazy, persistent IMAP connection manager.
    ///
    /// Crate-visible **while the handlers of `src/http.rs` are not yet
    /// migrated**; it goes back to private when the last one is. This is a
    /// migration artefact, **not** the target pattern.
    pub(crate) imap: Arc<ConnectionManager>,
    /// TLS mode of the configured IMAP endpoint, for the status endpoint.
    imap_tls: TlsMode,
    /// IDLE subscription supervisor (dedicated connection + webhook).
    idle: Arc<IdleSupervisor>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("name", &self.name)
            .field("version", &self.version)
            .field("api_key_hash", &"***")
            .field("account", &self.account)
            .field("mailer", &"***")
            .field("max_attachment_bytes", &self.max_attachment_bytes)
            .field("max_message_bytes", &self.max_message_bytes)
            .field("default_from", &"***")
            .field("imap", &"***")
            .field("imap_tls", &self.imap_tls)
            .field("idle", &"***")
            .finish()
    }
}

/// Builds the delivery queue from `config`, failing closed.
///
/// With `APIMAIL_QUEUE_PATH` set the queue is persistent: any I/O failure (a
/// path that cannot be created, opened or written) is reported as
/// [`QueueError`] and aborts startup, naming the variable. Without a path the
/// queue lives in memory only and nothing is ever written to disk. Either way
/// the concrete [`WebhookQueue`] is erased behind the [`NotificationQueue`]
/// trait the supervisor and worker depend on.
fn build_queue(config: &Config) -> Result<Arc<dyn NotificationQueue>, QueueError> {
    let limits = config.queue_limits();
    match &config.queue_path {
        Some(path) => Ok(Arc::new(WebhookQueue::persistent(path.clone(), limits)?)),
        None => Ok(Arc::new(WebhookQueue::in_memory(limits))),
    }
}

impl AppState {
    /// Builds the application state from the loaded [`Config`].
    ///
    /// This constructs the real SMTP transport, the real IMAP connector and the
    /// delivery queue from `config` — it opens no connection, but building any of
    /// them (resolving a TLS configuration, or creating the queue file) can fail,
    /// hence the `Result`.
    ///
    /// The expected key is hashed to a fixed-size digest here (not on every
    /// request) so the middleware can compare two equal-length buffers in
    /// constant time. [`Config`] guarantees a non-empty key; the assertion below
    /// documents that invariant in debug builds.
    pub fn from_config(config: &Config) -> Result<Self, AppStateError> {
        debug_assert!(
            !config.api_key.is_empty(),
            "AppState must not be built with an empty API key"
        );
        let mailer: Arc<dyn MailSender> =
            Arc::new(SmtpSender::from_endpoint(&config.account.smtp)?);
        let connector: Arc<dyn ImapConnector> = Arc::new(TokioImapConnector::from_config(config)?);
        Self::with_services(config, mailer, connector)
    }

    /// Builds the application state with an injected [`MailSender`].
    ///
    /// Used by tests to avoid the network. It delegates to
    /// [`with_services`](Self::with_services) with a real [`TokioImapConnector`]
    /// built from `config` (which opens no connection). The connector can still
    /// fail to build (for example, resolving its TLS configuration), so the
    /// fallible construction is propagated instead of panicking.
    pub fn with_mailer(
        config: &Config,
        mailer: Arc<dyn MailSender>,
    ) -> Result<Self, AppStateError> {
        let connector: Arc<dyn ImapConnector> = Arc::new(TokioImapConnector::from_config(config)?);
        Self::with_services(config, mailer, connector)
    }

    /// Builds the application state with both services injected.
    ///
    /// Used by tests to exercise the HTTP layer without touching the network. It
    /// delegates to [`with_idle`](Self::with_idle) with a real
    /// [`TokioIdleConnector`] and an [`HttpWebhookSender`], both of which are
    /// infallible to build (their TLS/HTTP clients are resolved lazily). The
    /// delivery queue is built from `config` and can fail closed when
    /// `APIMAIL_QUEUE_PATH` is unusable, hence the `Result`.
    pub fn with_services(
        config: &Config,
        mailer: Arc<dyn MailSender>,
        imap_connector: Arc<dyn ImapConnector>,
    ) -> Result<Self, AppStateError> {
        let idle_connector: Arc<dyn IdleConnector> = Arc::new(TokioIdleConnector::new(
            config.account.imap.clone(),
            config.imap_timeout,
        ));
        let webhook_sender: Arc<dyn WebhookSender> =
            Arc::new(HttpWebhookSender::new(config.webhook_timeout));
        Self::with_idle(
            config,
            mailer,
            imap_connector,
            idle_connector,
            webhook_sender,
        )
    }

    /// Builds the application state with every service injected.
    ///
    /// Used by tests to exercise the HTTP layer, including the IDLE routes,
    /// without touching the network: the dedicated-connection factory and the
    /// webhook client are supplied by the caller. The subscription is built from
    /// `config` (`APIMAIL_WEBHOOK_URL`, `APIMAIL_IDLE_MAILBOX`,
    /// `APIMAIL_WEBHOOK_TIMEOUT_SECS` and `APIMAIL_MAX_MESSAGE_BYTES`) but is not
    /// started until `POST /api/idle/start`.
    ///
    /// The delivery queue is built from `config` with `build_queue`, so an
    /// unusable `APIMAIL_QUEUE_PATH` fails the whole construction instead of
    /// silently degrading to an in-memory queue.
    pub fn with_idle(
        config: &Config,
        mailer: Arc<dyn MailSender>,
        imap_connector: Arc<dyn ImapConnector>,
        idle_connector: Arc<dyn IdleConnector>,
        webhook_sender: Arc<dyn WebhookSender>,
    ) -> Result<Self, AppStateError> {
        let queue = build_queue(config)?;
        let idle = Arc::new(IdleSupervisor::new(
            idle_connector,
            webhook_sender,
            IdleSettings {
                mailbox: config.idle_mailbox.clone(),
                webhook_url: config.webhook_url.clone(),
                webhook_timeout: config.webhook_timeout,
                max_message_bytes: config.max_message_bytes,
            },
            Backoff::default(),
            queue,
        ));
        Ok(Self {
            name: APP_NAME,
            version: APP_VERSION,
            api_key_hash: Sha256::digest(config.api_key.as_bytes()).into(),
            account: AccountView {
                imap: EndpointView {
                    host: config.account.imap.host.clone(),
                    port: config.account.imap.port,
                },
                smtp: EndpointView {
                    host: config.account.smtp.host.clone(),
                    port: config.account.smtp.port,
                },
            },
            mailer,
            max_attachment_bytes: config.max_attachment_bytes,
            max_message_bytes: config.max_message_bytes,
            default_from: config.account.smtp.username.clone(),
            imap: Arc::new(ConnectionManager::new(imap_connector, Backoff::default())),
            imap_tls: config.account.imap.tls,
            idle,
        })
    }

    /// Whether `token` is the configured API key.
    ///
    /// The token is hashed to a fixed-size digest and compared against the
    /// expected digest with `subtle`'s constant-time equality, so the secret is
    /// never compared in plaintext and its length cannot leak through timing.
    /// Because both operands always have the same length, the only variable cost
    /// is hashing the token, which depends solely on input the caller already
    /// controls.
    ///
    /// Parsing the `Authorization` header — the scheme, the whitespace — stays
    /// in the transport, which is the layer that speaks that dialect: this method
    /// takes the already-extracted token, so every transport (HTTP today, MCP
    /// tomorrow) authenticates against the very same digest.
    pub(crate) fn verify_token(&self, token: &str) -> bool {
        let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        candidate.ct_eq(&self.api_key_hash).into()
    }

    /// The IDLE subscription supervisor, controlled by the `/api/idle/*` routes.
    pub fn idle(&self) -> &IdleSupervisor {
        &self.idle
    }

    /// Maximum total size, in bytes, of the decoded attachments of a message.
    pub fn max_attachment_bytes(&self) -> usize {
        self.max_attachment_bytes
    }

    /// Maximum size, in bytes, of a message that will be fetched and parsed.
    ///
    /// Values reported or fetched above this bound are rejected before any MIME
    /// parsing, so untrusted content cannot exhaust memory.
    pub fn max_message_bytes(&self) -> usize {
        self.max_message_bytes
    }

    /// Upper bound for the raw JSON request body of `POST /api/messages`.
    ///
    /// The body carries base64-encoded attachments, which expand the raw bytes
    /// by a factor of ≈ 4/3; `BODY_LIMIT_SLACK` covers the rest of the JSON
    /// envelope. All arithmetic saturates, so an extreme configured limit cannot
    /// overflow.
    pub fn message_body_limit(&self) -> usize {
        self.max_attachment_bytes
            .saturating_mul(4)
            .saturating_div(3)
            .saturating_add(BODY_LIMIT_SLACK)
    }
}

/// Public view of a single mail endpoint: host and port only.
#[derive(Debug, Clone, Serialize)]
pub struct EndpointView {
    /// Server host name or address.
    pub host: String,
    /// Server port.
    pub port: u16,
}

/// Public view of the mail account, exposed by `GET /api/account`.
///
/// It intentionally carries neither the username nor the password.
#[derive(Debug, Clone, Serialize)]
pub struct AccountView {
    /// Incoming (IMAP) endpoint.
    pub imap: EndpointView,
    /// Outgoing (SMTP) endpoint.
    pub smtp: EndpointView,
}

/// JSON payload returned by `GET /api/whoami`.
#[derive(Debug, Serialize)]
pub struct WhoamiResponse {
    /// Always `true`: reached only after successful authentication.
    pub authenticated: bool,
}

/// JSON payload returned by a successful `GET /api/imap/status`.
#[derive(Debug, Serialize)]
pub struct ImapStatusResponse {
    /// Always `true`: reached only after a live session was ensured.
    pub connected: bool,
    /// IMAP server host.
    pub host: String,
    /// IMAP server port.
    pub port: u16,
    /// Configured TLS mode label.
    pub tls: &'static str,
}

/// One mailbox in the `GET /api/mailboxes` response.
///
/// The field order is the declaration order, which is the serialisation order of
/// the published body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct MailboxDto {
    /// Mailbox name.
    pub name: String,
    /// Hierarchy delimiter, or `null` for a flat namespace.
    pub delimiter: Option<String>,
    /// Mailbox attributes in IMAP style (`\NoSelect`, …).
    pub attributes: Vec<String>,
}

impl From<MailboxInfo> for MailboxDto {
    fn from(info: MailboxInfo) -> Self {
        Self {
            name: info.name,
            delimiter: info.delimiter,
            attributes: info.attributes,
        }
    }
}

/// JSON payload returned by a successful `GET /api/mailboxes`.
///
/// The field order is the declaration order, which is the serialisation order of
/// the published body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct MailboxListResponse {
    /// The account's mailboxes.
    pub mailboxes: Vec<MailboxDto>,
}

/// One address in a message envelope.
///
/// The field order is the declaration order, which is the serialisation order of
/// the published body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct AddressDto {
    /// Display name, or `null`.
    pub name: Option<String>,
    /// `mailbox@host`, or `null`.
    pub address: Option<String>,
}

impl From<&Address> for AddressDto {
    fn from(address: &Address) -> Self {
        Self {
            name: address.name.clone(),
            address: address.address.clone(),
        }
    }
}

/// A message envelope in the message responses.
///
/// The field order is the declaration order, which is the serialisation order of
/// the published body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct EnvelopeDto {
    /// `From` addresses.
    pub from: Vec<AddressDto>,
    /// `To` addresses.
    pub to: Vec<AddressDto>,
    /// `Cc` addresses.
    pub cc: Vec<AddressDto>,
    /// Raw `Subject` header.
    pub subject: Option<String>,
    /// Raw `Date` header.
    pub date: Option<String>,
    /// `Message-ID`.
    pub message_id: Option<String>,
}

impl From<&MessageEnvelope> for EnvelopeDto {
    fn from(envelope: &MessageEnvelope) -> Self {
        Self {
            from: envelope.from.iter().map(AddressDto::from).collect(),
            to: envelope.to.iter().map(AddressDto::from).collect(),
            cc: envelope.cc.iter().map(AddressDto::from).collect(),
            subject: envelope.subject.clone(),
            date: envelope.date.clone(),
            message_id: envelope.message_id.clone(),
        }
    }
}

/// The metadata shared by the listing and the single-message responses.
///
/// The field order is the declaration order, which is the serialisation order of
/// the published body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct MessageDto {
    /// Unique identifier within the mailbox.
    pub uid: u32,
    /// Sequence number within the mailbox.
    pub seq: u32,
    /// Flags in IMAP style (`\Seen`, …).
    pub flags: Vec<String>,
    /// `RFC822.SIZE`, if reported.
    pub size: Option<u32>,
    /// `INTERNALDATE` in RFC 3339 form, if reported.
    pub internal_date: Option<String>,
    /// Parsed envelope, if reported.
    pub envelope: Option<EnvelopeDto>,
}

impl From<&Message> for MessageDto {
    fn from(message: &Message) -> Self {
        Self {
            uid: message.uid,
            seq: message.seq,
            flags: message.flags.clone(),
            size: message.size,
            internal_date: message.internal_date.clone(),
            envelope: message.envelope.as_ref().map(EnvelopeDto::from),
        }
    }
}

/// JSON payload returned by a successful `GET /api/messages`.
///
/// The field order is the declaration order, which is the serialisation order of
/// the published body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct MessageListResponse {
    /// Selected mailbox name.
    pub mailbox: String,
    /// Total number of messages matching the search.
    pub total: u32,
    /// Applied page size.
    pub limit: u32,
    /// Applied page offset.
    pub offset: u32,
    /// The messages in this page.
    pub messages: Vec<MessageDto>,
}

/// JSON payload returned by a successful `GET /api/messages/{uid}`.
///
/// The `headers_base64` and `raw_base64` fields are present only for the matching
/// `format`, hence the conditional serialization. The field order is the
/// declaration order, which is the serialisation order of the published body, so
/// it is part of the contract.
#[derive(Debug, Serialize)]
pub struct MessageDetailResponse {
    /// Selected mailbox name.
    pub mailbox: String,
    /// Unique identifier within the mailbox.
    pub uid: u32,
    /// Sequence number within the mailbox.
    pub seq: u32,
    /// Flags in IMAP style (`\Seen`, …).
    pub flags: Vec<String>,
    /// `RFC822.SIZE`, if reported.
    pub size: Option<u32>,
    /// `INTERNALDATE` in RFC 3339 form, if reported.
    pub internal_date: Option<String>,
    /// Parsed envelope, if reported.
    pub envelope: Option<EnvelopeDto>,
    /// Requested format label: `summary`, `headers` or `full`.
    pub format: &'static str,
    /// Raw header block, base64-encoded (only for `format=headers`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_base64: Option<String>,
    /// Full RFC822 source, base64-encoded (only for `format=full`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_base64: Option<String>,
}

impl MessageDetailResponse {
    /// Builds the response from the fetched message and the requested format.
    ///
    /// The raw bytes come from the domain message (`headers`/`body`); the fake
    /// sessions in tests populate them directly.
    fn new(mailbox: String, message: Message, format: FetchFormat) -> Self {
        let metadata = MessageDto::from(&message);
        let (format_label, headers_base64, raw_base64) = match format {
            FetchFormat::Summary => ("summary", None, None),
            FetchFormat::Headers => (
                "headers",
                message
                    .headers
                    .as_deref()
                    .map(|bytes| STANDARD.encode(bytes)),
                None,
            ),
            FetchFormat::Full => (
                "full",
                None,
                message.body.as_deref().map(|bytes| STANDARD.encode(bytes)),
            ),
        };
        Self {
            mailbox,
            uid: metadata.uid,
            seq: metadata.seq,
            flags: metadata.flags,
            size: metadata.size,
            internal_date: metadata.internal_date,
            envelope: metadata.envelope,
            format: format_label,
            headers_base64,
            raw_base64,
        }
    }
}

/// JSON payload returned by a successful `GET /api/messages/{uid}/body`.
///
/// `text` and `html` are always present; each is `null` when the message carries
/// no such body and none can be derived. The field order is the declaration
/// order, which is the serialisation order of the published body, so it is part
/// of the contract.
#[derive(Debug, Serialize)]
pub struct MessageBodyResponse {
    /// Selected mailbox name.
    pub mailbox: String,
    /// Unique identifier within the mailbox.
    pub uid: u32,
    /// Derived plain-text body, or `null`.
    pub text: Option<String>,
    /// Decoded HTML body, or `null`.
    pub html: Option<String>,
}

/// One attachment in the `GET /api/messages/{uid}/attachments` response.
///
/// The content is intentionally omitted: it is only ever returned by the
/// single-attachment route, base64-encoded inside the JSON body. The field order
/// is the declaration order, which is the serialisation order of the published
/// body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct ParsedAttachmentDto {
    /// Positional identifier within the parsed attachment list.
    pub id: u32,
    /// Decoded file name, or `null`.
    pub filename: Option<String>,
    /// MIME content type, or `null`.
    pub content_type: Option<String>,
    /// Decoded content length, in bytes.
    pub size: usize,
    /// Whether the part is marked `inline`.
    pub inline: bool,
    /// `Content-ID`, or `null`.
    pub content_id: Option<String>,
}

impl From<&ParsedAttachment> for ParsedAttachmentDto {
    fn from(attachment: &ParsedAttachment) -> Self {
        Self {
            id: attachment.id,
            filename: attachment.filename.clone(),
            content_type: attachment.content_type.clone(),
            size: attachment.size,
            inline: attachment.inline,
            content_id: attachment.content_id.clone(),
        }
    }
}

/// JSON payload returned by a successful `GET /api/messages/{uid}/attachments`.
///
/// The field order is the declaration order, which is the serialisation order of
/// the published body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct AttachmentListResponse {
    /// Selected mailbox name.
    pub mailbox: String,
    /// Unique identifier within the mailbox.
    pub uid: u32,
    /// The message's attachments, without their content.
    pub attachments: Vec<ParsedAttachmentDto>,
}

/// JSON payload returned by a successful
/// `GET /api/messages/{uid}/attachments/{id}`.
///
/// The field order is the declaration order, which is the serialisation order of
/// the published body, so it is part of the contract.
#[derive(Debug, Serialize)]
pub struct AttachmentResponse {
    /// Selected mailbox name.
    pub mailbox: String,
    /// Unique identifier within the mailbox.
    pub uid: u32,
    /// Positional identifier of the attachment.
    pub id: u32,
    /// Decoded file name, or `null`.
    pub filename: Option<String>,
    /// MIME content type, or `null`.
    pub content_type: Option<String>,
    /// Decoded content length, in bytes.
    pub size: usize,
    /// The attachment's decoded content, base64-encoded.
    pub content_base64: String,
}

/// Failure of an application operation.
///
/// The variant decides the stable `error` code (via [`kind`](Self::kind)) and
/// the message a transport may expose (via
/// [`public_message`](Self::public_message)); the status code is a transport
/// decision and is **not** modelled here, so the same failure can be rendered as
/// an HTTP response or as an MCP tool error.
///
/// The accessor names follow the crate-wide convention established by
/// [`AppStateError`], [`ConfigError`](crate::config::ConfigError) and
/// [`AccountError`](crate::config::AccountError): `public_message()` plus
/// `kind()`, never `code()`.
///
/// The enum is `#[non_exhaustive]` because it is public, it is published on
/// crates.io and it is meant to grow: a new variant must never be a breaking
/// change for a downstream consumer. Inside the crate the transports still match
/// it **exhaustively** (no wildcard arm), so a new variant is a compile error
/// until it is mapped, and no failure mode can ever be silently rendered with
/// the wrong status.
///
/// Today this enum holds the variants the operations owned by this module can
/// produce: `whoami` and `account` cannot fail, `imap_status` can only fail with
/// `imap_unavailable`, and the seven read operations of the third migration wave
/// add the two `SELECT`-driven `404`s, the message `404`s, the two MIME-parsing
/// refusals and the `501` of a server that does not announce the extension an
/// operation needs. The remaining rows of the module's error table are
/// added, one operation at a time, as their handlers migrate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ServiceError {
    /// The IMAP session or server is unavailable (`503`, `imap_unavailable`).
    ///
    /// The message is a `&'static str` of this module's own vocabulary, and the
    /// underlying failure is kept as the [`source`](std::error::Error::source)
    /// for logs only: it may embed hosts, certificates or addresses, so it is
    /// never part of what a caller reads.
    #[error("{message}")]
    ImapUnavailable {
        /// Credential-free, stable message of the underlying IMAP failure.
        message: &'static str,
        /// The IMAP failure this response describes.
        #[source]
        source: ImapError,
    },
    /// The server does not announce the IMAP extension the operation needs
    /// (`501`, `capability_not_supported`).
    ///
    /// Same shape as [`ImapUnavailable`](Self::ImapUnavailable): the message is a
    /// `&'static str` of this module's own vocabulary, derived from the cause's
    /// [`ImapError::public_message`], and the underlying failure is kept as the
    /// [`source`](std::error::Error::source) for logs only, so nothing the server
    /// said ever becomes caller-visible text.
    #[error("{message}")]
    CapabilityNotSupported {
        /// Credential-free, stable message for a missing server extension.
        message: &'static str,
        /// The IMAP failure this response describes.
        #[source]
        source: ImapError,
    },
    /// The requested mailbox does not exist (`404`, `mailbox_not_found`).
    ///
    /// Produced by `SELECT` alone: the server answered `NO` for the mailbox.
    #[error("mailbox not found")]
    MailboxNotFound,
    /// The requested message does not exist in the selected mailbox (`404`,
    /// `message_not_found`).
    ///
    /// Produced by the message operations when the requested `uid` is absent: the
    /// session reports it before (or instead of) handing back a message.
    #[error("message not found")]
    MessageNotFound,
    /// The requested attachment identifier matches no attachment (`404`,
    /// `attachment_not_found`).
    ///
    /// Produced by the single-attachment operation alone, from the positional
    /// identifier of the parsed message.
    #[error("attachment not found")]
    AttachmentNotFound,
    /// The message exceeds the configured parsing size limit (`413`,
    /// `message_too_large`).
    ///
    /// The refusal is made deliberately before the MIME content is parsed; the
    /// refusal itself (which carries the reported and the configured size) is kept
    /// as the [`source`](std::error::Error::source) for logs only, and never
    /// reaches a caller.
    #[error("message exceeds the configured size limit")]
    MessageTooLarge {
        /// The refusal produced by the size guard.
        #[source]
        source: ParsedMessageError,
    },
    /// The message exists but its MIME content cannot be parsed (`422`,
    /// `message_not_parsable`).
    ///
    /// The parser refusal is kept as the [`source`](std::error::Error::source) for
    /// logs only: it may carry the raw content or a third-party description, so it
    /// is never part of what a caller reads.
    #[error("message cannot be parsed")]
    MessageNotParsable {
        /// The refusal produced by the MIME parser.
        #[source]
        source: ParsedMessageError,
    },
}

impl ServiceError {
    /// Builds the `imap_unavailable` failure around the underlying [`ImapError`].
    ///
    /// Both fields are derived here, in one place, so the public message can
    /// never drift away from the cause it describes.
    pub(crate) fn imap_unavailable(source: ImapError) -> Self {
        Self::ImapUnavailable {
            message: source.public_message(),
            source,
        }
    }

    /// Builds the `capability_not_supported` failure around the underlying
    /// [`ImapError`].
    ///
    /// Both fields are derived here, in one place, so the `501` of a missing
    /// server extension can never be flattened into an availability failure and
    /// its public message can never drift away from the cause it describes.
    pub(crate) fn capability_not_supported(source: ImapError) -> Self {
        Self::CapabilityNotSupported {
            message: source.public_message(),
            source,
        }
    }

    /// Maps the failure surface of an operation that only issues `SELECT`.
    ///
    /// A missing mailbox is a domain `404`; a missing server extension keeps its
    /// own `501`; every other failure of the underlying session is an
    /// availability failure flattened into one code, so the categories can never
    /// be confused by a caller-written wildcard.
    fn select_failure(source: ImapError) -> Self {
        match source {
            ImapError::MailboxNotFound => Self::MailboxNotFound,
            ImapError::CapabilityNotSupported => Self::capability_not_supported(source),
            other => Self::imap_unavailable(other),
        }
    }

    /// Maps the failure surface of an operation that selects a mailbox and then
    /// addresses **one message** by `uid`.
    ///
    /// The two missing-object outcomes stay distinct — a missing mailbox is not a
    /// missing message —, a missing server extension keeps its own `501`, and
    /// everything else is an availability failure.
    fn message_failure(source: ImapError) -> Self {
        match source {
            ImapError::MailboxNotFound => Self::MailboxNotFound,
            ImapError::MessageNotFound => Self::MessageNotFound,
            ImapError::CapabilityNotSupported => Self::capability_not_supported(source),
            other => Self::imap_unavailable(other),
        }
    }

    /// Maps the failure surface of the parsed-message operations.
    ///
    /// The IMAP variant reuses [`message_failure`](Self::message_failure) so the
    /// two `404`s keep their own code; the size guard and the parser refusal keep
    /// their cause as the source, for logs only.
    fn parsed_failure(source: ParsedMessageError) -> Self {
        match source {
            ParsedMessageError::Imap(error) => Self::message_failure(error),
            ParsedMessageError::TooLarge { .. } => Self::MessageTooLarge { source },
            ParsedMessageError::Unparsable => Self::MessageNotParsable { source },
        }
    }

    /// Stable, machine-filterable `error` code of the shared envelope.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ImapUnavailable { .. } => "imap_unavailable",
            Self::CapabilityNotSupported { .. } => "capability_not_supported",
            Self::MailboxNotFound => "mailbox_not_found",
            Self::MessageNotFound => "message_not_found",
            Self::AttachmentNotFound => "attachment_not_found",
            Self::MessageTooLarge { .. } => "message_too_large",
            Self::MessageNotParsable { .. } => "message_not_parsable",
        }
    }

    /// Stable, credential-free description, safe to expose to a caller.
    ///
    /// It never propagates third-party text directly: the underlying
    /// [`ImapError::public_message`] already groups its variants into stable
    /// families, the MIME refusals are fixed strings, and the message is fixed at
    /// construction time.
    pub fn public_message(&self) -> &'static str {
        match self {
            Self::ImapUnavailable { message, .. } => message,
            Self::CapabilityNotSupported { message, .. } => message,
            Self::MailboxNotFound => "mailbox not found",
            Self::MessageNotFound => "message not found",
            Self::AttachmentNotFound => "attachment not found",
            Self::MessageTooLarge { .. } => "message exceeds the configured size limit",
            Self::MessageNotParsable { .. } => "message cannot be parsed",
        }
    }
}

/// Answers who the caller is: the sole fact this operation can assert is that
/// the caller was already authenticated.
///
/// Authentication itself is a *transport* concern — the HTTP API enforces it
/// with a middleware before the handler runs, and the MCP transports reuse that
/// same middleware — so this operation never sees a credential, which is what
/// makes it structurally unable to leak one.
pub fn whoami() -> WhoamiResponse {
    WhoamiResponse {
        authenticated: true,
    }
}

/// Reports the public view of the configured mail account.
///
/// Returns a borrow rather than a copy: the account is fixed at startup and
/// carries no secret, so there is nothing to re-derive per call.
pub fn account(state: &AppState) -> &AccountView {
    &state.account
}

/// Ensures a live IMAP session (connecting lazily on first use) and reports the
/// endpoint it talks to, never a credential.
///
/// # Errors
///
/// [`ServiceError::ImapUnavailable`] (`503`), carrying the credential-free
/// [`ImapError::public_message`] of the underlying failure and the failure
/// itself as its `source`.
pub async fn imap_status(state: &AppState) -> Result<ImapStatusResponse, ServiceError> {
    state
        .imap
        .status()
        .await
        .map_err(ServiceError::imap_unavailable)?;
    Ok(ImapStatusResponse {
        connected: true,
        host: state.account.imap.host.clone(),
        port: state.account.imap.port,
        tls: state.imap_tls.as_str(),
    })
}

/// Lists the account's mailboxes over the shared, lazily-established IMAP
/// session, never exposing a credential.
///
/// This operation only ever issues `LIST`, and [`ImapError::MailboxNotFound`] is
/// produced by `SELECT` alone, so no `404` can originate here. Its mapping is
/// nevertheless the one every `SELECT`-driven operation uses — a missing mailbox
/// is a `404`, a server that does not announce the extension is a `501`, and
/// everything else is an availability failure — so no code this route could one
/// day produce is flattened into a different one.
///
/// # Errors
///
/// [`ServiceError::MailboxNotFound`] (`404`) and
/// [`ServiceError::CapabilityNotSupported`] (`501`) if the session reports them,
/// and [`ServiceError::ImapUnavailable`] (`503`) otherwise, carrying the
/// credential-free [`ImapError::public_message`] of the underlying failure and
/// the failure itself as its `source`.
pub async fn list_mailboxes(state: &AppState) -> Result<MailboxListResponse, ServiceError> {
    let mailboxes = state
        .imap
        .list_mailboxes()
        .await
        .map_err(ServiceError::select_failure)?;
    Ok(MailboxListResponse {
        mailboxes: mailboxes.into_iter().map(MailboxDto::from).collect(),
    })
}

/// Reports the state of the IDLE subscription.
///
/// Infallible: the supervisor's snapshot is a plain read of its own task handles,
/// with no network, no credential and no failure mode. The DTO belongs to the
/// domain module ([`IdleStatus`], in [`crate::idle`]) and is returned verbatim, so
/// no transport can drift from it.
pub async fn idle_status(state: &AppState) -> IdleStatus {
    state.idle().status().await
}

/// Reports the delivery queue's observable state.
///
/// Infallible for the same reason as [`idle_status`]: [`QueueStats`] is an
/// in-memory snapshot of the queue (see [`crate::queue`]), taken without I/O even
/// when the queue is persistent. The DTO lives in the domain module and is
/// returned verbatim.
pub fn idle_queue(state: &AppState) -> QueueStats {
    state.idle().queue().stats()
}

/// Lists one page of the messages of `mailbox` matching `criteria`.
///
/// The operation composes `SELECT`, `SEARCH` and a summary `UID FETCH` on the
/// shared session, in that order and on a single lock.
///
/// This operation only ever issues `SELECT`, `SEARCH` and a summary `UID FETCH`,
/// and [`ImapError::MessageNotFound`] is produced by the single-message
/// operations alone, so no `404 message_not_found` can originate here: a missing
/// mailbox is the only domain `404`, and every other failure of the session is an
/// availability failure flattened into one code, exactly as the published error
/// table requires.
///
/// # Errors
///
/// - [`ServiceError::MailboxNotFound`] (`404`) when the server answers `NO` for
///   the mailbox.
/// - [`ServiceError::ImapUnavailable`] (`503`), carrying the credential-free
///   [`ImapError::public_message`] of the underlying failure and the failure
///   itself as its `source`.
pub async fn list_messages(
    state: &AppState,
    mailbox: String,
    criteria: SearchCriteria,
    limit: u32,
    offset: u32,
) -> Result<MessageListResponse, ServiceError> {
    let page = state
        .imap
        .list_messages(&mailbox, criteria, limit, offset)
        .await
        .map_err(ServiceError::select_failure)?;
    Ok(MessageListResponse {
        mailbox,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        messages: page.messages.iter().map(MessageDto::from).collect(),
    })
}

/// Fetches the single message `uid` of `mailbox` in the requested `format`.
///
/// `uid` is expected to be already validated as a positive identifier: a
/// non-numeric or zero `uid` is a request that does not speak the transport's
/// dialect and never reaches this operation.
///
/// # Errors
///
/// - [`ServiceError::MailboxNotFound`] (`404`) when the server answers `NO` for
///   the mailbox.
/// - [`ServiceError::MessageNotFound`] (`404`) when the mailbox holds no message
///   with that `uid`.
/// - [`ServiceError::ImapUnavailable`] (`503`), carrying the credential-free
///   [`ImapError::public_message`] of the underlying failure and the failure
///   itself as its `source`.
pub async fn fetch_message(
    state: &AppState,
    mailbox: String,
    uid: u32,
    format: FetchFormat,
) -> Result<MessageDetailResponse, ServiceError> {
    let message = state
        .imap
        .fetch_message(&mailbox, uid, format)
        .await
        .map_err(ServiceError::message_failure)?;
    Ok(MessageDetailResponse::new(mailbox, message, format))
}

/// Fetches the single message `uid`, parses its MIME content and reports its
/// plain-text and HTML bodies.
///
/// The raw source is bounded by [`AppState::max_message_bytes`] and is parsed off
/// the session lock (see `fetch_parsed`), so an oversized or hostile message
/// neither exhausts memory nor stalls another request.
///
/// # Errors
///
/// - [`ServiceError::MailboxNotFound`] (`404`) when the server answers `NO` for
///   the mailbox.
/// - [`ServiceError::MessageNotFound`] (`404`) when the mailbox holds no message
///   with that `uid`.
/// - [`ServiceError::MessageTooLarge`] (`413`) when the message exceeds the
///   configured parsing size limit; the content is never parsed.
/// - [`ServiceError::MessageNotParsable`] (`422`) when the message exists but its
///   MIME content cannot be parsed.
/// - [`ServiceError::ImapUnavailable`] (`503`) for every other session failure.
pub async fn message_body(
    state: &AppState,
    mailbox: String,
    uid: u32,
) -> Result<MessageBodyResponse, ServiceError> {
    let parsed = state
        .imap
        .fetch_parsed(&mailbox, uid, state.max_message_bytes())
        .await
        .map_err(ServiceError::parsed_failure)?;
    Ok(MessageBodyResponse {
        mailbox,
        uid,
        text: parsed.text,
        html: parsed.html,
    })
}

/// Fetches the single message `uid`, parses it and reports the metadata of each
/// attachment, **without** their content.
///
/// # Errors
///
/// Same failure surface as [`message_body`]: the two `404`s, `413`, `422` and the
/// `503` availability failure.
pub async fn list_attachments(
    state: &AppState,
    mailbox: String,
    uid: u32,
) -> Result<AttachmentListResponse, ServiceError> {
    let parsed = state
        .imap
        .fetch_parsed(&mailbox, uid, state.max_message_bytes())
        .await
        .map_err(ServiceError::parsed_failure)?;
    Ok(AttachmentListResponse {
        mailbox,
        uid,
        attachments: parsed
            .attachments
            .iter()
            .map(ParsedAttachmentDto::from)
            .collect(),
    })
}

/// Fetches the single message `uid`, parses it and reports the decoded content of
/// the attachment identified by the positional `id`, base64-encoded.
///
/// The content is encoded inside the JSON body and never reflected into a header,
/// so a hostile `filename` or `content_type` cannot inject one.
///
/// # Errors
///
/// The same failure surface as [`list_attachments`], plus
/// [`ServiceError::AttachmentNotFound`] (`404`) when `id` matches no attachment of
/// the parsed message.
pub async fn attachment(
    state: &AppState,
    mailbox: String,
    uid: u32,
    id: u32,
) -> Result<AttachmentResponse, ServiceError> {
    let parsed = state
        .imap
        .fetch_parsed(&mailbox, uid, state.max_message_bytes())
        .await
        .map_err(ServiceError::parsed_failure)?;
    let attachment = parsed
        .attachment(id)
        .ok_or(ServiceError::AttachmentNotFound)?;

    Ok(AttachmentResponse {
        mailbox,
        uid,
        id,
        filename: attachment.filename.clone(),
        content_type: attachment.content_type.clone(),
        size: attachment.size,
        content_base64: STANDARD.encode(&attachment.content),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Third-party text that must never reach a caller: a host, an address and
    /// a credential, as they appear in the strings `async-imap`, rustls and the
    /// operating system hand back.
    const THIRD_PARTY_TEXT: &str = "user@internal.example at 10.0.0.7:993";

    /// Every `ImapError` family an unavailable session can report, paired with
    /// the service message it must produce.
    ///
    /// The service flattens them all into one code, so what this assertion pins
    /// is the *mapping*: `imap_unavailable` plus the credential-free message of
    /// the family the cause belongs to.
    fn cases() -> [(ImapError, &'static str); 4] {
        [
            (
                ImapError::Unavailable(THIRD_PARTY_TEXT.to_string()),
                "could not connect to the IMAP server",
            ),
            (
                ImapError::Tcp(std::io::Error::other(THIRD_PARTY_TEXT)),
                "could not connect to the IMAP server",
            ),
            (
                ImapError::Tls(THIRD_PARTY_TEXT.to_string()),
                "IMAP TLS handshake failed",
            ),
            (
                ImapError::Login(THIRD_PARTY_TEXT.to_string()),
                "IMAP authentication or command failed",
            ),
        ]
    }

    #[test]
    fn imap_unavailable_kind_is_stable_and_message_matches_the_cause() {
        for (cause, expected) in cases() {
            let error = ServiceError::imap_unavailable(cause);

            assert_eq!(error.kind(), "imap_unavailable");
            assert!(
                !error.public_message().is_empty(),
                "the public message must never be empty"
            );
            assert_eq!(
                error.public_message(),
                expected,
                "the message must be the one of the cause's family"
            );
        }
    }

    #[test]
    fn imap_unavailable_never_leaks_third_party_text() {
        for (cause, expected) in cases() {
            let error = ServiceError::imap_unavailable(cause);

            assert_eq!(error.public_message(), expected);
            assert!(
                !error.public_message().contains("internal.example"),
                "the public message leaked a host name: {}",
                error.public_message()
            );
            assert!(!error.public_message().contains("10.0.0.7"));
            assert!(!error.public_message().contains("user@"));
            // `Display` is what a `tracing::error!(%error)` prints, so it must
            // be as safe as `public_message`.
            assert_eq!(error.to_string(), error.public_message());
        }
    }

    #[test]
    fn imap_unavailable_keeps_the_cause_reachable_for_logs() {
        let error =
            ServiceError::imap_unavailable(ImapError::Unavailable(THIRD_PARTY_TEXT.to_string()));

        let source = std::error::Error::source(&error).expect("the cause must be kept");
        assert!(
            source.to_string().contains(THIRD_PARTY_TEXT),
            "the source must be the original failure, not a rewrite: {source}"
        );
        assert!(
            !error.public_message().contains(THIRD_PARTY_TEXT),
            "the cause must stay out of the public message"
        );
    }

    /// A string taken from a request must never become a public message: the
    /// vocabulary is the service's own, fixed at construction time.
    #[test]
    fn imap_unavailable_message_is_static_domain_vocabulary() {
        let error =
            ServiceError::imap_unavailable(ImapError::Login("attacker-controlled".to_string()));

        let message: &'static str = error.public_message();
        assert_eq!(message, "IMAP authentication or command failed");
    }

    /// The `501` keeps the code, the message and the shape of
    /// [`ImapUnavailable`](ServiceError::ImapUnavailable): a fixed message of this
    /// module's vocabulary, derived from the cause in one place, with the cause
    /// itself reachable as the `source` for logs.
    #[test]
    fn capability_not_supported_kind_is_stable_and_message_is_domain_vocabulary() {
        let error = ServiceError::capability_not_supported(ImapError::CapabilityNotSupported);

        assert_eq!(error.kind(), "capability_not_supported");
        let message: &'static str = error.public_message();
        assert_eq!(message, "the server does not support this operation");
        assert_eq!(
            error.to_string(),
            message,
            "`Display`, what `tracing::error!(%error)` prints, must say exactly the same"
        );

        let source = std::error::Error::source(&error).expect("the cause must be kept");
        let cause = source
            .downcast_ref::<ImapError>()
            .expect("the source must be the original IMAP failure, not a rewrite");
        assert_eq!(cause.kind(), "unsupported", "for logs only");
    }

    /// The cause of the `501` never reaches the caller: the public message is the
    /// service's own fixed vocabulary, decided at construction time, so no text
    /// of the underlying failure can leak through it.
    #[test]
    fn capability_not_supported_keeps_the_cause_out_of_the_public_message() {
        let error = ServiceError::capability_not_supported(ImapError::CapabilityNotSupported);

        assert_eq!(
            error.public_message(),
            "the server does not support this operation",
            "the message is the fixed, credential-free one of the cause's family"
        );
        assert!(
            !error.public_message().contains(THIRD_PARTY_TEXT),
            "the public message must never carry third-party text"
        );
    }

    /// Every fixed-message variant, paired with its published `error` code and the
    /// message the API has always served for it.
    fn fixed_variants() -> [(ServiceError, &'static str, &'static str); 5] {
        [
            (
                ServiceError::MailboxNotFound,
                "mailbox_not_found",
                "mailbox not found",
            ),
            (
                ServiceError::MessageNotFound,
                "message_not_found",
                "message not found",
            ),
            (
                ServiceError::AttachmentNotFound,
                "attachment_not_found",
                "attachment not found",
            ),
            (
                ServiceError::MessageTooLarge {
                    source: ParsedMessageError::TooLarge {
                        size: 4096,
                        limit: 1024,
                    },
                },
                "message_too_large",
                "message exceeds the configured size limit",
            ),
            (
                ServiceError::MessageNotParsable {
                    source: ParsedMessageError::Unparsable,
                },
                "message_not_parsable",
                "message cannot be parsed",
            ),
        ]
    }

    /// `kind()`/`public_message()` are the two values every transport renders, so
    /// they are pinned here together with the invariant that `Display` — what
    /// `tracing::error!(%error)` prints — says exactly the same as the public
    /// message.
    #[test]
    fn fixed_variant_codes_and_messages_are_stable() {
        for (error, kind, message) in fixed_variants() {
            assert_eq!(error.kind(), kind);
            assert_eq!(error.public_message(), message);
            assert_eq!(error.to_string(), message);
        }
    }

    /// The size guard's refusal carries the reported and the configured size for
    /// logs; the parser refusal may carry third-party text. Neither may reach a
    /// caller.
    ///
    /// The sizes are asserted **through the type** (`downcast_ref`, not the
    /// `Display` text): [`ParsedMessageError::TooLarge`] keeps `size` and `limit`
    /// as fields, so its `Display` is the fixed `"message too large"` and a
    /// `contains("4096")` on it would measure the wrong thing.
    #[test]
    fn mime_refusals_keep_their_cause_out_of_the_public_message() {
        let too_large = ServiceError::MessageTooLarge {
            source: ParsedMessageError::TooLarge {
                size: 4096,
                limit: 1024,
            },
        };
        assert_eq!(too_large.public_message(), too_large.to_string());
        assert!(!too_large.public_message().contains("4096"));
        assert!(!too_large.public_message().contains("1024"));
        let source = std::error::Error::source(&too_large).expect("the cause must be kept");
        let cause = source
            .downcast_ref::<ParsedMessageError>()
            .expect("the source must be the size guard's refusal, not a rewrite");
        assert!(
            matches!(
                cause,
                ParsedMessageError::TooLarge {
                    size: 4096,
                    limit: 1024
                }
            ),
            "the cause must keep the reported and the configured size for logs: {cause:?}"
        );

        let unparsable = ServiceError::MessageNotParsable {
            source: ParsedMessageError::Unparsable,
        };
        assert_eq!(unparsable.public_message(), "message cannot be parsed");
        assert!(std::error::Error::source(&unparsable).is_some());
    }

    /// The mapping is **per operation**: an operation that never addresses a
    /// single message must never answer `message_not_found`, even when the
    /// session reports one, because the published table assigns that code to the
    /// single-message operations alone.
    #[test]
    fn select_failure_never_yields_message_not_found() {
        assert_eq!(
            ServiceError::select_failure(ImapError::MailboxNotFound).kind(),
            "mailbox_not_found"
        );
        assert_eq!(
            ServiceError::select_failure(ImapError::MessageNotFound).kind(),
            "imap_unavailable"
        );
        assert_eq!(
            ServiceError::select_failure(ImapError::CapabilityNotSupported).kind(),
            "capability_not_supported",
            "a missing server extension keeps its own 501 instead of becoming a 503"
        );
    }

    /// A missing mailbox and a missing message stay two distinct codes, and every
    /// other session failure is an availability failure.
    #[test]
    fn message_failure_keeps_the_two_404s_apart() {
        assert_eq!(
            ServiceError::message_failure(ImapError::MailboxNotFound).kind(),
            "mailbox_not_found"
        );
        assert_eq!(
            ServiceError::message_failure(ImapError::MessageNotFound).kind(),
            "message_not_found"
        );
        assert_eq!(
            ServiceError::message_failure(ImapError::Login(THIRD_PARTY_TEXT.to_string())).kind(),
            "imap_unavailable"
        );
        assert_eq!(
            ServiceError::message_failure(ImapError::CapabilityNotSupported).kind(),
            "capability_not_supported",
            "a missing server extension keeps its own 501 instead of becoming a 503"
        );
    }

    /// The parsed-message mapping dispatches on the cause: the IMAP variant
    /// reuses the message mapping (so its two `404`s survive), while the size
    /// guard and the parser refusal get their own codes.
    #[test]
    fn parsed_failure_dispatches_on_the_cause() {
        let cases = [
            (
                ParsedMessageError::Imap(ImapError::MailboxNotFound),
                "mailbox_not_found",
            ),
            (
                ParsedMessageError::Imap(ImapError::MessageNotFound),
                "message_not_found",
            ),
            (
                ParsedMessageError::Imap(ImapError::Unavailable(THIRD_PARTY_TEXT.to_string())),
                "imap_unavailable",
            ),
            (
                ParsedMessageError::TooLarge { size: 10, limit: 5 },
                "message_too_large",
            ),
            (ParsedMessageError::Unparsable, "message_not_parsable"),
        ];

        for (cause, expected) in cases {
            let error = ServiceError::parsed_failure(cause);
            assert_eq!(error.kind(), expected);
            assert!(
                !error.public_message().contains(THIRD_PARTY_TEXT),
                "the public message leaked third-party text: {}",
                error.public_message()
            );
        }
    }
}
