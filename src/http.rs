//! HTTP routing and handlers.
//!
//! # Health contract
//!
//! `GET /api/health` is **public** and responds with HTTP `200` and
//! `Content-Type: application/json`. The body is a JSON object exposing the
//! service status together with the application name and version resolved at
//! compile time from `Cargo.toml`:
//!
//! ```json
//! { "status": "ok", "name": "apimail", "version": "0.1.0" }
//! ```
//!
//! # Authentication contract
//!
//! Every route **except** `GET /api/health` is protected by the
//! [`require_api_key`] middleware. A request must carry the configured API key
//! in the `Authorization` header using the `Bearer` scheme (the scheme is
//! matched case-insensitively):
//!
//! ```text
//! Authorization: Bearer <APIMAIL_API_KEY>
//! ```
//!
//! The configured key is never compared directly. Instead a SHA-256 digest of
//! the key is precomputed once at startup and the digest of the incoming token
//! is compared against it with `subtle`'s constant-time equality over a
//! **fixed-size** `[u8; 32]` buffer. Because both operands always have the same
//! length, the comparison cannot leak the secret's length through timing; the
//! only variable cost is hashing the attacker-supplied token, which depends
//! solely on the input the attacker already controls.
//!
//! Between the scheme and the token any run of ASCII spaces or tabs is accepted,
//! but whitespace inside the token is not.
//!
//! - `GET /api/whoami` is a protected scaffolding route. On success it returns
//!   HTTP `200`, `Content-Type: application/json` and the body
//!   `{"authenticated":true}`.
//! - `GET /api/account` is a protected route that reports only the non-sensitive
//!   part of the configured mail account. On success it returns HTTP `200`,
//!   `Content-Type: application/json` and the body
//!   `{"imap":{"host":"...","port":993},"smtp":{"host":"...","port":465}}`.
//!   Neither the username nor the password is ever included.
//! - Any missing header, non-`Bearer` scheme or wrong token is rejected with
//!   HTTP `401`, `Content-Type: application/json`, the body
//!   `{"error":"unauthorized","message":"..."}` and the challenge header
//!   `WWW-Authenticate: Bearer`.
//!
//! # Message sending contract
//!
//! `POST /api/messages` is protected by [`require_api_key`] and accepts an
//! `application/json` body describing an outgoing message:
//!
//! ```json
//! {
//!   "from": "optional@example.com",
//!   "to": ["dest@example.com"],
//!   "cc": [], "bcc": [],
//!   "subject": "hello",
//!   "text": "plain body", "html": "<p>html body</p>",
//!   "attachments": [
//!     { "filename": "note.txt", "content_type": "text/plain", "data_base64": "..." }
//!   ]
//! }
//! ```
//!
//! `to` is required (at least one address) and `from` defaults to the configured
//! SMTP user. Attachment content is base64-encoded and is decoded **here**, at
//! the HTTP boundary; the total **decoded** size must not exceed
//! `APIMAIL_MAX_ATTACHMENT_BYTES`. Every address is validated as a mailbox. On
//! success the route returns HTTP `200`, `Content-Type: application/json` and
//! `{"status":"sent"}`. Failures use the `{"error":...,"message":...}` pattern:
//!
//! - `400` (`invalid_request`) — malformed JSON, missing recipients, invalid
//!   address or invalid base64.
//! - `413` (`payload_too_large`) — the raw request body exceeds the per-route
//!   body limit, or the decoded attachments exceed the attachment limit.
//! - `502` (`smtp_error`) — the SMTP server rejects or fails the delivery.
//!
//! The per-route body limit is **not** enforced by axum's `DefaultBodyLimit`
//! (which would answer with a plain-text `413`): the route disables it and the
//! handler reads the body with [`to_bytes`] against
//! [`AppState::message_body_limit`], so even an oversized body gets the shared
//! JSON error envelope. The body limit is
//! `max_attachment_bytes * 4 / 3 + BODY_LIMIT_SLACK` (base64 expansion plus a
//! small, documented slack for the rest of the JSON); the authoritative
//! *decoded* check is still [`AppState::max_attachment_bytes`].
//!
//! Requests for unknown routes still return `404`, and requests using an
//! unhandled method (for example `POST /api/health`) still return `405`; the
//! authentication middleware is layered only over the protected sub-router.
//!
//! # IMAP status contract
//!
//! `GET /api/imap/status` is protected by [`require_api_key`]. It attempts to
//! ensure a live IMAP session (connecting lazily on first use) and reports the
//! outcome — never a credential:
//!
//! - success returns HTTP `200`, `Content-Type: application/json` and
//!   `{"connected":true,"host":"...","port":993,"tls":"implicit|starttls|none"}`,
//!   where `tls` is the configured IMAP mode;
//! - failure returns HTTP `503`, `Content-Type: application/json` and
//!   `{"connected":false,"error":"imap_unavailable","message":"..."}`.
//!
//! # Mailbox contract
//!
//! `GET /api/mailboxes` is protected by [`require_api_key`]. It lists the
//! account's mailboxes (`LIST`) and never exposes a credential. On success it
//! returns HTTP `200`, `Content-Type: application/json` and a body of the shape:
//!
//! ```json
//! {"mailboxes":[{"name":"INBOX","delimiter":"/","attributes":["\\HasNoChildren"]}]}
//! ```
//!
//! where `delimiter` is `null` when the server reports a flat namespace and
//! `attributes` are the IMAP-style mailbox attributes (for example
//! `\NoSelect`).
//!
//! `POST /api/mailboxes/select` is protected by [`require_api_key`] and accepts
//! an `application/json` body `{"mailbox":"INBOX"}`. On success it selects the
//! mailbox (`SELECT`) and returns HTTP `200`, `Content-Type: application/json`
//! and a body of the shape:
//!
//! ```json
//! {"mailbox":"INBOX","exists":42,"recent":0,"unseen":3,"uid_validity":7,"uid_next":100,"flags":["\\Seen"]}
//! ```
//!
//! where `unseen`, `uid_validity` and `uid_next` are `null` when the server
//! omits them. The selected mailbox stays active for subsequent operations.
//!
//! Both routes share the `{"error":...,"message":...}` envelope:
//!
//! - `400` (`invalid_request`) — malformed JSON body or a missing/blank
//!   `mailbox`.
//! - `404` (`mailbox_not_found`) — the server answered `NO` for the mailbox.
//! - `503` (`imap_unavailable`) — the session or server is unavailable; the
//!   message is the stable [`ImapError::public_message`] and never
//!   [`Display`](std::fmt::Display) text from a third party.
//! - `401` (`unauthorized`) — missing or invalid API key.
//!
//! # Message reading contract
//!
//! `GET /api/messages` and `GET /api/messages/{uid}` are protected by
//! [`require_api_key`]. Both select the mailbox named by the required `mailbox`
//! query parameter and never expose a credential; bodies are fetched with
//! `BODY.PEEK`, so reading never sets `\Seen`.
//!
//! `GET /api/messages` accepts the standard `SEARCH` filters `from`, `to`,
//! `subject`, `text`, `since`/`before` (`YYYY-MM-DD`), `seen`/`flagged`
//! (`true`/`false`) and `unseen` (the alias of `seen`: `unseen=true` means
//! `seen=false`), plus `limit` (`1..=200`, default `50`) and `offset` (default
//! `0`). On success it returns HTTP `200`, `Content-Type: application/json` and:
//!
//! ```json
//! {"mailbox":"INBOX","total":123,"limit":50,"offset":0,
//!  "messages":[{"uid":42,"seq":42,"flags":["\\Seen"],"size":2048,
//!    "internal_date":"2026-10-08T12:00:00+00:00",
//!    "envelope":{"from":[{"name":"Alice","address":"alice@example.com"}],
//!      "to":[{"name":null,"address":"bob@example.com"}],"cc":[],
//!      "subject":"hi","date":"...","message_id":"<...>"}}]}
//! ```
//!
//! `GET /api/messages/{uid}` takes the numeric `uid` in the path and an optional
//! `format` of `summary` (default), `headers` or `full`. On success it returns
//! HTTP `200`, `Content-Type: application/json` and the message metadata plus
//! `"format"`; `headers` adds `headers_base64` (the raw header block) and `full`
//! adds `raw_base64` (the full RFC822 source), both base64 because raw MIME is
//! not necessarily UTF-8. Failures use the shared `{"error":...,"message":...}`
//! envelope:
//!
//! - `400` (`invalid_request`) — missing/blank `mailbox`, a control character in
//!   any value, an invalid date, boolean, `limit`/`offset`, `format` or `uid`, or
//!   contradictory `seen`/`unseen`.
//! - `404` (`mailbox_not_found`) — the server answered `NO` for the mailbox.
//! - `404` (`message_not_found`) — the requested `uid` does not exist.
//! - `503` (`imap_unavailable`) — the session or server is unavailable.
//! - `401` (`unauthorized`) — missing or invalid API key.
//!
//! The query string is parsed with [`RawQuery`] + `serde_urlencoded` (not
//! `Query<T>`) so every rejection is the shared JSON envelope rather than axum's
//! plain-text one.
//!
//! # Message mutation contract
//!
//! `PATCH /api/messages/{uid}/flags`, `POST /api/messages/{uid}/move`,
//! `POST /api/messages/{uid}/copy` and `DELETE /api/messages/{uid}` are protected
//! by [`require_api_key`]. All four select the mailbox named by the required
//! `mailbox` query parameter and act on the single message identified by the
//! numeric `uid` in the path. On success they return HTTP `200` and
//! `Content-Type: application/json`:
//!
//! ```json
//! {"mailbox":"INBOX","uid":42,"flags":["\\Seen"]}              // PATCH .../flags
//! {"mailbox":"INBOX","uid":42,"to":"Archive","status":"moved"} // POST .../move
//! {"mailbox":"INBOX","uid":42,"to":"Archive","status":"copied"}// POST .../copy
//! {"mailbox":"INBOX","uid":42,"status":"deleted"}              // DELETE
//! ```
//!
//! `PATCH .../flags` accepts `{"add":["\\Seen","\\Flagged"],"remove":["\\Deleted"]}`
//! (both fields optional; at least one non-empty). Every flag is parsed
//! case-insensitively against a fixed allowlist (`\Seen`, `\Answered`,
//! `\Flagged`, `\Draft`, `\Deleted`), canonicalised and deduplicated, and the
//! same flag may not appear in both `add` and `remove`. The resulting `flags`
//! are read back with a `UID FETCH`. `move`/`copy` accept `{"to":"Archive"}`
//! (required, non-blank, without control characters); `move` uses `UID MOVE`
//! when the server announces it, otherwise emulates it with `UID COPY` +
//! `UID STORE +FLAGS.SILENT (\Deleted)` + `UID EXPUNGE` when `UIDPLUS` is
//! announced. `DELETE` requires `UIDPLUS` and never falls back to a global
//! `EXPUNGE`.
//!
//! Failures use the shared `{"error":...,"message":...}` envelope:
//!
//! - `400` (`invalid_request`) — a non-numeric or zero `uid`, a missing/blank
//!   `mailbox` or `to`, a control character in any value, an unknown or
//!   contradictory flag, an empty flag update, or a malformed JSON body.
//! - `404` (`mailbox_not_found`) — the server answered `NO` for the mailbox.
//! - `404` (`message_not_found`) — the requested `uid` does not exist.
//! - `501` (`capability_not_supported`) — the server does not announce the
//!   extension the operation needs (`MOVE`/`UIDPLUS`).
//! - `503` (`imap_unavailable`) — the session or server is unavailable.
//! - `401` (`unauthorized`) — missing or invalid API key.
//!
//! Validation happens at the HTTP boundary before any command is sent; the
//! `STORE` query is built only from the allowlist and the `UID` set is rendered
//! from a numeric `u32`, so no request can inject IMAP commands.
//!
//! # MIME parsing contract
//!
//! `GET /api/messages/{uid}/body`, `GET /api/messages/{uid}/attachments` and
//! `GET /api/messages/{uid}/attachments/{id}` are protected by [`require_api_key`].
//! All three select the mailbox named by the required `mailbox` query parameter,
//! fetch the single message identified by the numeric `uid` in the path and
//! parse its MIME content on the existing lazily-established session; the raw
//! source is bounded by [`AppState::max_message_bytes`] (configured through
//! `APIMAIL_MAX_MESSAGE_BYTES`) and is never written to disk, logs or headers.
//! The `id` is the positional attachment identifier, so `0` is valid.
//!
//! On success they return HTTP `200` and `Content-Type: application/json`:
//!
//! ```json
//! {"mailbox":"INBOX","uid":42,"text":"...","html":"..."}
//! {"mailbox":"INBOX","uid":42,"attachments":[
//!   {"id":0,"filename":"informe.pdf","content_type":"application/pdf",
//!    "size":13,"inline":false,"content_id":null}]}
//! {"mailbox":"INBOX","uid":42,"id":0,"filename":"informe.pdf",
//!  "content_type":"application/pdf","size":13,"content_base64":"..."}
//! ```
//!
//! `text` and `html` are always present (each `null` when the message carries no
//! such body and none can be derived, following RFC 8621 §4.1.4). The listing
//! omits the content; only the single-attachment route returns it, base64-encoded
//! inside the JSON body so a hostile `filename`/`content_type` can never reach an
//! HTTP header. HTML is returned verbatim: rendering it safely is the client's
//! responsibility.
//!
//! Failures use the shared `{"error":...,"message":...}` envelope:
//!
//! - `400` (`invalid_request`) — a non-numeric or zero `uid`, a non-numeric `id`,
//!   a missing/blank `mailbox`, or a control character in any value; no IMAP
//!   command is sent.
//! - `404` (`mailbox_not_found`) — the server answered `NO` for the mailbox.
//! - `404` (`message_not_found`) — the requested `uid` does not exist.
//! - `404` (`attachment_not_found`) — the requested `id` matches no attachment.
//! - `413` (`message_too_large`) — the message exceeds `APIMAIL_MAX_MESSAGE_BYTES`
//!   and is rejected before its MIME content is parsed.
//! - `422` (`message_not_parsable`) — the message exists but cannot be parsed.
//! - `503` (`imap_unavailable`) — the session or server is unavailable.
//! - `401` (`unauthorized`) — missing or invalid API key.
//!
//! The `413`/`422`/`404` messages are fixed strings, never third-party error
//! text, so no untrusted content is reflected to the client.

use axum::body::{Bytes, to_bytes};
use axum::extract::{DefaultBodyLimit, Path, RawQuery, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::Config;
use crate::config::TlsMode;
use crate::imap::{
    Address, Backoff, ConnectionManager, FetchFormat, ImapConnector, ImapError, MailboxInfo,
    MailboxStatus, Message, MessageEnvelope, ParsedMessageError, SearchCriteria, SearchDate,
    SystemFlag, TokioImapConnector,
};
use crate::mime::ParsedAttachment;
use crate::smtp::{
    MailSender, MessageError as SmtpMessageError, OutgoingAttachment, OutgoingMessage, SmtpError,
    SmtpSender,
};

/// Application name, resolved from `Cargo.toml`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
/// Application version, resolved from `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Slack added to the computed body limit, on top of the base64 expansion of the
/// attachment bytes, to cover the rest of the JSON envelope.
const BODY_LIMIT_SLACK: usize = 64 * 1024;

/// Errors produced while building [`AppState`].
///
/// It wraps both service constructors so a single `Result` covers the real SMTP
/// transport and the IMAP connector, neither of which opens a connection at
/// construction time.
#[derive(Debug, thiserror::Error)]
pub enum AppStateError {
    /// Building the SMTP transport failed.
    #[error(transparent)]
    Smtp(#[from] SmtpError),
    /// Building the IMAP connector failed.
    #[error(transparent)]
    Imap(#[from] ImapError),
}

/// Shared state exposed to handlers.
///
/// The configured API key is stored only as its SHA-256 digest and the field is
/// private, so the plaintext secret can neither be read nor formatted. The
/// manual [`Debug`] implementation redacts the digest as well as the mailer and
/// the connection manager, which hold the SMTP and IMAP credentials.
#[derive(Clone)]
pub struct AppState {
    /// Application name.
    pub name: &'static str,
    /// Application version.
    pub version: &'static str,
    /// SHA-256 digest of the expected API key.
    api_key_hash: [u8; 32],
    /// Non-sensitive view of the configured mail account (host and port only).
    account: AccountView,
    /// Outgoing mail transport (real at runtime, fake in tests).
    mailer: Arc<dyn MailSender>,
    /// Maximum total size, in bytes, of the decoded attachments of a message.
    max_attachment_bytes: usize,
    /// Maximum size, in bytes, of a message that will be fetched and parsed.
    max_message_bytes: usize,
    /// Sender used when a message omits `from` (the configured SMTP user).
    default_from: String,
    /// Lazy, persistent IMAP connection manager.
    imap: Arc<ConnectionManager>,
    /// TLS mode of the configured IMAP endpoint, for the status endpoint.
    imap_tls: TlsMode,
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
            .field("imap", &"***")
            .field("imap_tls", &self.imap_tls)
            .finish()
    }
}

impl AppState {
    /// Builds the application state from the loaded [`Config`].
    ///
    /// This constructs the real SMTP transport and the real IMAP connector from
    /// `config` — it opens no connection, but building either (or resolving its
    /// TLS configuration) can fail, hence the `Result`.
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
        Ok(Self::with_services(config, mailer, connector))
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
        Ok(Self::with_services(config, mailer, connector))
    }

    /// Builds the application state with both services injected.
    ///
    /// Used by tests to exercise the HTTP layer without touching the network.
    pub fn with_services(
        config: &Config,
        mailer: Arc<dyn MailSender>,
        imap_connector: Arc<dyn ImapConnector>,
    ) -> Self {
        Self {
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
        }
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

/// JSON payload returned by `GET /api/health`.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// Liveness status marker.
    pub status: &'static str,
    /// Application name.
    pub name: &'static str,
    /// Application version.
    pub version: &'static str,
}

/// JSON payload returned by `GET /api/whoami`.
#[derive(Debug, Serialize)]
struct WhoamiResponse {
    /// Always `true`: reached only after successful authentication.
    authenticated: bool,
}

/// JSON payload returned by a successful `POST /api/messages`.
#[derive(Debug, Serialize)]
struct SentResponse {
    /// Always `sent`: reached only after the transport accepted the message.
    status: &'static str,
}

/// JSON payload returned by a successful `GET /api/imap/status`.
#[derive(Debug, Serialize)]
struct ImapStatusResponse {
    /// Always `true`: reached only after a live session was ensured.
    connected: bool,
    /// IMAP server host.
    host: String,
    /// IMAP server port.
    port: u16,
    /// Configured TLS mode label.
    tls: &'static str,
}

/// JSON payload returned by a failing `GET /api/imap/status`.
#[derive(Debug, Serialize)]
struct ImapUnavailableResponse {
    /// Always `false`.
    connected: bool,
    /// Stable machine-readable error code.
    error: &'static str,
    /// Human-readable explanation.
    message: String,
}

/// One mailbox in the `GET /api/mailboxes` response.
#[derive(Debug, Serialize)]
struct MailboxDto {
    /// Mailbox name.
    name: String,
    /// Hierarchy delimiter, or `null` for a flat namespace.
    delimiter: Option<String>,
    /// Mailbox attributes in IMAP style (`\NoSelect`, …).
    attributes: Vec<String>,
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
#[derive(Debug, Serialize)]
struct MailboxListResponse {
    /// The account's mailboxes.
    mailboxes: Vec<MailboxDto>,
}

/// `POST /api/mailboxes/select` request body.
#[derive(Debug, Deserialize)]
struct SelectMailboxDto {
    /// Mailbox to select.
    mailbox: String,
}

/// JSON payload returned by a successful `POST /api/mailboxes/select`.
#[derive(Debug, Serialize)]
struct MailboxStatusResponse {
    /// Selected mailbox name.
    mailbox: String,
    /// Number of messages (`EXISTS`).
    exists: u32,
    /// Messages with the `\Recent` flag (`RECENT`).
    recent: u32,
    /// First unseen message (`UNSEEN`), if reported.
    unseen: Option<u32>,
    /// UID validity (`UIDVALIDITY`), if reported.
    uid_validity: Option<u32>,
    /// Next UID (`UIDNEXT`), if reported.
    uid_next: Option<u32>,
    /// Defined message flags in IMAP style (`\Seen`, …).
    flags: Vec<String>,
}

impl MailboxStatusResponse {
    /// Builds the response from the selected mailbox status.
    fn new(mailbox: String, status: MailboxStatus) -> Self {
        Self {
            mailbox,
            exists: status.exists,
            recent: status.recent,
            unseen: status.unseen,
            uid_validity: status.uid_validity,
            uid_next: status.uid_next,
            flags: status.flags,
        }
    }
}

/// Failure modes of the mailbox routes, mapped to the documented statuses.
#[derive(Debug)]
enum MailboxError {
    /// The request payload is malformed or misses a non-empty `mailbox`
    /// (`400`).
    InvalidRequest(String),
    /// The server answered `NO`: the mailbox does not exist (`404`).
    NotFound,
    /// The IMAP session or server is unavailable (`503`).
    Unavailable(String),
}

impl IntoResponse for MailboxError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            Self::InvalidRequest(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                "mailbox_not_found",
                "mailbox not found".to_string(),
            ),
            Self::Unavailable(message) => {
                (StatusCode::SERVICE_UNAVAILABLE, "imap_unavailable", message)
            }
        };
        (status, Json(ErrorResponse { error, message })).into_response()
    }
}

/// Maps an [`ImapError`] to the mailbox routes' failure model.
///
/// A missing mailbox becomes `404`; every other failure becomes `503` carrying
/// the stable, credential-free [`ImapError::public_message`].
impl From<ImapError> for MailboxError {
    fn from(error: ImapError) -> Self {
        match error {
            ImapError::MailboxNotFound => Self::NotFound,
            other => Self::Unavailable(other.public_message().to_string()),
        }
    }
}

/// One address in a message envelope.
#[derive(Debug, Serialize)]
struct AddressDto {
    /// Display name, or `null`.
    name: Option<String>,
    /// `mailbox@host`, or `null`.
    address: Option<String>,
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
#[derive(Debug, Serialize)]
struct EnvelopeDto {
    /// `From` addresses.
    from: Vec<AddressDto>,
    /// `To` addresses.
    to: Vec<AddressDto>,
    /// `Cc` addresses.
    cc: Vec<AddressDto>,
    /// Raw `Subject` header.
    subject: Option<String>,
    /// Raw `Date` header.
    date: Option<String>,
    /// `Message-ID`.
    message_id: Option<String>,
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
#[derive(Debug, Serialize)]
struct MessageDto {
    /// Unique identifier within the mailbox.
    uid: u32,
    /// Sequence number within the mailbox.
    seq: u32,
    /// Flags in IMAP style (`\Seen`, …).
    flags: Vec<String>,
    /// `RFC822.SIZE`, if reported.
    size: Option<u32>,
    /// `INTERNALDATE` in RFC 3339 form, if reported.
    internal_date: Option<String>,
    /// Parsed envelope, if reported.
    envelope: Option<EnvelopeDto>,
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
#[derive(Debug, Serialize)]
struct MessageListResponse {
    /// Selected mailbox name.
    mailbox: String,
    /// Total number of messages matching the search.
    total: u32,
    /// Applied page size.
    limit: u32,
    /// Applied page offset.
    offset: u32,
    /// The messages in this page.
    messages: Vec<MessageDto>,
}

/// JSON payload returned by a successful `GET /api/messages/{uid}`.
///
/// The `headers_base64` and `raw_base64` fields are present only for the matching
/// `format`, hence the conditional serialization.
#[derive(Debug, Serialize)]
struct MessageDetailResponse {
    /// Selected mailbox name.
    mailbox: String,
    /// Unique identifier within the mailbox.
    uid: u32,
    /// Sequence number within the mailbox.
    seq: u32,
    /// Flags in IMAP style (`\Seen`, …).
    flags: Vec<String>,
    /// `RFC822.SIZE`, if reported.
    size: Option<u32>,
    /// `INTERNALDATE` in RFC 3339 form, if reported.
    internal_date: Option<String>,
    /// Parsed envelope, if reported.
    envelope: Option<EnvelopeDto>,
    /// Requested format label: `summary`, `headers` or `full`.
    format: &'static str,
    /// Raw header block, base64-encoded (only for `format=headers`).
    #[serde(skip_serializing_if = "Option::is_none")]
    headers_base64: Option<String>,
    /// Full RFC822 source, base64-encoded (only for `format=full`).
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_base64: Option<String>,
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
/// no such body and none can be derived.
#[derive(Debug, Serialize)]
struct MessageBodyResponse {
    /// Selected mailbox name.
    mailbox: String,
    /// Unique identifier within the mailbox.
    uid: u32,
    /// Derived plain-text body, or `null`.
    text: Option<String>,
    /// Decoded HTML body, or `null`.
    html: Option<String>,
}

/// One attachment in the `GET /api/messages/{uid}/attachments` response.
///
/// The content is intentionally omitted: it is only ever returned by the
/// single-attachment route, base64-encoded inside the JSON body.
#[derive(Debug, Serialize)]
struct ParsedAttachmentDto {
    /// Positional identifier within the parsed attachment list.
    id: u32,
    /// Decoded file name, or `null`.
    filename: Option<String>,
    /// MIME content type, or `null`.
    content_type: Option<String>,
    /// Decoded content length, in bytes.
    size: usize,
    /// Whether the part is marked `inline`.
    inline: bool,
    /// `Content-ID`, or `null`.
    content_id: Option<String>,
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
#[derive(Debug, Serialize)]
struct AttachmentListResponse {
    /// Selected mailbox name.
    mailbox: String,
    /// Unique identifier within the mailbox.
    uid: u32,
    /// The message's attachments, without their content.
    attachments: Vec<ParsedAttachmentDto>,
}

/// JSON payload returned by a successful
/// `GET /api/messages/{uid}/attachments/{id}`.
#[derive(Debug, Serialize)]
struct AttachmentResponse {
    /// Selected mailbox name.
    mailbox: String,
    /// Unique identifier within the mailbox.
    uid: u32,
    /// Positional identifier of the attachment.
    id: u32,
    /// Decoded file name, or `null`.
    filename: Option<String>,
    /// MIME content type, or `null`.
    content_type: Option<String>,
    /// Decoded content length, in bytes.
    size: usize,
    /// The attachment's decoded content, base64-encoded.
    content_base64: String,
}

/// Query string of `GET /api/messages`, decoded loosely into optional strings.
///
/// Kept as `Option<String>` for every field so the handler can validate each
/// value itself and answer with the shared JSON `400` envelope instead of axum's
/// plain-text rejection.
#[derive(Debug, Default, Deserialize)]
struct ListMessagesQuery {
    /// Mailbox to search (required).
    mailbox: Option<String>,
    /// `FROM` filter.
    from: Option<String>,
    /// `TO` filter.
    to: Option<String>,
    /// `SUBJECT` filter.
    subject: Option<String>,
    /// `BODY` filter.
    text: Option<String>,
    /// `SINCE` date filter (`YYYY-MM-DD`).
    since: Option<String>,
    /// `BEFORE` date filter (`YYYY-MM-DD`).
    before: Option<String>,
    /// `SEEN`/`UNSEEN` filter.
    seen: Option<String>,
    /// `FLAGGED`/`UNFLAGGED` filter.
    flagged: Option<String>,
    /// Alias of `seen` (`unseen=true` means `seen=false`).
    unseen: Option<String>,
    /// Page size (`1..=200`, default `50`).
    limit: Option<String>,
    /// Page offset (default `0`).
    offset: Option<String>,
}

/// Query string of `GET /api/messages/{uid}`.
#[derive(Debug, Default, Deserialize)]
struct FetchMessageQuery {
    /// Mailbox to select (required).
    mailbox: Option<String>,
    /// Requested format: `summary` (default), `headers` or `full`.
    format: Option<String>,
}

/// Whether `value` carries a character that could break an IMAP command line.
///
/// The IMAP command strings are built by quoting, but rejecting `CR`, `LF` and
/// `NUL` at the boundary is the documented defence in depth.
fn has_control_characters(value: &str) -> bool {
    value.chars().any(|c| matches!(c, '\r' | '\n' | '\0'))
}

/// Parses a strict `true`/`false` boolean.
fn parse_strict_bool(value: &str, field: &str) -> Result<bool, MessageError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(MessageError::InvalidRequest(format!(
            "`{field}` must be `true` or `false`"
        ))),
    }
}

/// Parses and validates a non-blank mailbox name.
fn parse_mailbox(value: Option<String>) -> Result<String, MessageError> {
    let value = value.ok_or_else(|| {
        MessageError::InvalidRequest("`mailbox` query parameter is required".to_string())
    })?;
    if has_control_characters(&value) {
        return Err(MessageError::InvalidRequest(
            "`mailbox` contains a forbidden control character".to_string(),
        ));
    }
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(MessageError::InvalidRequest(
            "`mailbox` must not be empty".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Validates an optional free-text filter against control characters.
fn parse_text_filter(value: Option<String>, field: &str) -> Result<Option<String>, MessageError> {
    match value {
        Some(value) if has_control_characters(&value) => Err(MessageError::InvalidRequest(
            format!("`{field}` contains a forbidden control character"),
        )),
        other => Ok(other),
    }
}

/// Validates and parses an optional date filter.
fn parse_date_filter(
    value: Option<String>,
    field: &str,
) -> Result<Option<SearchDate>, MessageError> {
    match value {
        Some(value) => SearchDate::parse(&value)
            .map(Some)
            .map_err(|_| MessageError::InvalidRequest(format!("`{field}` must be `YYYY-MM-DD`"))),
        None => Ok(None),
    }
}

/// Resolves the `seen` filter, folding the `unseen` alias in.
///
/// `unseen=true` is `seen=false` and vice versa. When both are supplied they must
/// agree, otherwise the request is rejected.
fn parse_seen_filter(
    seen: Option<String>,
    unseen: Option<String>,
) -> Result<Option<bool>, MessageError> {
    match (seen, unseen) {
        (Some(seen), Some(unseen)) => {
            let seen = parse_strict_bool(&seen, "seen")?;
            let unseen = parse_strict_bool(&unseen, "unseen")?;
            if seen == unseen {
                return Err(MessageError::InvalidRequest(
                    "`seen` and `unseen` disagree".to_string(),
                ));
            }
            Ok(Some(seen))
        }
        (Some(seen), None) => Ok(Some(parse_strict_bool(&seen, "seen")?)),
        (None, Some(unseen)) => Ok(Some(!parse_strict_bool(&unseen, "unseen")?)),
        (None, None) => Ok(None),
    }
}

/// Parses an optional `u32` bounded above.
fn parse_bounded_u32(
    value: Option<String>,
    field: &str,
    default: u32,
    max: u32,
) -> Result<u32, MessageError> {
    match value {
        Some(value) => {
            let parsed: u32 = value.parse().map_err(|_| {
                MessageError::InvalidRequest(format!("`{field}` must be an integer"))
            })?;
            if parsed < 1 || parsed > max {
                return Err(MessageError::InvalidRequest(format!(
                    "`{field}` must be between 1 and {max}"
                )));
            }
            Ok(parsed)
        }
        None => Ok(default),
    }
}

/// Parses an optional non-negative `u32`.
fn parse_optional_u32(
    value: Option<String>,
    field: &str,
    default: u32,
) -> Result<u32, MessageError> {
    match value {
        Some(value) => value.parse().map_err(|_| {
            MessageError::InvalidRequest(format!("`{field}` must be a non-negative integer"))
        }),
        None => Ok(default),
    }
}

impl ListMessagesQuery {
    /// Validates the query and converts it into the domain pieces.
    fn into_query(self) -> Result<(String, SearchCriteria, u32, u32), MessageError> {
        let mailbox = parse_mailbox(self.mailbox)?;

        let criteria = SearchCriteria {
            from: parse_text_filter(self.from, "from")?,
            to: parse_text_filter(self.to, "to")?,
            subject: parse_text_filter(self.subject, "subject")?,
            text: parse_text_filter(self.text, "text")?,
            since: parse_date_filter(self.since, "since")?,
            before: parse_date_filter(self.before, "before")?,
            seen: parse_seen_filter(self.seen, self.unseen)?,
            flagged: match self.flagged {
                Some(value) => Some(parse_strict_bool(&value, "flagged")?),
                None => None,
            },
        };

        let limit = parse_bounded_u32(self.limit, "limit", 50, 200)?;
        let offset = parse_optional_u32(self.offset, "offset", 0)?;
        Ok((mailbox, criteria, limit, offset))
    }
}

impl FetchMessageQuery {
    /// Validates the query, resolving the mailbox and the requested format.
    fn into_query(self) -> Result<(String, FetchFormat), MessageError> {
        let mailbox = parse_mailbox(self.mailbox)?;
        let format = match self.format.as_deref() {
            None | Some("summary") => FetchFormat::Summary,
            Some("headers") => FetchFormat::Headers,
            Some("full") => FetchFormat::Full,
            Some(_) => {
                return Err(MessageError::InvalidRequest(
                    "`format` must be `summary`, `headers` or `full`".to_string(),
                ));
            }
        };
        Ok((mailbox, format))
    }
}

/// Query string shared by the flag, move, copy and delete routes.
#[derive(Debug, Default, Deserialize)]
struct MailboxQuery {
    /// Mailbox to act on (required).
    mailbox: Option<String>,
}

/// `PATCH /api/messages/{uid}/flags` request body.
///
/// Both fields are optional and default to empty; the handler then requires at
/// least one of them to be non-empty.
#[derive(Debug, Default, Deserialize)]
struct FlagUpdateDto {
    /// Flags to add.
    #[serde(default)]
    add: Vec<String>,
    /// Flags to remove.
    #[serde(default)]
    remove: Vec<String>,
}

impl FlagUpdateDto {
    /// Validates the request into allowlisted domain flags.
    ///
    /// Each value goes through [`SystemFlag::parse`] (case-insensitive) and is
    /// canonicalised and deduplicated, preserving order. At least one of `add`
    /// or `remove` must be non-empty, and no flag may appear in both.
    fn into_flags(self) -> Result<(Vec<SystemFlag>, Vec<SystemFlag>), MessageError> {
        let add = parse_flags(self.add)?;
        let remove = parse_flags(self.remove)?;
        if add.is_empty() && remove.is_empty() {
            return Err(MessageError::InvalidRequest(
                "at least one of `add` or `remove` must be non-empty".to_string(),
            ));
        }
        if add.iter().any(|flag| remove.contains(flag)) {
            return Err(MessageError::InvalidRequest(
                "the same flag cannot be added and removed".to_string(),
            ));
        }
        Ok((add, remove))
    }
}

/// `POST /api/messages/{uid}/move` and `.../copy` request body.
#[derive(Debug, Deserialize)]
struct MoveCopyDto {
    /// Destination mailbox.
    to: String,
}

/// Parses and canonicalises a list of flag names against the allowlist.
///
/// The unknown-flag message is fixed and never echoes the supplied value.
fn parse_flags(values: Vec<String>) -> Result<Vec<SystemFlag>, MessageError> {
    let mut flags: Vec<SystemFlag> = Vec::with_capacity(values.len());
    for value in values {
        let flag = SystemFlag::parse(&value).map_err(|_| {
            MessageError::InvalidRequest("a flag is not a known system flag".to_string())
        })?;
        if !flags.contains(&flag) {
            flags.push(flag);
        }
    }
    Ok(flags)
}

/// Parses a strictly positive numeric `uid` from the path.
fn parse_uid(uid: &str) -> Result<u32, MessageError> {
    let uid: u32 = uid.parse().map_err(|_| {
        MessageError::InvalidRequest("`uid` must be a numeric identifier".to_string())
    })?;
    if uid == 0 {
        return Err(MessageError::InvalidRequest(
            "`uid` must be a positive integer".to_string(),
        ));
    }
    Ok(uid)
}

/// Parses a positional attachment `id` from the path.
///
/// Unlike `uid`, `0` is a valid attachment identifier; only a non-numeric value
/// is rejected.
fn parse_attachment_id(id: &str) -> Result<u32, MessageError> {
    id.parse()
        .map_err(|_| MessageError::InvalidRequest("`id` must be a numeric identifier".to_string()))
}

/// Validates a non-blank destination mailbox.
///
/// `CR`, `LF` and `NUL` are rejected at the boundary (defence in depth); the
/// value is then trimmed and must not be empty.
fn parse_destination(value: String) -> Result<String, MessageError> {
    if has_control_characters(&value) {
        return Err(MessageError::InvalidRequest(
            "`to` contains a forbidden control character".to_string(),
        ));
    }
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(MessageError::InvalidRequest(
            "`to` must not be empty".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

/// JSON payload returned by a successful `PATCH /api/messages/{uid}/flags`.
#[derive(Debug, Serialize)]
struct FlagUpdateResponse {
    /// Selected mailbox name.
    mailbox: String,
    /// Unique identifier within the mailbox.
    uid: u32,
    /// Resulting flags in IMAP style (`\Seen`, …).
    flags: Vec<String>,
}

/// JSON payload returned by a successful move or copy.
#[derive(Debug, Serialize)]
struct MoveCopyResponse {
    /// Selected (source) mailbox name.
    mailbox: String,
    /// Unique identifier within the source mailbox.
    uid: u32,
    /// Destination mailbox name.
    to: String,
    /// `moved` for a move, `copied` for a copy.
    status: &'static str,
}

/// JSON payload returned by a successful `DELETE /api/messages/{uid}`.
#[derive(Debug, Serialize)]
struct DeleteResponse {
    /// Selected mailbox name.
    mailbox: String,
    /// Unique identifier within the mailbox.
    uid: u32,
    /// Always `deleted`.
    status: &'static str,
}

/// Decodes a raw query string into `T`, mapping a failure to `invalid_request`.
fn parse_raw_query<T>(raw: Option<String>) -> Result<T, MessageError>
where
    T: serde::de::DeserializeOwned + Default,
{
    match raw {
        Some(raw) => serde_urlencoded::from_str(&raw)
            .map_err(|_| MessageError::InvalidRequest("invalid query string".to_string())),
        None => Ok(T::default()),
    }
}

/// Failure modes of the message routes, mapped to the documented statuses.
#[derive(Debug)]
enum MessageError {
    /// The request is malformed (`400`).
    InvalidRequest(String),
    /// The server answered `NO`: the mailbox does not exist (`404`).
    MailboxNotFound,
    /// The requested message does not exist (`404`).
    MessageNotFound,
    /// The requested attachment does not exist (`404`).
    AttachmentNotFound,
    /// The message exceeds the configured parsing size limit (`413`).
    TooLarge,
    /// The message exists but its MIME content cannot be parsed (`422`).
    Unparsable,
    /// The server does not announce the extension the operation needs (`501`).
    CapabilityNotSupported(String),
    /// The IMAP session or server is unavailable (`503`).
    Unavailable(String),
}

impl IntoResponse for MessageError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            Self::InvalidRequest(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
            Self::MailboxNotFound => (
                StatusCode::NOT_FOUND,
                "mailbox_not_found",
                "mailbox not found".to_string(),
            ),
            Self::MessageNotFound => (
                StatusCode::NOT_FOUND,
                "message_not_found",
                "message not found".to_string(),
            ),
            Self::AttachmentNotFound => (
                StatusCode::NOT_FOUND,
                "attachment_not_found",
                "attachment not found".to_string(),
            ),
            Self::TooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "message_too_large",
                "message exceeds the configured size limit".to_string(),
            ),
            Self::Unparsable => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "message_not_parsable",
                "message cannot be parsed".to_string(),
            ),
            Self::CapabilityNotSupported(message) => (
                StatusCode::NOT_IMPLEMENTED,
                "capability_not_supported",
                message,
            ),
            Self::Unavailable(message) => {
                (StatusCode::SERVICE_UNAVAILABLE, "imap_unavailable", message)
            }
        };
        (status, Json(ErrorResponse { error, message })).into_response()
    }
}

/// Maps an [`ImapError`] to the message routes' failure model.
///
/// Missing mailboxes and messages become `404`, a missing extension becomes
/// `501` and every other failure becomes `503`, always carrying the stable,
/// credential-free [`ImapError::public_message`].
impl From<ImapError> for MessageError {
    fn from(error: ImapError) -> Self {
        match &error {
            ImapError::MailboxNotFound => Self::MailboxNotFound,
            ImapError::MessageNotFound => Self::MessageNotFound,
            ImapError::InvalidFlags => Self::InvalidRequest("invalid flags".to_string()),
            ImapError::CapabilityNotSupported => {
                Self::CapabilityNotSupported(error.public_message().to_string())
            }
            other => Self::Unavailable(other.public_message().to_string()),
        }
    }
}

/// Maps a [`ParsedMessageError`] to the message routes' failure model.
///
/// The IMAP variant reuses the existing [`From<ImapError>`] mapping; the size
/// guard and the parser refusal become `413` and `422` respectively, each with a
/// fixed message that never echoes third-party text.
impl From<ParsedMessageError> for MessageError {
    fn from(error: ParsedMessageError) -> Self {
        match error {
            ParsedMessageError::Imap(error) => Self::from(error),
            ParsedMessageError::TooLarge { .. } => Self::TooLarge,
            ParsedMessageError::Unparsable => Self::Unparsable,
        }
    }
}

/// One attachment of the `POST /api/messages` request body.
#[derive(Debug, Deserialize)]
struct AttachmentDto {
    /// File name presented to the recipient.
    filename: String,
    /// Optional MIME content type.
    #[serde(default)]
    content_type: Option<String>,
    /// Attachment content, base64-encoded.
    data_base64: String,
}

/// `POST /api/messages` request body.
#[derive(Debug, Deserialize)]
struct SendMessageDto {
    /// Optional sender; defaults to the configured SMTP user.
    #[serde(default)]
    from: Option<String>,
    /// Primary recipients (at least one is required).
    #[serde(default)]
    to: Vec<String>,
    /// Carbon-copy recipients.
    #[serde(default)]
    cc: Vec<String>,
    /// Blind carbon-copy recipients.
    #[serde(default)]
    bcc: Vec<String>,
    /// Subject line.
    #[serde(default)]
    subject: String,
    /// Optional plain-text body.
    #[serde(default)]
    text: Option<String>,
    /// Optional HTML body.
    #[serde(default)]
    html: Option<String>,
    /// Attachments.
    #[serde(default)]
    attachments: Vec<AttachmentDto>,
}

impl SendMessageDto {
    /// Validates the request and converts it into an [`OutgoingMessage`].
    ///
    /// The effective sender is resolved here (to `default_from` when `from` is
    /// absent) so downstream transports — including test fakes — always observe
    /// the sender that will actually be used.
    fn into_outgoing(
        self,
        default_from: &str,
        limit: usize,
    ) -> Result<OutgoingMessage, SmtpMessageError> {
        if self.to.is_empty() {
            return Err(SmtpMessageError::MissingRecipient);
        }

        let mut total = 0usize;
        let mut attachments = Vec::with_capacity(self.attachments.len());
        for attachment in self.attachments {
            let data = STANDARD
                .decode(attachment.data_base64.as_bytes())
                .map_err(|_| SmtpMessageError::InvalidBase64 {
                    filename: attachment.filename.clone(),
                })?;
            total = total.saturating_add(data.len());
            attachments.push(OutgoingAttachment {
                filename: attachment.filename,
                content_type: attachment.content_type,
                data,
            });
        }
        if total > limit {
            return Err(SmtpMessageError::TooLarge { total, limit });
        }

        Ok(OutgoingMessage {
            from: Some(self.from.unwrap_or_else(|| default_from.to_string())),
            to: self.to,
            cc: self.cc,
            bcc: self.bcc,
            subject: self.subject,
            text: self.text,
            html: self.html,
            attachments,
        })
    }
}

/// JSON body serialised for a `POST /api/messages` failure.
#[derive(Debug, Serialize)]
struct ErrorResponse {
    /// Stable machine-readable error code.
    error: &'static str,
    /// Human-readable explanation.
    message: String,
}

/// Failure modes of `POST /api/messages`, mapped to the documented statuses.
#[derive(Debug)]
enum SendError {
    /// Invalid request payload (`400`).
    InvalidRequest(String),
    /// The request is too large — the raw body or the decoded attachments exceed
    /// the configured limit (`413`).
    PayloadTooLarge(String),
    /// The SMTP transport failed (`502`).
    Smtp(String),
}

impl IntoResponse for SendError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            Self::InvalidRequest(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
            Self::PayloadTooLarge(message) => {
                (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", message)
            }
            Self::Smtp(message) => (StatusCode::BAD_GATEWAY, "smtp_error", message),
        };
        (status, Json(ErrorResponse { error, message })).into_response()
    }
}

/// Single point of truth mapping a validation failure to its HTTP status: an
/// oversized attachment is `413`, anything else is a `400`.
impl From<SmtpMessageError> for SendError {
    fn from(error: SmtpMessageError) -> Self {
        match error {
            SmtpMessageError::TooLarge { .. } => Self::PayloadTooLarge(error.to_string()),
            other => Self::InvalidRequest(other.to_string()),
        }
    }
}

/// Rejection returned when a request is not authenticated.
///
/// This is the embryo of the unified error model: a dedicated type implementing
/// [`IntoResponse`]. It renders the `401` contract (JSON body plus the
/// `Bearer` challenge).
#[derive(Debug)]
struct Unauthorized;

/// JSON body serialised for a `401` response.
#[derive(Debug, Serialize)]
struct UnauthorizedResponse {
    /// Stable machine-readable error code.
    error: &'static str,
    /// Human-readable explanation.
    message: &'static str,
}

impl IntoResponse for Unauthorized {
    fn into_response(self) -> Response {
        let body = Json(UnauthorizedResponse {
            error: "unauthorized",
            message: "missing or invalid API key",
        });
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            body,
        )
            .into_response()
    }
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        name: state.name,
        version: state.version,
    })
}

async fn whoami() -> Json<WhoamiResponse> {
    Json(WhoamiResponse {
        authenticated: true,
    })
}

async fn account(State(state): State<AppState>) -> Response {
    Json(&state.account).into_response()
}

/// `GET /api/imap/status` handler: ensures a live IMAP session and reports the
/// outcome without exposing any credential.
async fn imap_status(State(state): State<AppState>) -> Response {
    match state.imap.status().await {
        Ok(()) => (
            StatusCode::OK,
            Json(ImapStatusResponse {
                connected: true,
                host: state.account.imap.host.clone(),
                port: state.account.imap.port,
                tls: state.imap_tls.as_str(),
            }),
        )
            .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ImapUnavailableResponse {
                connected: false,
                error: "imap_unavailable",
                message: error.public_message().to_string(),
            }),
        )
            .into_response(),
    }
}

/// `GET /api/mailboxes` handler: lists the account's mailboxes.
async fn list_mailboxes(State(state): State<AppState>) -> Result<Response, MailboxError> {
    let mailboxes = state.imap.list_mailboxes().await?;
    let response = MailboxListResponse {
        mailboxes: mailboxes.into_iter().map(MailboxDto::from).collect(),
    };
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `POST /api/mailboxes/select` handler: selects a mailbox, reporting its
/// status.
///
/// The body is read as raw bytes and parsed here so a malformed payload gets
/// the shared JSON error envelope (`400 invalid_request`) instead of axum's
/// plain-text rejection. A missing or blank (`trim`-empty) `mailbox` is also
/// rejected with a `400`.
async fn select_mailbox(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, MailboxError> {
    // A fixed message: the serde error could echo part of the attacker-supplied
    // payload, so it must not be reflected back to the client.
    let dto: SelectMailboxDto = serde_json::from_slice(&body)
        .map_err(|_| MailboxError::InvalidRequest("invalid JSON body".to_string()))?;

    let mailbox = dto.mailbox.trim();
    if mailbox.is_empty() {
        return Err(MailboxError::InvalidRequest(
            "`mailbox` must not be empty".to_string(),
        ));
    }

    let status = state.imap.select_mailbox(mailbox).await?;
    let response = MailboxStatusResponse::new(mailbox.to_string(), status);
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages` handler: searches a mailbox and returns one page.
///
/// The query string is decoded with [`RawQuery`] + `serde_urlencoded` so every
/// rejection is the shared JSON `400` envelope; the mailbox is selected, the
/// search is run and only the requested window is fetched, all on the same live
/// session.
async fn list_messages(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let query: ListMessagesQuery = parse_raw_query(raw)?;
    let (mailbox, criteria, limit, offset) = query.into_query()?;

    let page = state
        .imap
        .list_messages(&mailbox, criteria, limit, offset)
        .await?;

    let response = MessageListResponse {
        mailbox,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        messages: page.messages.iter().map(MessageDto::from).collect(),
    };
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages/{uid}` handler: fetches a single message.
///
/// The `uid` is extracted as a string and parsed here so a non-numeric value is
/// answered with the shared JSON `400` instead of axum's plain-text rejection.
async fn fetch_message(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid: u32 = uid.parse().map_err(|_| {
        MessageError::InvalidRequest("`uid` must be a numeric identifier".to_string())
    })?;
    if uid == 0 {
        // UID 0 is not a valid IMAP message identifier.
        return Err(MessageError::InvalidRequest(
            "`uid` must be a positive integer".to_string(),
        ));
    }

    let query: FetchMessageQuery = parse_raw_query(raw)?;
    let (mailbox, format) = query.into_query()?;

    let message = state.imap.fetch_message(&mailbox, uid, format).await?;
    let response = MessageDetailResponse::new(mailbox, message, format);
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages/{uid}/body` handler: fetches the single message, parses
/// its MIME content and returns its plain-text and HTML bodies.
///
/// The `uid` path parameter and the `mailbox` query parameter are validated
/// first (shared JSON `400` envelope), so no IMAP command is sent for an invalid
/// request. Oversized and unparsable messages are reported by the domain layer
/// and mapped to `413`/`422`.
async fn get_message_body(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;

    let parsed = state
        .imap
        .fetch_parsed(&mailbox, uid, state.max_message_bytes())
        .await?;

    Ok((
        StatusCode::OK,
        Json(MessageBodyResponse {
            mailbox,
            uid,
            text: parsed.text,
            html: parsed.html,
        }),
    )
        .into_response())
}

/// `GET /api/messages/{uid}/attachments` handler: fetches the single message,
/// parses it and returns the metadata of each attachment, without their content.
async fn list_attachments(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;

    let parsed = state
        .imap
        .fetch_parsed(&mailbox, uid, state.max_message_bytes())
        .await?;

    let response = AttachmentListResponse {
        mailbox,
        uid,
        attachments: parsed
            .attachments
            .iter()
            .map(ParsedAttachmentDto::from)
            .collect(),
    };
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages/{uid}/attachments/{id}` handler: fetches the single
/// message, parses it and returns the decoded content of the attachment
/// identified by the positional `id`, base64-encoded inside the JSON body.
///
/// The content is never reflected into an HTTP header, so a hostile `filename`
/// or `content_type` cannot inject one.
async fn get_attachment(
    State(state): State<AppState>,
    Path((uid, id)): Path<(String, String)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let id = parse_attachment_id(&id)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;

    let parsed = state
        .imap
        .fetch_parsed(&mailbox, uid, state.max_message_bytes())
        .await?;
    let attachment = parsed
        .attachment(id)
        .ok_or(MessageError::AttachmentNotFound)?;

    Ok((
        StatusCode::OK,
        Json(AttachmentResponse {
            mailbox,
            uid,
            id,
            filename: attachment.filename.clone(),
            content_type: attachment.content_type.clone(),
            size: attachment.size,
            content_base64: STANDARD.encode(&attachment.content),
        }),
    )
        .into_response())
}

/// `PATCH /api/messages/{uid}/flags` handler: applies flag additions/removals to
/// a single message and returns its resulting flags.
///
/// The `uid` path parameter and the `mailbox` query parameter are validated
/// first (shared JSON `400` envelope). The body is read as raw bytes and parsed
/// here so a malformed payload or an unknown flag is a `400` rather than axum's
/// plain-text rejection; validation rejects an empty or contradictory update
/// **before** any IMAP command is sent.
async fn update_flags(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;

    // A fixed message: the serde error could echo part of the attacker-supplied
    // payload, so it must not be reflected back to the client.
    let dto: FlagUpdateDto = serde_json::from_slice(&body)
        .map_err(|_| MessageError::InvalidRequest("invalid JSON body".to_string()))?;
    let (add, remove) = dto.into_flags()?;

    let flags = state.imap.update_flags(&mailbox, uid, add, remove).await?;
    Ok((
        StatusCode::OK,
        Json(FlagUpdateResponse {
            mailbox,
            uid,
            flags,
        }),
    )
        .into_response())
}

/// `POST /api/messages/{uid}/move` handler: moves a single message to another
/// mailbox.
async fn move_message(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;
    let to = parse_body_destination(&body)?;

    state.imap.move_message(&mailbox, uid, &to).await?;
    Ok((
        StatusCode::OK,
        Json(MoveCopyResponse {
            mailbox,
            uid,
            to,
            status: "moved",
        }),
    )
        .into_response())
}

/// `POST /api/messages/{uid}/copy` handler: copies a single message to another
/// mailbox.
async fn copy_message(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;
    let to = parse_body_destination(&body)?;

    state.imap.copy_message(&mailbox, uid, &to).await?;
    Ok((
        StatusCode::OK,
        Json(MoveCopyResponse {
            mailbox,
            uid,
            to,
            status: "copied",
        }),
    )
        .into_response())
}

/// `DELETE /api/messages/{uid}` handler: deletes a single message.
async fn delete_message(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;

    state.imap.delete_message(&mailbox, uid).await?;
    Ok((
        StatusCode::OK,
        Json(DeleteResponse {
            mailbox,
            uid,
            status: "deleted",
        }),
    )
        .into_response())
}

/// Parses the required `to` field of a move/copy body and validates it.
///
/// A malformed body or a missing/blank/control-character `to` becomes a `400`
/// with a fixed message that never echoes the payload.
fn parse_body_destination(body: &[u8]) -> Result<String, MessageError> {
    let dto: MoveCopyDto = serde_json::from_slice(body)
        .map_err(|_| MessageError::InvalidRequest("invalid JSON body".to_string()))?;
    parse_destination(dto.to)
}

/// `POST /api/messages` handler: reads the body, validates it, then sends it.
///
/// The route disables axum's `DefaultBodyLimit`; the body is read here with
/// [`to_bytes`] against [`AppState::message_body_limit`] so that exceeding the
/// limit is answered with the shared JSON envelope (`413 payload_too_large`)
/// rather than axum's plain-text rejection. A malformed JSON body is a `400`.
async fn send_message(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, SendError> {
    let body = to_bytes(request.into_body(), state.message_body_limit())
        .await
        .map_err(|error| {
            SendError::PayloadTooLarge(format!("request body exceeds the limit: {error}"))
        })?;

    let dto: SendMessageDto = serde_json::from_slice(&body)
        .map_err(|error| SendError::InvalidRequest(error.to_string()))?;

    let message = dto.into_outgoing(&state.default_from, state.max_attachment_bytes)?;
    message.validate()?;
    state
        .mailer
        .send(message)
        .await
        .map_err(|error| SendError::Smtp(error.public_message().to_string()))?;

    Ok((StatusCode::OK, Json(SentResponse { status: "sent" })).into_response())
}

/// Authorization middleware: rejects requests without a valid `Bearer` API key.
async fn require_api_key(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if is_authorized(&request, &state.api_key_hash) {
        next.run(request).await
    } else {
        Unauthorized.into_response()
    }
}

/// Returns `true` when the request carries the expected key as a `Bearer` token.
///
/// The token is hashed to a fixed-size digest and compared against the expected
/// digest in constant time.
fn is_authorized(request: &Request, expected_hash: &[u8; 32]) -> bool {
    let Some(value) = request.headers().get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(token) = bearer_token(value) else {
        return false;
    };
    let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    candidate.ct_eq(expected_hash).into()
}

/// Extracts the token from an `Authorization` header value using the `Bearer`
/// scheme (matched case-insensitively).
///
/// The scheme and the token may be separated by one or more ASCII spaces or
/// tabs; trailing separators are ignored. Whitespace inside the token is
/// rejected, and an empty token is not accepted.
fn bearer_token(value: &str) -> Option<&str> {
    let value = value.trim();
    let (scheme, rest) = value.split_once([' ', '\t'])?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = rest.trim_matches([' ', '\t']);
    if token.is_empty() || token.contains([' ', '\t']) {
        return None;
    }
    Some(token)
}

/// Builds the application [`Router`] with all routes mounted.
///
/// The returned router is self-contained and can be exercised in tests via
/// `tower::ServiceExt::oneshot` without binding a socket. `GET /api/health`
/// stays public; every other route sits behind [`require_api_key`].
pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/whoami", get(whoami))
        .route("/api/account", get(account))
        .route("/api/imap/status", get(imap_status))
        .route("/api/mailboxes", get(list_mailboxes))
        .route("/api/mailboxes/select", post(select_mailbox))
        .route(
            "/api/messages/{uid}",
            get(fetch_message).delete(delete_message),
        )
        .route("/api/messages/{uid}/flags", patch(update_flags))
        .route("/api/messages/{uid}/move", post(move_message))
        .route("/api/messages/{uid}/copy", post(copy_message))
        .route("/api/messages/{uid}/body", get(get_message_body))
        .route("/api/messages/{uid}/attachments", get(list_attachments))
        .route("/api/messages/{uid}/attachments/{id}", get(get_attachment))
        // The GET listing and the POST sending share the path; the send handler
        // enforces its own body limit (and its JSON `413` envelope), so axum's
        // default rejection must not pre-empt it.
        .route(
            "/api/messages",
            get(list_messages)
                .post(send_message)
                .layer(DefaultBodyLimit::disable()),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key,
        ));

    Router::new()
        .route("/api/health", get(health))
        .merge(protected)
        .with_state(state)
}
