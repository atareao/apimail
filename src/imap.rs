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
use async_imap::types::{Flag, NameAttribute};
use futures_util::TryStreamExt;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use crate::config::{Config, MailEndpoint, TlsMode};

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
            Self::Login(_) | Self::MailboxNotFound => false,
            Self::Imap(inner) => matches!(
                inner,
                async_imap::error::Error::Io(_) | async_imap::error::Error::ConnectionLost
            ),
        }
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
        let provider = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
        let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls_config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|error| ImapError::TlsConfig(error.to_string()))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Self {
            endpoint: config.account.imap.clone(),
            timeout: config.imap_timeout,
            tls_config: Arc::new(tls_config),
        })
    }

    /// Runs the whole connection and authentication sequence under a single
    /// timeout.
    async fn connect_inner(&self) -> Result<Box<dyn ImapSession>, ImapError> {
        let endpoint = &self.endpoint;
        let strategy = tls_strategy(endpoint.tls);
        let tcp = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
            .await
            .map_err(ImapError::Tcp)?;

        let stream = match strategy {
            TlsStrategy::Implicit => {
                let tls = self.upgrade(tcp, &endpoint.host).await?;
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
                let tls = self.upgrade(tcp, &endpoint.host).await?;
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
        Ok(Box::new(SessionHandle { session }))
    }

    /// Performs the TLS handshake over an already-connected TCP stream.
    async fn upgrade(&self, tcp: TcpStream, host: &str) -> Result<TlsStream<TcpStream>, ImapError> {
        let server_name = ServerName::try_from(host)
            .map_err(|_| ImapError::InvalidServerName(host.to_string()))?
            .to_owned();
        let connector = TlsConnector::from(Arc::clone(&self.tls_config));
        connector
            .connect(server_name, tcp)
            .await
            .map_err(|error| ImapError::Tls(error.to_string()))
    }
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
            Ok(MailboxStatus {
                exists: mailbox.exists,
                recent: mailbox.recent,
                unseen: mailbox.unseen,
                uid_validity: mailbox.uid_validity,
                uid_next: mailbox.uid_next,
                flags: mailbox.flags.iter().map(flag_label).collect(),
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
    async fn run_once<T, F>(&self, command: F) -> Result<T, ImapError>
    where
        F: for<'a> FnOnce(&'a mut Box<dyn ImapSession>) -> SendFuture<'a, Result<T, ImapError>>,
    {
        let mut guard = self.session.lock().await;
        self.ensure_session(&mut guard).await?;

        let outcome = match guard.as_mut() {
            Some(session) => command(session).await,
            None => Err(ImapError::Unavailable(
                "no IMAP session available".to_string(),
            )),
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
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// A fake session whose `NOOP`, `list_mailboxes` and `select` outcomes are
    /// scripted.
    #[derive(Clone)]
    struct FakeSession {
        /// When `true`, every `noop` fails.
        dead: bool,
        /// Mailboxes returned by `list_mailboxes`.
        mailboxes: Vec<MailboxInfo>,
        /// Outcome of `select`.
        select: SelectScript,
    }

    impl FakeSession {
        /// A healthy session with an empty mailbox list.
        fn healthy() -> Self {
            Self {
                dead: false,
                mailboxes: Vec::new(),
                select: SelectScript::Status(default_status()),
            }
        }

        /// A session whose `noop` fails.
        fn dead() -> Self {
            Self {
                dead: true,
                mailboxes: Vec::new(),
                select: SelectScript::Connection,
            }
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
}
