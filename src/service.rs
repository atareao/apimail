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
//! operations of this module.
//!
//! # Published HTTP error table
//!
//! This table is the contract of the API and is **unchanged** by the migration
//! of operations into this module. The `ServiceError` column says who can
//! produce each row *today*: operations already migrated here do it through
//! [`ServiceError`], while the rest still use their own per-route error type in
//! [`crate::http`] until they migrate, one operation at a time.
//!
//! | HTTP | `error`                | Produced today by                              |
//! |------|------------------------|------------------------------------------------|
//! | 401  | `unauthorized`         | `http::Unauthorized` (the auth middleware)     |
//! | 400  | `invalid_request`      | `http::MailboxError`, `http::MessageError`, `http::SendError` |
//! | 404  | `mailbox_not_found`    | `http::MailboxError`, `http::MessageError`     |
//! | 404  | `message_not_found`    | `http::MessageError`                           |
//! | 404  | `attachment_not_found` | `http::MessageError`                           |
//! | 413  | `message_too_large`    | `http::MessageError`                           |
//! | 413  | `payload_too_large`    | `http::SendError`                              |
//! | 422  | `message_not_parsable` | `http::MessageError`                           |
//! | 501  | `capability_not_supported` | `http::MessageError`                       |
//! | 501  | `idle_not_configured`  | `http::IdleHttpError`                          |
//! | 502  | `smtp_error`           | `http::SendError`                              |
//! | 503  | `imap_unavailable`     | [`ServiceError::ImapUnavailable`], `http::MailboxError`, `http::MessageError` |
//!
//! Two rows share the code `imap_unavailable` but not the same body: the 503 of
//! `GET /api/imap/status` also carries `"connected": false`, which is that
//! route's own rendering, so it stays in the transport.

use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Config;
use crate::config::TlsMode;
use crate::idle::{
    HttpWebhookSender, IdleConnector, IdleSettings, IdleSupervisor, TokioIdleConnector,
    WebhookSender,
};
use crate::imap::{Backoff, ConnectionManager, ImapConnector, ImapError, TokioImapConnector};
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
    ///
    /// Crate-visible, and no further: the HTTP auth middleware lives in another
    /// module but owns the header parsing, so it is the one that needs to
    /// compare this digest. It stays unreachable from outside the crate.
    pub(crate) api_key_hash: [u8; 32],
    /// Non-sensitive view of the configured mail account (host and port only).
    account: AccountView,
    /// Outgoing mail transport (real at runtime, fake in tests).
    pub(crate) mailer: Arc<dyn MailSender>,
    /// Maximum total size, in bytes, of the decoded attachments of a message.
    max_attachment_bytes: usize,
    /// Maximum size, in bytes, of a message that will be fetched and parsed.
    max_message_bytes: usize,
    /// Sender used when a message omits `from` (the configured SMTP user).
    pub(crate) default_from: String,
    /// Lazy, persistent IMAP connection manager.
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

/// Failure of an application operation.
///
/// The variant decides the stable `error` code (via [`code`](Self::code)) and
/// the message a transport may expose (via
/// [`public_message`](Self::public_message)); the status code is a transport
/// decision and is **not** modelled here, so the same failure can be rendered as
/// an HTTP response or as an MCP tool error.
///
/// Today this enum only holds the variants the operations owned by this module
/// can produce: `whoami` and `account` cannot fail, and `imap_status` can only
/// fail with `ImapUnavailable`. The remaining rows of the module's error table
/// are added, one operation at a time, as their handlers migrate. The HTTP
/// transport matches this enum **exhaustively** (no wildcard arm), so a new
/// variant is a compile error until it is mapped, and no failure mode can ever
/// be silently rendered with the wrong status.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The IMAP session or server is unavailable (`503`, `imap_unavailable`).
    #[error("{0}")]
    ImapUnavailable(String),
}

impl ServiceError {
    /// Stable, machine-filterable `error` code of the shared envelope.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ImapUnavailable(_) => "imap_unavailable",
        }
    }

    /// Stable, credential-free description, safe to expose to a caller.
    ///
    /// It never propagates third-party text directly: the underlying
    /// [`ImapError::public_message`] already groups its variants into stable
    /// families.
    pub fn public_message(&self) -> &str {
        match self {
            Self::ImapUnavailable(message) => message,
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
/// [`ServiceError::ImapUnavailable`], carrying the credential-free
/// [`ImapError::public_message`] of the underlying failure.
pub async fn imap_status(state: &AppState) -> Result<ImapStatusResponse, ServiceError> {
    state
        .imap
        .status()
        .await
        .map_err(|error| ServiceError::ImapUnavailable(error.public_message().to_string()))?;
    Ok(ImapStatusResponse {
        connected: true,
        host: state.account.imap.host.clone(),
        port: state.account.imap.port,
        tls: state.imap_tls.as_str(),
    })
}
