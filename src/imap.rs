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
            Self::Unavailable(_) => "unavailable",
            Self::Timeout(_) => "timeout",
            Self::Imap(_) => "imap",
            Self::Io(_) => "io",
        }
    }
}

/// A live, authenticated IMAP session able to run commands.
///
/// The trait intentionally exposes only what the connection layer needs to
/// verify liveness for now; mailbox operations will extend it in later changes.
pub trait ImapSession: Send {
    /// Sends a `NOOP`, used to check that the session is still alive.
    fn noop(&mut self) -> SendFuture<'_, Result<(), ImapError>>;
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

    /// Ensures a live session, connecting or reconnecting as needed.
    ///
    /// If a session is cached, it is probed with `NOOP`; a failure discards it.
    /// Then a fresh session is attempted up to `backoff.attempts` times, sleeping
    /// the backoff delay between attempts. Returns `Ok(())` once a live session is
    /// cached, or the last error after exhausting the attempts.
    pub async fn status(&self) -> Result<(), ImapError> {
        let mut guard = self.session.lock().await;

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
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A fake session whose `NOOP` outcome is scripted.
    struct FakeSession {
        /// When `true`, every `noop` fails.
        dead: bool,
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
    }

    /// A scripted outcome for a single `connect` call.
    enum Outcome {
        /// Hands back a session whose `noop` succeeds (`false`) or fails (`true`).
        Session(bool),
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
                    Some(Outcome::Session(dead)) => {
                        Ok(Box::new(FakeSession { dead }) as Box<dyn ImapSession>)
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
        let connector = FakeConnector::scripted([Outcome::Session(false)]);
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
        let connector = FakeConnector::scripted([Outcome::Session(true), Outcome::Session(false)]);
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
}
