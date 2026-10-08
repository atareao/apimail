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

use axum::body::to_bytes;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::Config;
use crate::smtp::{
    MailSender, MessageError, OutgoingAttachment, OutgoingMessage, SmtpError, SmtpSender,
};

/// Application name, resolved from `Cargo.toml`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
/// Application version, resolved from `Cargo.toml`.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Slack added to the computed body limit, on top of the base64 expansion of the
/// attachment bytes, to cover the rest of the JSON envelope.
const BODY_LIMIT_SLACK: usize = 64 * 1024;

/// Shared state exposed to handlers.
///
/// The configured API key is stored only as its SHA-256 digest and the field is
/// private, so the plaintext secret can neither be read nor formatted. The
/// manual [`Debug`] implementation redacts the digest as well as the mailer,
/// which holds the SMTP credentials.
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
    /// Sender used when a message omits `from` (the configured SMTP user).
    default_from: String,
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
            .finish()
    }
}

impl AppState {
    /// Builds the application state from the loaded [`Config`].
    ///
    /// This constructs the real SMTP transport from `config.account.smtp` — it
    /// opens no connection, but building it (or resolving its TLS configuration)
    /// can fail, hence the `Result`.
    ///
    /// The expected key is hashed to a fixed-size digest here (not on every
    /// request) so the middleware can compare two equal-length buffers in
    /// constant time. [`Config`] guarantees a non-empty key; the assertion below
    /// documents that invariant in debug builds.
    pub fn from_config(config: &Config) -> Result<Self, SmtpError> {
        debug_assert!(
            !config.api_key.is_empty(),
            "AppState must not be built with an empty API key"
        );
        let mailer: Arc<dyn MailSender> =
            Arc::new(SmtpSender::from_endpoint(&config.account.smtp)?);
        Ok(Self::with_mailer(config, mailer))
    }

    /// Builds the application state with an injected [`MailSender`].
    ///
    /// Used by tests to avoid the network and by [`from_config`](Self::from_config)
    /// to wrap the real [`SmtpSender`].
    pub fn with_mailer(config: &Config, mailer: Arc<dyn MailSender>) -> Self {
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
            default_from: config.account.smtp.username.clone(),
        }
    }

    /// Maximum total size, in bytes, of the decoded attachments of a message.
    pub fn max_attachment_bytes(&self) -> usize {
        self.max_attachment_bytes
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
    ) -> Result<OutgoingMessage, MessageError> {
        if self.to.is_empty() {
            return Err(MessageError::MissingRecipient);
        }

        let mut total = 0usize;
        let mut attachments = Vec::with_capacity(self.attachments.len());
        for attachment in self.attachments {
            let data = STANDARD
                .decode(attachment.data_base64.as_bytes())
                .map_err(|_| MessageError::InvalidBase64 {
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
            return Err(MessageError::TooLarge { total, limit });
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
impl From<MessageError> for SendError {
    fn from(error: MessageError) -> Self {
        match error {
            MessageError::TooLarge { .. } => Self::PayloadTooLarge(error.to_string()),
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
        .map_err(|error| SendError::Smtp(error.to_string()))?;

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
        // The handler enforces its own body limit (and its JSON `413` envelope),
        // so axum's default rejection must not pre-empt it.
        .route(
            "/api/messages",
            post(send_message).layer(DefaultBodyLimit::disable()),
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
