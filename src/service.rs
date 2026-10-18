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
//! `whoami`, `account` and `imap_status`; the rest still hold their own logic
//! and their own per-route error type in [`crate::http`].
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

use serde::Serialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::Config;
use crate::config::TlsMode;
use crate::idle::{
    HttpWebhookSender, IdleConnector, IdleSettings, IdleSupervisor, TokioIdleConnector,
    WebhookSender,
};
use crate::imap::{
    Backoff, ConnectionManager, ImapConnector, ImapError, MailboxInfo, TokioImapConnector,
};
use crate::queue::{NotificationQueue, QueueError, WebhookQueue};
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
/// [`AppState::verify_token`] is the only way to compare a token against it. The
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
/// Today this enum only holds the variants the operations owned by this module
/// can produce: `whoami` and `account` cannot fail, and `imap_status` can only
/// fail with `imap_unavailable`. The remaining rows of the module's error table
/// are added, one operation at a time, as their handlers migrate.
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

    /// Stable, machine-filterable `error` code of the shared envelope.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ImapUnavailable { .. } => "imap_unavailable",
        }
    }

    /// Stable, credential-free description, safe to expose to a caller.
    ///
    /// It never propagates third-party text directly: the underlying
    /// [`ImapError::public_message`] already groups its variants into stable
    /// families, and the message is fixed at construction time.
    pub fn public_message(&self) -> &'static str {
        match self {
            Self::ImapUnavailable { message, .. } => message,
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
/// produced by `SELECT` alone, so no `404` can originate here: every failure of
/// the underlying session is an availability failure and is flattened into one
/// code, exactly as the published error table requires.
///
/// # Errors
///
/// [`ServiceError::ImapUnavailable`] (`503`), carrying the credential-free
/// [`ImapError::public_message`] of the underlying failure and the failure itself
/// as its `source`.
pub async fn list_mailboxes(state: &AppState) -> Result<MailboxListResponse, ServiceError> {
    let mailboxes = state
        .imap
        .list_mailboxes()
        .await
        .map_err(ServiceError::imap_unavailable)?;
    Ok(MailboxListResponse {
        mailboxes: mailboxes.into_iter().map(MailboxDto::from).collect(),
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
}
