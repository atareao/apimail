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
//! `require_api_key` middleware. A request must carry the configured API key
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
//! solely on the input the attacker already controls. This module owns parsing
//! the header; the comparison itself is `AppState::verify_token`, in the
//! service layer, so a second transport authenticates against the same digest
//! instead of re-implementing it.
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
//! `POST /api/messages` is protected by `require_api_key` and accepts an
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
//! SMTP user. Attachment content is base64-encoded and is decoded by the
//! operation that sends it ([`crate::service::send_message`]), not at the HTTP
//! boundary: the total **decoded** size is what must not exceed
//! `APIMAIL_MAX_ATTACHMENT_BYTES`, and that rule has to hold for every transport.
//! Every address is validated as a mailbox. On success the route returns HTTP
//! `200`, `Content-Type: application/json` and `{"status":"sent"}`. Failures use
//! the `{"error":...,"message":...}` pattern:
//!
//! - `400` (`invalid_request`) — malformed JSON, missing recipients, invalid
//!   address or invalid base64.
//! - `413` (`payload_too_large`) — the raw request body exceeds the per-route
//!   body limit, or the decoded attachments exceed the attachment limit.
//! - `502` (`smtp_error`) — the SMTP server rejects or fails the delivery.
//!
//! The `413` has **two producers and they are not interchangeable**: the *raw
//! body* limit below is the transport's, and the limit over the *decoded*
//! attachments is the service's ([`crate::service::ServiceError::PayloadTooLarge`]),
//! rendered with the very same envelope. The same is true of the `400`: a body
//! that is not JSON at all is the transport's, while a decoded body that breaks a
//! domain rule (no recipient, an address that is not a mailbox, invalid base64)
//! is the service's ([`crate::service::ServiceError::InvalidRequest`]). Both are
//! documented in the error table of [`crate::service`].
//!
//! The per-route body limit is **not** enforced by axum's `DefaultBodyLimit`
//! (which would answer with a plain-text `413`): the route disables it and the
//! handler reads the body with [`to_bytes`] against
//! [`AppState::message_body_limit`], so even an oversized body gets the shared
//! JSON error envelope. The body limit is
//! `max_attachment_bytes * 4 / 3 + BODY_LIMIT_SLACK` (base64 expansion plus a
//! small, documented slack for the rest of the JSON); the authoritative
//! *decoded* check is still [`AppState::max_attachment_bytes`], owned by the
//! service operation.
//!
//! Requests for unknown routes still return `404`, and requests using an
//! unhandled method (for example `POST /api/health`) still return `405`; the
//! authentication middleware is layered only over the protected sub-router.
//!
//! # IMAP status contract
//!
//! `GET /api/imap/status` is protected by `require_api_key`. It attempts to
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
//! `GET /api/mailboxes` is protected by `require_api_key`. It lists the
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
//! `POST /api/mailboxes/select` is protected by `require_api_key` and accepts
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
//!   message is the stable [`public_message`](crate::imap::ImapError::public_message)
//!   and never [`Display`](std::fmt::Display) text from a third party.
//! - `401` (`unauthorized`) — missing or invalid API key.
//!
//! # Message reading contract
//!
//! `GET /api/messages` and `GET /api/messages/{uid}` are protected by
//! `require_api_key`. Both select the mailbox named by the required `mailbox`
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
//! by `require_api_key`. All four select the mailbox named by the required
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
//! `GET /api/messages/{uid}/attachments/{id}` are protected by `require_api_key`.
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
//!
//! # IDLE contract
//!
//! `POST /api/idle/start`, `POST /api/idle/stop`, `GET /api/idle/status` and
//! `GET /api/idle/queue` are protected by `require_api_key`. They control the
//! IDLE subscription, which watches a single configured mailbox
//! (`APIMAIL_IDLE_MAILBOX`, default `INBOX`) over its own dedicated IMAP
//! connection and POSTs every newly arrived message to `APIMAIL_WEBHOOK_URL`,
//! without interfering with the shared session used by the other routes.
//!
//! - `POST /api/idle/start` starts the subscription and returns HTTP `200`,
//!   `Content-Type: application/json` and
//!   `{"status":"running","mailbox":"INBOX","last_error":null}`. It is
//!   idempotent: starting an already-running subscription returns the same body
//!   without opening a second connection.
//! - `POST /api/idle/stop` stops the subscription and returns HTTP `200` with
//!   `{"status":"stopped",...}`; stopping when it is not running is a no-op.
//! - `GET /api/idle/status` returns HTTP `200` with the current `status`
//!   (`"running"` or `"stopped"`), the watched `mailbox` and `last_error` (a
//!   stable code such as `"imap_unavailable"`/`"webhook_failed"`, or `null`).
//! - `GET /api/idle/queue` returns HTTP `200` with the delivery queue's
//!   observable state: `persistent` (whether the queue is backed by
//!   `APIMAIL_QUEUE_PATH`), `pending` (notifications still undelivered),
//!   `delivered`, `dropped` (evicted because a limit was exceeded), `failed`
//!   (discarded after bounded attempts) and `oldest_pending_secs` (the age, in
//!   seconds, of the oldest pending notification, or `null` when none is
//!   pending).
//!
//! Failures use the shared `{"error":...,"message":...}` envelope:
//!
//! - `501` (`idle_not_configured`) — `start` was requested without a configured
//!   `APIMAIL_WEBHOOK_URL`; no connection is opened.
//! - `401` (`unauthorized`) — missing or invalid API key.

use axum::body::{Bytes, to_bytes};
use axum::extract::{DefaultBodyLimit, Path, RawQuery, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::idle::IdleStatus;
use crate::imap::{FetchFormat, SearchCriteria, SearchDate, SystemFlag};
use crate::queue::QueueStats;

// The application state and its construction error belong to the service layer
// (`crate::service`); the re-export keeps the historical `crate::http::AppState`
// path — and the intra-doc links of this module — working.
use crate::service::{self, ServiceError};
pub use crate::service::{APP_NAME, APP_VERSION, AppState, AppStateError};

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

/// JSON payload returned by a failing `GET /api/imap/status`.
///
/// The success DTO lives in the service layer
/// ([`crate::service::ImapStatusResponse`]); this one is the transport's
/// rendering of a [`ServiceError::ImapUnavailable`], which is why it stays here:
/// no other route answers `503` with a `connected` field.
#[derive(Debug, Serialize)]
struct ImapUnavailableResponse {
    /// Always `false`.
    connected: bool,
    /// Stable machine-readable error code.
    error: &'static str,
    /// Human-readable explanation.
    message: String,
}

/// `POST /api/mailboxes/select` request body.
#[derive(Debug, Deserialize)]
struct SelectMailboxDto {
    /// Mailbox to select.
    mailbox: String,
}

/// Failure modes of `POST /api/mailboxes/select`, mapped to the documented
/// statuses.
///
/// The transport owns one of them — the `400` of a request that does not speak
/// HTTP's dialect (a malformed JSON body, a blank `mailbox`) — plus the wrapper
/// ([`Domain`](Self::Domain)) around a [`ServiceError`] of the migrated
/// operation, which is rendered by the shared `IntoResponse` implementation of
/// the service layer instead of a second envelope built here.
///
/// The `404`/`501`/`503` of the route are the service's and travel here as a
/// [`Domain`](Self::Domain) value: the transport never decides their status, nor
/// rewrites their message.
#[derive(Debug)]
enum SelectMailboxError {
    /// The body is not a decodable `SelectMailboxDto`, or its `mailbox` is blank
    /// (`400`).
    InvalidRequest(String),
    /// A domain failure of the operation, rendered by the service layer.
    Domain(ServiceError),
}

impl IntoResponse for SelectMailboxError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            Self::InvalidRequest(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
            // One single rendering of a domain failure: the shared envelope of the
            // service layer, never a second copy of it.
            Self::Domain(error) => return error.into_response(),
        };
        (status, Json(ErrorResponse { error, message })).into_response()
    }
}

/// Maps a [`ServiceError`] into the select route's failure model: the domain
/// failure travels untouched and is rendered by the service layer, so the thin
/// handler only has to propagate it with `?`.
impl From<ServiceError> for SelectMailboxError {
    fn from(error: ServiceError) -> Self {
        Self::Domain(error)
    }
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
///
/// The transport owns one of them — the `400` of a request that does not speak
/// HTTP's dialect (a malformed `uid`, query string or body) — plus the wrapper
/// ([`Domain`](Self::Domain)) around a [`ServiceError`] of the operation behind
/// each handler, which is rendered by the shared `IntoResponse` implementation of
/// the service layer instead of a second envelope built here.
///
/// Every domain failure of a message route — the two `404`s, the `501` of a
/// missing extension and the `503` — travels here as a [`Domain`](Self::Domain)
/// value: the transport never decides their status, nor rewrites their message.
/// The per-route variants that used to carry them disappeared with the last
/// producer, `DELETE /api/messages/{uid}`.
#[derive(Debug)]
enum MessageError {
    /// The request is malformed (`400`).
    InvalidRequest(String),
    /// A domain failure of an operation, rendered by the service layer.
    Domain(ServiceError),
}

impl IntoResponse for MessageError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            Self::InvalidRequest(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
            // One single rendering of a domain failure: the shared envelope of the
            // service layer, never a second copy of it.
            Self::Domain(error) => return error.into_response(),
        };
        (status, Json(ErrorResponse { error, message })).into_response()
    }
}

/// Maps a [`ServiceError`] into the transport's failure model: the domain failure
/// travels untouched and is rendered by the service layer, so a thin handler
/// only has to propagate it with `?`.
impl From<ServiceError> for MessageError {
    fn from(error: ServiceError) -> Self {
        Self::Domain(error)
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
///
/// The transport owns two of them — the `413` of the **raw HTTP request body**
/// and the `400` of a body that is not a [`SendMessageDto`](service::SendMessageDto)
/// at all — plus the wrapper ([`Domain`](Self::Domain)) around the
/// [`ServiceError`] of the operation, which is rendered by the shared
/// `IntoResponse` implementation of the service layer instead of a second
/// envelope built here.
///
/// The other failures of the route — an unparsable payload that breaks a domain
/// rule (`400`), the **decoded** attachments exceeding their limit (`413`) and a
/// failed delivery (`502`) — are the service's, and travel here as a
/// [`Domain`](Self::Domain) value: the transport never decides their status, nor
/// rewrites their message.
#[derive(Debug)]
enum SendError {
    /// The body is not a decodable `SendMessageDto` (`400`).
    InvalidRequest(String),
    /// The raw request body exceeds the per-route body limit (`413`); the decoded
    /// attachments have their own `413`, owned by the service.
    PayloadTooLarge(String),
    /// A domain failure of the operation, rendered by the service layer.
    Domain(ServiceError),
}

impl IntoResponse for SendError {
    fn into_response(self) -> Response {
        let (status, error, message) = match self {
            Self::InvalidRequest(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
            Self::PayloadTooLarge(message) => {
                (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", message)
            }
            // One single rendering of a domain failure: the shared envelope of the
            // service layer, never a second copy of it.
            Self::Domain(error) => return error.into_response(),
        };
        (status, Json(ErrorResponse { error, message })).into_response()
    }
}

/// Maps a [`ServiceError`] into the send route's failure model: the domain failure
/// travels untouched and is rendered by the service layer, so the thin handler
/// only has to propagate it with `?`.
impl From<ServiceError> for SendError {
    fn from(error: ServiceError) -> Self {
        Self::Domain(error)
    }
}

/// Single point of truth mapping a [`ServiceError`] to the HTTP status of the
/// published error table (see the table in [`crate::service`]).
///
/// The status of every domain failure is decided here, in one place, and the
/// match is **exhaustive** on purpose: adding a variant to [`ServiceError`] is a
/// compile error until its status is chosen, so no failure mode can silently
/// inherit the wrong one. The body is the shared envelope built by the
/// [`IntoResponse`] implementation below, with one deliberate exception: the
/// `503` of `GET /api/imap/status` also carries `"connected": false`, and that
/// route renders itself.
fn status_for(error: &ServiceError) -> StatusCode {
    match error {
        ServiceError::ImapUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
        ServiceError::CapabilityNotSupported { .. } => StatusCode::NOT_IMPLEMENTED,
        ServiceError::MailboxNotFound
        | ServiceError::MessageNotFound
        | ServiceError::AttachmentNotFound => StatusCode::NOT_FOUND,
        ServiceError::MessageTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        ServiceError::MessageNotParsable { .. } => StatusCode::UNPROCESSABLE_ENTITY,
        ServiceError::InvalidRequest { .. } => StatusCode::BAD_REQUEST,
        ServiceError::PayloadTooLarge { .. } => StatusCode::PAYLOAD_TOO_LARGE,
        ServiceError::SmtpFailure { .. } => StatusCode::BAD_GATEWAY,
        // A `STORE` that cannot be rendered is the second domain producer of
        // `invalid_request`, next to the send operation's.
        ServiceError::InvalidFlags { .. } => StatusCode::BAD_REQUEST,
        ServiceError::IdleNotConfigured => StatusCode::NOT_IMPLEMENTED,
    }
}

/// Renders a [`ServiceError`] as the shared `{"error":…,"message":…}` envelope.
///
/// This is the **one** place where a domain failure becomes an HTTP response, so
/// a migrating handler only has to propagate the error (`?`, or `ServiceError` in
/// its return type) instead of rebuilding the envelope by hand. The status comes
/// from `status_for`; the body is the pair [`ServiceError::kind`] plus
/// [`ServiceError::public_message`], which is transport-agnostic — MCP will read
/// the very same two values.
///
/// A route that needs a *different* body — `GET /api/imap/status` adds
/// `"connected": false` to its `503` — keeps its own rendering on purpose rather
/// than growing a flag here.
impl IntoResponse for ServiceError {
    fn into_response(self) -> Response {
        let status = status_for(&self);
        let body = Json(ErrorResponse {
            error: self.kind(),
            message: self.public_message().to_string(),
        });
        (status, body).into_response()
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

/// `GET /api/whoami` handler: thin wrapper over [`crate::service::whoami`].
async fn whoami() -> Json<service::WhoamiResponse> {
    Json(service::whoami())
}

/// `GET /api/account` handler: thin wrapper over [`crate::service::account`].
async fn account(State(state): State<AppState>) -> Response {
    Json(service::account(&state)).into_response()
}

/// `GET /api/imap/status` handler: thin wrapper over
/// [`crate::service::imap_status`], which ensures a live IMAP session and
/// reports the outcome without exposing any credential.
///
/// The failure branch is the one route that renders its own body instead of the
/// shared [`ServiceError`] envelope: it adds `"connected": false`. Its status
/// still comes from [`status_for`], and its `error`/`message` from the service.
async fn imap_status(State(state): State<AppState>) -> Response {
    match service::imap_status(&state).await {
        Ok(status) => (StatusCode::OK, Json(status)).into_response(),
        Err(error) => (
            status_for(&error),
            Json(ImapUnavailableResponse {
                connected: false,
                error: error.kind(),
                message: error.public_message().to_string(),
            }),
        )
            .into_response(),
    }
}

/// `GET /api/mailboxes` handler: thin wrapper over
/// [`crate::service::list_mailboxes`].
///
/// The failure branch is rendered by the shared [`ServiceError`] envelope; only
/// `503 imap_unavailable` can originate here (the operation never selects a
/// mailbox).
async fn list_mailboxes(
    State(state): State<AppState>,
) -> Result<Json<service::MailboxListResponse>, ServiceError> {
    Ok(Json(service::list_mailboxes(&state).await?))
}

/// `POST /api/mailboxes/select` handler: thin wrapper over
/// [`crate::service::select_mailbox`], which selects a mailbox and reports its
/// status.
///
/// The body is read as raw bytes and parsed here so a malformed payload gets
/// the shared JSON error envelope (`400 invalid_request`) instead of axum's
/// plain-text rejection. A missing or blank (`trim`-empty) `mailbox` is also
/// rejected with a `400`. The `404` of a missing mailbox and the `503` of an
/// unavailable session are the service's and are rendered by its shared
/// `IntoResponse` implementation.
async fn select_mailbox(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, SelectMailboxError> {
    // A fixed message: the serde error could echo part of the attacker-supplied
    // payload, so it must not be reflected back to the client.
    let dto: SelectMailboxDto = serde_json::from_slice(&body)
        .map_err(|_| SelectMailboxError::InvalidRequest("invalid JSON body".to_string()))?;

    let mailbox = dto.mailbox.trim();
    if mailbox.is_empty() {
        return Err(SelectMailboxError::InvalidRequest(
            "`mailbox` must not be empty".to_string(),
        ));
    }

    let response = service::select_mailbox(&state, mailbox.to_string()).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages` handler: thin wrapper over
/// [`crate::service::list_messages`].
///
/// The query string is decoded with [`RawQuery`] + `serde_urlencoded` so every
/// rejection is the shared JSON `400` envelope; the service then runs the search
/// and the fetch of one page on the single live session.
async fn list_messages(
    State(state): State<AppState>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let query: ListMessagesQuery = parse_raw_query(raw)?;
    let (mailbox, criteria, limit, offset) = query.into_query()?;

    let response = service::list_messages(&state, mailbox, criteria, limit, offset).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages/{uid}` handler: thin wrapper over
/// [`crate::service::fetch_message`].
///
/// The `uid` is extracted as a string and parsed here so a non-numeric value is
/// answered with the shared JSON `400` instead of axum's plain-text rejection;
/// the service receives an already-typed, positive identifier.
async fn fetch_message(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let query: FetchMessageQuery = parse_raw_query(raw)?;
    let (mailbox, format) = query.into_query()?;

    let response = service::fetch_message(&state, mailbox, uid, format).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages/{uid}/body` handler: thin wrapper over
/// [`crate::service::message_body`].
///
/// The `uid` path parameter and the `mailbox` query parameter are validated
/// first (shared JSON `400` envelope), so no IMAP command is sent for an invalid
/// request. Oversized and unparsable messages are reported by the service and
/// rendered as `413`/`422` by the shared [`ServiceError`] envelope.
async fn get_message_body(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;

    let response = service::message_body(&state, mailbox, uid).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages/{uid}/attachments` handler: thin wrapper over
/// [`crate::service::list_attachments`], which parses the message and reports the
/// metadata of each attachment, without their content.
async fn list_attachments(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;

    let response = service::list_attachments(&state, mailbox, uid).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /api/messages/{uid}/attachments/{id}` handler: thin wrapper over
/// [`crate::service::attachment`], which returns the decoded content of the
/// attachment identified by the positional `id`, base64-encoded inside the JSON
/// body.
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

    let response = service::attachment(&state, mailbox, uid, id).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `PATCH /api/messages/{uid}/flags` handler: thin wrapper over
/// [`crate::service::update_flags`].
///
/// The `uid` path parameter and the `mailbox` query parameter are validated
/// first (shared JSON `400` envelope). The body is read as raw bytes and parsed
/// here so a malformed payload or an unknown flag is a `400` rather than axum's
/// plain-text rejection; validation rejects an empty or contradictory update
/// **before** any IMAP command is sent. The domain outcomes — the two `404`s, the
/// `400 invalid_request` of a `STORE` that cannot be rendered and the `503` — are
/// the service's and are rendered by its shared `IntoResponse` implementation.
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

    let response = service::update_flags(&state, mailbox, uid, add, remove).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `POST /api/messages/{uid}/move` handler: thin wrapper over
/// [`crate::service::move_message`].
///
/// The `uid`, the `mailbox` and the body's `to` are validated here (shared JSON
/// `400` envelope); the move itself — `UID MOVE`, or the `UIDPLUS` emulation —
/// and its domain outcomes (the two `404`s, the `501` of a server that announces
/// neither capability and the `503`) belong to the operation.
async fn move_message(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;
    let to = parse_body_destination(&body)?;

    let response = service::move_message(&state, mailbox, uid, to).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `POST /api/messages/{uid}/copy` handler: thin wrapper over
/// [`crate::service::copy_message`].
///
/// Same split as [`move_message`]: the request's own validation stays here and
/// every domain outcome — the two `404`s and the `503` — is the operation's.
async fn copy_message(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
    body: Bytes,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;
    let to = parse_body_destination(&body)?;

    let response = service::copy_message(&state, mailbox, uid, to).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `DELETE /api/messages/{uid}` handler: thin wrapper over
/// [`crate::service::delete_message`].
///
/// The `uid` and the `mailbox` are validated here (shared JSON `400` envelope);
/// the deletion itself — the purge the operation owns, which requires `UIDPLUS`
/// and never falls back to a global `EXPUNGE` — and its domain outcomes (the two
/// `404`s, the `501` of a server that does not announce the extension and the
/// `503`) belong to the operation.
async fn delete_message(
    State(state): State<AppState>,
    Path(uid): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, MessageError> {
    let uid = parse_uid(&uid)?;
    let mailbox = parse_mailbox(parse_raw_query::<MailboxQuery>(raw)?.mailbox)?;

    let response = service::delete_message(&state, mailbox, uid).await?;
    Ok((StatusCode::OK, Json(response)).into_response())
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

/// `POST /api/messages` handler: reads and decodes the body, then sends it.
///
/// A **thin wrapper** over [`service::send_message`], which owns every rule of an
/// outgoing message (recipients, addresses, base64 attachments and their decoded
/// size) so a second transport inherits them instead of re-implementing them.
/// What stays here is exactly what only an HTTP request has: its own body and the
/// `serde` dialect of that body.
///
/// The route disables axum's `DefaultBodyLimit`; the body is read here with
/// [`to_bytes`] against [`AppState::message_body_limit`] so that exceeding the
/// limit is answered with the shared JSON envelope (`413 payload_too_large`)
/// rather than axum's plain-text rejection — that limit exists only because an
/// HTTP request *has* a body, which is why it is the transport's `413` and not
/// the service's. A body that is not a
/// [`SendMessageDto`](service::SendMessageDto) is likewise the transport's `400`.
/// The `200` body is the operation's own [`service::SentResponse`].
async fn send_message(
    State(state): State<AppState>,
    request: Request,
) -> Result<Response, SendError> {
    let body = to_bytes(request.into_body(), state.message_body_limit())
        .await
        .map_err(|error| {
            SendError::PayloadTooLarge(format!("request body exceeds the limit: {error}"))
        })?;

    let dto: service::SendMessageDto = serde_json::from_slice(&body)
        .map_err(|error| SendError::InvalidRequest(error.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(service::send_message(&state, dto).await?),
    )
        .into_response())
}

/// `POST /api/idle/start` handler: thin wrapper over
/// [`crate::service::idle_start`].
///
/// Idempotent and safe against concurrent calls; the domain operation returns the
/// resulting status. Without a configured webhook it answers `501`
/// `idle_not_configured` through the shared [`ServiceError`] envelope.
async fn idle_start(State(state): State<AppState>) -> Result<Response, ServiceError> {
    let status = service::idle_start(&state).await?;
    Ok((StatusCode::OK, Json(status)).into_response())
}

/// `POST /api/idle/stop` handler: thin wrapper over
/// [`crate::service::idle_stop`].
///
/// Idempotent: stopping a subscription that is not running is a no-op, and the
/// operation is infallible, so there is no failure branch.
async fn idle_stop(State(state): State<AppState>) -> Response {
    let status = service::idle_stop(&state).await;
    (StatusCode::OK, Json(status)).into_response()
}

/// `GET /api/idle/status` handler: thin wrapper over
/// [`crate::service::idle_status`].
///
/// Infallible, so there is no failure branch; the DTO is the one of the domain
/// module and no envelope is involved.
async fn idle_status(State(state): State<AppState>) -> Json<IdleStatus> {
    Json(service::idle_status(&state).await)
}

/// `GET /api/idle/queue` handler: thin wrapper over
/// [`crate::service::idle_queue`].
///
/// [`QueueStats`](crate::queue::QueueStats) already derives `Serialize`, so it is
/// returned verbatim: `persistent`, `pending`, `delivered`, `dropped`, `failed`
/// and `oldest_pending_secs`.
async fn idle_queue(State(state): State<AppState>) -> Json<QueueStats> {
    Json(service::idle_queue(&state))
}

/// Authorization middleware: rejects requests without a valid `Bearer` API key.
async fn require_api_key(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if is_authorized(&request, &state) {
        next.run(request).await
    } else {
        Unauthorized.into_response()
    }
}

/// Returns `true` when the request carries the expected key as a `Bearer` token.
///
/// This function owns what is HTTP dialect — the `Authorization` header and the
/// `Bearer` scheme, including the whitespace rules — and delegates the
/// comparison itself to [`AppState::verify_token`], which lives in the service
/// layer so that every transport authenticates against the same digest.
fn is_authorized(request: &Request, state: &AppState) -> bool {
    let Some(value) = request.headers().get(header::AUTHORIZATION) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(token) = bearer_token(value) else {
        return false;
    };
    state.verify_token(token)
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
/// stays public; every other route sits behind `require_api_key`.
///
/// When the application state carries an MCP path (`Some`, i.e. the
/// configuration enabled the Streamable HTTP transport) the `rmcp` service is
/// nested **inside** the protected sub-router, before `route_layer`, so the
/// endpoint shares this listener and this middleware instead of opening a second
/// port or a second authentication path. With `None` — always the case without
/// the `mcp` feature — no MCP route exists at all and the path answers `404`.
pub fn build_router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/whoami", get(whoami))
        .route("/api/account", get(account))
        .route("/api/imap/status", get(imap_status))
        .route("/api/idle/start", post(idle_start))
        .route("/api/idle/stop", post(idle_stop))
        .route("/api/idle/status", get(idle_status))
        .route("/api/idle/queue", get(idle_queue))
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
        );

    // The MCP Streamable HTTP transport joins the *protected* sub-router, not the
    // root one: nesting it here, before `route_layer`, is what makes the existing
    // `require_api_key` middleware cover it too — the same listener, the same
    // authentication, no second port.
    #[cfg(feature = "mcp")]
    let protected = match state.mcp_path() {
        Some(path) => {
            protected.nest_service(path, crate::mcp::streamable_http_service(state.clone()))
        }
        None => protected,
    };

    let protected = protected.route_layer(middleware::from_fn_with_state(
        state.clone(),
        require_api_key,
    ));

    Router::new()
        .route("/api/health", get(health))
        .merge(protected)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imap::ImapError;
    use crate::smtp::SmtpError;

    #[test]
    fn app_state_error_public_message_never_leaks_third_party_text() {
        // SMTP delivery: the server response carries a host name that must not
        // reach the log-safe message.
        let smtp = AppStateError::Smtp(SmtpError::Delivery(
            "550 rejected by internal.example".into(),
        ));
        assert_eq!(smtp.public_message(), "SMTP delivery failed");
        assert!(!smtp.public_message().contains("internal.example"));
        assert_eq!(smtp.kind(), "smtp");

        // IMAP login: the third-party text embeds a user and a host name.
        let imap_login = AppStateError::Imap(ImapError::Login(
            "auth failed for user@internal.example".into(),
        ));
        assert_eq!(
            imap_login.public_message(),
            "IMAP authentication or command failed"
        );
        assert!(!imap_login.public_message().contains("internal.example"));
        assert_eq!(imap_login.kind(), "imap");

        // IMAP TLS: the inner text embeds a host name too.
        let imap_tls = AppStateError::Imap(ImapError::Tls(
            "certificate for internal.example invalid".into(),
        ));
        assert_eq!(imap_tls.public_message(), "IMAP TLS handshake failed");
        assert!(!imap_tls.public_message().contains("internal.example"));
        assert_eq!(imap_tls.kind(), "imap");
    }

    #[test]
    fn queue_error_public_message_names_the_variable_and_kind_is_stable() {
        let error = AppStateError::Queue(crate::queue::QueueError::Storage);
        assert_eq!(
            error.public_message(),
            "invalid APIMAIL_QUEUE_PATH: the queue file could not be created or written"
        );
        assert!(
            error.public_message().contains("APIMAIL_QUEUE_PATH"),
            "the message must name the offending variable"
        );
        assert_eq!(error.kind(), "queue");
    }

    /// Pins the shared `IntoResponse for ServiceError` rendering, which has **no
    /// consuming handler yet**: `whoami` and `account` cannot fail, and
    /// `imap_status` deliberately renders its own body, so nothing exercises this
    /// implementation in the request path. It is the scaffolding the ~70 migrated
    /// operations will rely on, hence it is verified directly here — status,
    /// `Content-Type` and the exact `{"error","message"}` envelope — instead of
    /// being trusted.
    ///
    /// The cause carries third-party text (a user, a host, an address); none of
    /// it may reach the body.
    #[tokio::test]
    async fn service_error_renders_the_shared_envelope_with_its_own_status() {
        let error = ServiceError::imap_unavailable(ImapError::Unavailable(
            "auth failed for user@internal.example at 10.0.0.7:993".to_string(),
        ));

        let response = error.into_response();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body should be readable");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON body");
        assert_eq!(
            json,
            serde_json::json!({
                "error": "imap_unavailable",
                "message": "could not connect to the IMAP server",
            }),
            "the envelope must be exactly the shared {{error, message}} pair"
        );
        assert!(
            !json.to_string().contains("internal.example"),
            "the rendering leaked third-party text: {json}"
        );
    }
}
