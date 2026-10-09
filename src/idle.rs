//! IDLE subscription abstraction and webhook payload.
//!
//! This module holds the pure, network-free pieces the IDLE supervisor (block 2)
//! is built on: the [`IdleSession`]/[`IdleConnector`] traits describing the
//! dedicated IMAP connection, the [`WebhookSender`] trait and its
//! [`WebhookError`], and the typed [`WebhookPayload`] that is POSTed for every
//! new message.
//!
//! Everything here is injectable and testable without a network: the payload is
//! built with [`WebhookPayload::from_message`] from an already-fetched
//! [`Message`] and, when available, its parsed MIME [`ParsedMessage`].

use std::fmt;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_imap::Session;
use async_imap::extensions::idle::IdleResponse;
use serde::Serialize;

use crate::config::MailEndpoint;
use crate::imap::{
    Address, Backoff, FetchFormat, ImapError, ImapStream, MailboxStatus, Message, MessageEnvelope,
    SendFuture, build_tls_config, connect_and_login, fetch_messages, mailbox_status,
};
use crate::mime::{ParsedAttachment, ParsedMessage};

/// Outcome of a bounded IDLE wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvent {
    /// The server reported some change in the mailbox.
    Changed,
    /// The timeout elapsed without a change.
    TimedOut,
}

/// The operations the IDLE supervisor needs from its dedicated IMAP session.
pub trait IdleSession: Send {
    /// Selects `mailbox` with `SELECT`, returning its status.
    fn select(&mut self, mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>>;

    /// Runs `IDLE` + a bounded wait + `DONE` internally; the session is free
    /// again once it resolves.
    fn wait_for_change(
        &mut self,
        timeout: Duration,
    ) -> SendFuture<'_, Result<IdleEvent, ImapError>>;

    /// Runs `UID FETCH <from_uid>:*` in the requested `format`.
    fn fetch_from(
        &mut self,
        from_uid: u32,
        format: FetchFormat,
    ) -> SendFuture<'_, Result<Vec<Message>, ImapError>>;
}

/// A factory able to open a dedicated IMAP connection for the subscription.
pub trait IdleConnector: Send + Sync {
    /// Establishes a new dedicated session, connecting and authenticating.
    fn connect(&self) -> SendFuture<'_, Result<Box<dyn IdleSession>, ImapError>>;
}

/// Failure of a delivery to the webhook.
///
/// The variants are intentionally coarse and free of third-party text: the
/// supervisor only needs to classify the failure as retryable or not.
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    /// The request never reached the webhook (DNS, TCP, TLS, timeout, …).
    #[error("webhook transport failed")]
    Transport,
    /// The webhook answered with a non-success HTTP status.
    #[error("webhook returned a non-success status")]
    Status(u16),
}

impl WebhookError {
    /// Whether the failure is worth retrying: a transport error or a `5xx`/`429`.
    ///
    /// Every other status (for example `4xx` other than `429`) is a definitive
    /// refusal and must not be retried.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport => true,
            Self::Status(code) => *code >= 500 || *code == 429,
        }
    }
}

/// Sends a notification to the configured webhook.
pub trait WebhookSender: Send + Sync {
    /// POSTs `payload` as JSON to `url`.
    ///
    /// The returned future borrows `self`, `url` and `payload` for the same
    /// lifetime, so an implementation never has to clone them.
    fn send<'a>(
        &'a self,
        url: &'a str,
        payload: &'a WebhookPayload,
    ) -> SendFuture<'a, Result<(), WebhookError>>;
}

/// JSON payload of a notification: the message metadata plus its parsed content.
///
/// Attachments are listed with their metadata only; their bytes are **never**
/// carried in the payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebhookPayload {
    /// Mailbox the message was delivered to.
    pub mailbox: String,
    /// Message UID within the mailbox.
    pub uid: u32,
    /// UID validity of the mailbox, when the server reported it.
    pub uid_validity: Option<u32>,
    /// Flags set on the message, in IMAP style (`\Seen`, …).
    pub flags: Vec<String>,
    /// `RFC822.SIZE`, when the server reported it.
    pub size: Option<u32>,
    /// `INTERNALDATE` in RFC 3339 form, when the server reported it.
    pub internal_date: Option<String>,
    /// Parsed envelope, when the message carried one.
    pub envelope: Option<EnvelopePayload>,
    /// Whether `text`/`html`/`attachments` were derived from a MIME parse.
    pub parsed: bool,
    /// Plain-text body, or `null` when not parsed.
    pub text: Option<String>,
    /// HTML body, or `null` when not parsed.
    pub html: Option<String>,
    /// Attachment metadata; empty when not parsed.
    pub attachments: Vec<AttachmentPayload>,
}

/// The subset of a message envelope carried in a [`WebhookPayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvelopePayload {
    /// `From` addresses.
    pub from: Vec<AddressPayload>,
    /// `To` addresses.
    pub to: Vec<AddressPayload>,
    /// `Cc` addresses.
    pub cc: Vec<AddressPayload>,
    /// Raw `Subject` header, if reported.
    pub subject: Option<String>,
    /// Raw `Date` header, if reported.
    pub date: Option<String>,
    /// `Message-ID`, if reported.
    pub message_id: Option<String>,
}

/// One envelope address carried in a [`WebhookPayload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddressPayload {
    /// Display name, if the server reported one.
    pub name: Option<String>,
    /// `mailbox@host`, or just the mailbox when no host was reported.
    pub address: Option<String>,
}

/// Metadata of one attachment carried in a [`WebhookPayload`].
///
/// The attachment's bytes are deliberately absent: only the metadata is
/// reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentPayload {
    /// Positional identifier within the parsed attachment list.
    pub id: u32,
    /// Decoded file name, if reported.
    pub filename: Option<String>,
    /// `"{type}/{subtype}"`, if reported.
    pub content_type: Option<String>,
    /// Length of the decoded content, in bytes.
    pub size: usize,
    /// Whether the part is marked `inline` by its `Content-Disposition`.
    pub inline: bool,
    /// `Content-ID`, if reported.
    pub content_id: Option<String>,
}

impl From<&Address> for AddressPayload {
    fn from(address: &Address) -> Self {
        Self {
            name: address.name.clone(),
            address: address.address.clone(),
        }
    }
}

impl From<&MessageEnvelope> for EnvelopePayload {
    fn from(envelope: &MessageEnvelope) -> Self {
        Self {
            from: envelope.from.iter().map(AddressPayload::from).collect(),
            to: envelope.to.iter().map(AddressPayload::from).collect(),
            cc: envelope.cc.iter().map(AddressPayload::from).collect(),
            subject: envelope.subject.clone(),
            date: envelope.date.clone(),
            message_id: envelope.message_id.clone(),
        }
    }
}

impl From<&ParsedAttachment> for AttachmentPayload {
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

impl WebhookPayload {
    /// Builds the payload from the message and, when available, its MIME parse.
    ///
    /// `parsed = Some(p)` → `parsed = true` with `text`/`html` and the attachment
    /// metadata taken from `p` (the attachments are listed **without** their
    /// content). `None` → `parsed = false` with `text`/`html` `null` and an empty
    /// `attachments` list, keeping the message metadata.
    pub fn from_message(
        mailbox: &str,
        uid_validity: Option<u32>,
        message: &Message,
        parsed: Option<&ParsedMessage>,
    ) -> Self {
        let (parsed_flag, text, html, attachments) = match parsed {
            Some(parsed) => (
                true,
                parsed.text.clone(),
                parsed.html.clone(),
                parsed
                    .attachments
                    .iter()
                    .map(AttachmentPayload::from)
                    .collect(),
            ),
            None => (false, None, None, Vec::new()),
        };

        Self {
            mailbox: mailbox.to_string(),
            uid: message.uid,
            uid_validity,
            flags: message.flags.clone(),
            size: message.size,
            internal_date: message.internal_date.clone(),
            envelope: message.envelope.as_ref().map(EnvelopePayload::from),
            parsed: parsed_flag,
            text,
            html,
            attachments,
        }
    }
}

/// Intervalo máximo entre reemisiones de `IDLE` (RFC 2177).
///
/// El servidor puede considerar inactivo a un cliente que mantenga un `IDLE`
/// abierto; reemitirlo antes de este intervalo evita que la sesión dedicada sea
/// cerrada por inactividad.
pub const IDLE_REISSUE_INTERVAL: Duration = Duration::from_secs(29 * 60);

/// Conexión IMAP dedicada real, sobre `async-imap`.
pub struct TokioIdleConnector {
    /// Endpoint IMAP (host, puerto, modo TLS y credenciales).
    endpoint: MailEndpoint,
    /// Timeout aplicado a la conexión y al `LOGIN` de la sesión dedicada.
    timeout: Duration,
}

impl TokioIdleConnector {
    /// Construcción **infalible** (la config rustls se resuelve al conectar).
    pub fn new(endpoint: MailEndpoint, timeout: Duration) -> Self {
        Self { endpoint, timeout }
    }
}

impl IdleConnector for TokioIdleConnector {
    fn connect(&self) -> SendFuture<'_, Result<Box<dyn IdleSession>, ImapError>> {
        Box::pin(async move {
            let tls_config = build_tls_config()?;
            let session =
                tokio::time::timeout(self.timeout, connect_and_login(&self.endpoint, &tls_config))
                    .await
                    .map_err(|_| ImapError::Timeout(self.timeout))??;
            Ok(Box::new(TokioIdleSession {
                session: Some(session),
            }) as Box<dyn IdleSession>)
        })
    }
}

/// Sesión `IDLE` real: envuelve una [`Session`] que se **cede** a `idle()`
/// durante la espera y se recupera al terminar (`done()`).
struct TokioIdleSession {
    /// La sesión viva, o `None` mientras una espera `IDLE` la retiene.
    session: Option<Session<ImapStream>>,
}

impl TokioIdleSession {
    /// El error devuelto al invocar un comando con la sesión en `IDLE`.
    fn busy() -> ImapError {
        ImapError::Unavailable("idle session is busy".to_string())
    }

    /// Pide prestada la sesión viva, o falla con [`ImapError::Unavailable`] si
    /// está retenida por una espera `IDLE`.
    fn borrow_session(&mut self) -> Result<&mut Session<ImapStream>, ImapError> {
        self.session.as_mut().ok_or_else(Self::busy)
    }
}

impl IdleSession for TokioIdleSession {
    fn select(&mut self, mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>> {
        Box::pin(async move {
            let session = self.borrow_session()?;
            let mailbox = session.select(mailbox).await.map_err(|error| match error {
                async_imap::error::Error::No(_) => ImapError::MailboxNotFound,
                other => ImapError::Imap(other),
            })?;
            Ok(mailbox_status(&mailbox))
        })
    }

    fn wait_for_change(
        &mut self,
        timeout: Duration,
    ) -> SendFuture<'_, Result<IdleEvent, ImapError>> {
        Box::pin(async move {
            let session = self.session.take().ok_or_else(Self::busy)?;
            let mut handle = session.idle();
            handle.init().await.map_err(ImapError::from)?;
            let response = {
                let (fut, _stop) = handle.wait_with_timeout(timeout);
                fut.await
            };
            // Always end IDLE and recover the session before classifying the
            // response, so the connection is reusable on the next cycle.
            let session = handle.done().await.map_err(ImapError::from)?;
            self.session = Some(session);
            match response.map_err(ImapError::from)? {
                IdleResponse::Timeout => Ok(IdleEvent::TimedOut),
                IdleResponse::NewData(_) | IdleResponse::ManualInterrupt => Ok(IdleEvent::Changed),
            }
        })
    }

    fn fetch_from(
        &mut self,
        from_uid: u32,
        format: FetchFormat,
    ) -> SendFuture<'_, Result<Vec<Message>, ImapError>> {
        Box::pin(async move {
            let session = self.borrow_session()?;
            fetch_messages(session, format!("{from_uid}:*"), format).await
        })
    }
}

/// Cliente HTTP real del webhook, sobre `reqwest`.
///
/// El `reqwest::Client` se construye de forma **perezosa** una sola vez, de modo
/// que [`HttpWebhookSender::new`] es infalible y no complica `AppState`.
pub struct HttpWebhookSender {
    /// Timeout aplicado a cada petición.
    timeout: Duration,
    /// Cliente HTTP construido de forma perezosa.
    client: tokio::sync::OnceCell<reqwest::Client>,
}

impl HttpWebhookSender {
    /// Construcción **infalible**; el `reqwest::Client` se crea de forma
    /// perezosa (una vez).
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            client: tokio::sync::OnceCell::new(),
        }
    }
}

impl WebhookSender for HttpWebhookSender {
    fn send<'a>(
        &'a self,
        url: &'a str,
        payload: &'a WebhookPayload,
    ) -> SendFuture<'a, Result<(), WebhookError>> {
        // The trait unifies the lifetimes of `self`, `url` and `payload`, so the
        // borrowed pieces are used directly: `reqwest` parses the URL and
        // serialises the payload eagerly, and neither has to be cloned.
        Box::pin(async move {
            let client = self
                .client
                .get_or_try_init(|| async {
                    reqwest::Client::builder()
                        .timeout(self.timeout)
                        .build()
                        .map_err(|_| WebhookError::Transport)
                })
                .await?;
            let response = client
                .post(url)
                .json(payload)
                .send()
                .await
                .map_err(|_| WebhookError::Transport)?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(WebhookError::Status(response.status().as_u16()))
            }
        })
    }
}

/// Bounded time [`IdleSupervisor::stop`] waits for its task before aborting it.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// Total number of webhook delivery attempts (the first try plus its retries).
const WEBHOOK_ATTEMPTS: usize = 3;

/// Delays applied before the second and third webhook attempts.
const WEBHOOK_RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(2)];

/// [`IdleStatus::status`] while the subscription is active.
pub const STATUS_RUNNING: &str = "running";
/// [`IdleStatus::status`] when the subscription is not active.
pub const STATUS_STOPPED: &str = "stopped";
/// [`IdleStatus::last_error`] after an IMAP connection or command failure.
pub const LAST_ERROR_IMAP_UNAVAILABLE: &str = "imap_unavailable";
/// [`IdleStatus::last_error`] after the webhook retries are exhausted.
pub const LAST_ERROR_WEBHOOK_FAILED: &str = "webhook_failed";

/// Settings of one IDLE subscription, built from [`Config`](crate::Config).
#[derive(Debug, Clone)]
pub struct IdleSettings {
    /// Mailbox watched by the subscription.
    pub mailbox: String,
    /// Webhook notified on every new message, or `None` when unconfigured.
    pub webhook_url: Option<String>,
    /// Timeout applied to each webhook request.
    pub webhook_timeout: Duration,
    /// Maximum message size, in bytes, that will be parsed.
    pub max_message_bytes: usize,
}

/// Errors produced while controlling the subscription.
#[derive(Debug, thiserror::Error)]
pub enum IdleError {
    /// `start` was requested without a configured webhook URL.
    #[error("idle notifications are not configured")]
    NotConfigured,
}

/// Observable state of the subscription.
///
/// `last_error` is always a stable code ([`LAST_ERROR_IMAP_UNAVAILABLE`] or
/// [`LAST_ERROR_WEBHOOK_FAILED`]) or `None`: no raw error rendering ever leaves
/// the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdleStatus {
    /// [`STATUS_RUNNING`] or [`STATUS_STOPPED`].
    pub status: &'static str,
    /// Mailbox watched by the subscription.
    pub mailbox: String,
    /// Last failure code, or `None` when the last operation succeeded.
    pub last_error: Option<&'static str>,
}

/// Running task and its shutdown channel.
struct TaskHandle {
    /// Notifies the task that it must stop.
    shutdown: tokio::sync::watch::Sender<bool>,
    /// Join handle of the supervisor task.
    handle: tokio::task::JoinHandle<()>,
}

/// Owns the IDLE subscription over its own dedicated IMAP connection.
///
/// `start`/`stop` are idempotent and safe to call concurrently; `status` reports
/// a snapshot. The background task keeps a persistent `last_uid` across
/// reconnections, re-issues `IDLE` every [`IDLE_REISSUE_INTERVAL`] and keeps
/// running until stopped.
pub struct IdleSupervisor {
    /// Factory for the dedicated IMAP connections.
    connector: Arc<dyn IdleConnector>,
    /// Webhook client used to notify new messages.
    webhook: Arc<dyn WebhookSender>,
    /// Subscription settings.
    settings: IdleSettings,
    /// Reconnection backoff.
    backoff: Backoff,
    /// Last failure code, shared with the task.
    last_error: Arc<StdMutex<Option<&'static str>>>,
    /// Running task and its shutdown channel, if any.
    task: tokio::sync::Mutex<Option<TaskHandle>>,
}

impl fmt::Debug for IdleSupervisor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The connector holds credentials and the webhook holds a URL: both are
        // redacted, mirroring `ConnectionManager`.
        f.debug_struct("IdleSupervisor")
            .field("mailbox", &self.settings.mailbox)
            .field("backoff", &self.backoff)
            .field("connector", &"***")
            .field("webhook", &"***")
            .finish()
    }
}

impl IdleSupervisor {
    /// Builds a supervisor over `connector` and `webhook`.
    ///
    /// Nothing is opened or scheduled until [`start`](Self::start) is called.
    pub fn new(
        connector: Arc<dyn IdleConnector>,
        webhook: Arc<dyn WebhookSender>,
        settings: IdleSettings,
        backoff: Backoff,
    ) -> Self {
        Self {
            connector,
            webhook,
            settings,
            backoff,
            last_error: Arc::new(StdMutex::new(None)),
            task: tokio::sync::Mutex::new(None),
        }
    }

    /// Starts the subscription.
    ///
    /// Idempotent: if it is already running, returns the current [`IdleStatus`]
    /// without opening a second connection. A task whose handle has already
    /// finished is discarded and replaced by a fresh one. Without a configured
    /// webhook URL nothing is started and [`IdleError::NotConfigured`] is
    /// returned.
    ///
    /// A genuinely new subscription starts clean: any `last_error` left by a
    /// previous run is cleared.
    pub async fn start(&self) -> Result<IdleStatus, IdleError> {
        if self.settings.webhook_url.is_none() {
            return Err(IdleError::NotConfigured);
        }

        let mut task = self.task.lock().await;
        if let Some(existing) = task.as_ref() {
            if !existing.handle.is_finished() {
                return Ok(self.snapshot(STATUS_RUNNING));
            }
            // The previous task died: drop its handle so a new one can be spawned.
            *task = None;
        }

        // Starting fresh: never drag a `webhook_failed`/`imap_unavailable` from a
        // previous execution into the new subscription.
        *self.last_error.lock().expect("last_error lock poisoned") = None;

        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(run_supervisor(
            Arc::clone(&self.connector),
            Arc::clone(&self.webhook),
            self.settings.clone(),
            self.backoff,
            Arc::clone(&self.last_error),
            receiver,
        ));
        *task = Some(TaskHandle { shutdown, handle });
        Ok(self.snapshot(STATUS_RUNNING))
    }

    /// Stops the subscription and waits for its task, bounded by
    /// [`STOP_TIMEOUT`]; the task is aborted if it does not finish in time.
    ///
    /// Idempotent: stopping a subscription that is not running is a no-op.
    pub async fn stop(&self) -> IdleStatus {
        let handle = self.task.lock().await.take();
        if let Some(TaskHandle { shutdown, handle }) = handle {
            // Closing the connection ends IDLE; a `DONE` is sent on the normal
            // timeout path inside `wait_for_change`.
            let _ = shutdown.send(true);
            let mut handle = handle;
            if tokio::time::timeout(STOP_TIMEOUT, &mut handle)
                .await
                .is_err()
            {
                handle.abort();
            }
        }
        self.snapshot(STATUS_STOPPED)
    }

    /// Returns a snapshot of the subscription state.
    ///
    /// A task whose `JoinHandle` has already finished (for example, it panicked)
    /// is reported as `stopped`, never as `running`.
    pub async fn status(&self) -> IdleStatus {
        let running = self
            .task
            .lock()
            .await
            .as_ref()
            .map(|task| !task.handle.is_finished())
            .unwrap_or(false);
        let status = if running {
            STATUS_RUNNING
        } else {
            STATUS_STOPPED
        };
        self.snapshot(status)
    }

    /// Builds an [`IdleStatus`] with `status` and the shared last error.
    fn snapshot(&self, status: &'static str) -> IdleStatus {
        IdleStatus {
            status,
            mailbox: self.settings.mailbox.clone(),
            last_error: *self.last_error.lock().expect("last_error lock poisoned"),
        }
    }
}

/// Background loop: connect, watch the mailbox and notify new messages.
async fn run_supervisor(
    connector: Arc<dyn IdleConnector>,
    webhook: Arc<dyn WebhookSender>,
    settings: IdleSettings,
    backoff: Backoff,
    last_error: Arc<StdMutex<Option<&'static str>>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let Some(webhook_url) = settings.webhook_url.clone() else {
        // `start` guards this, but never run without a webhook.
        return;
    };

    // `last_uid` deliberately lives outside the connection loop: a reconnection
    // must never skip mail that arrived while the link was down.
    let mut last_uid: u32 = 0;
    let mut uid_validity: Option<u32> = None;
    let mut first_select = true;
    let mut attempt: u32 = 1;

    'reconnect: loop {
        let mut session = match until_shutdown(&mut shutdown, connector.connect()).await {
            Some(Ok(session)) => session,
            Some(Err(_)) => {
                if !handle_failure(&mut shutdown, &backoff, &last_error, &mut attempt).await {
                    break 'reconnect;
                }
                continue 'reconnect;
            }
            None => break 'reconnect,
        };

        let status =
            match until_shutdown(&mut shutdown, session.select(settings.mailbox.clone())).await {
                Some(Ok(status)) => status,
                Some(Err(_)) => {
                    if !handle_failure(&mut shutdown, &backoff, &last_error, &mut attempt).await {
                        break 'reconnect;
                    }
                    continue 'reconnect;
                }
                None => break 'reconnect,
            };

        // The starting point is `UIDNEXT - 1`, so only mail that arrives *after*
        // the subscription starts is reported. A change of `UIDVALIDITY`
        // invalidates every stored UID, so the point is re-derived from the new
        // status.
        if first_select || status.uid_validity != uid_validity {
            last_uid = status.uid_next.map_or(0, |next| next.saturating_sub(1));
            uid_validity = status.uid_validity;
            first_select = false;
        }

        // A usable connection clears the IMAP error and resets the backoff.
        clear_last_error_if(&last_error, LAST_ERROR_IMAP_UNAVAILABLE);
        attempt = 1;

        loop {
            match until_shutdown(
                &mut shutdown,
                session.wait_for_change(IDLE_REISSUE_INTERVAL),
            )
            .await
            {
                Some(Ok(_)) => {}
                Some(Err(_)) => {
                    if !handle_failure(&mut shutdown, &backoff, &last_error, &mut attempt).await {
                        break 'reconnect;
                    }
                    continue 'reconnect;
                }
                None => break 'reconnect,
            }

            let fetched = until_shutdown(
                &mut shutdown,
                session.fetch_from(last_uid.saturating_add(1), FetchFormat::Full),
            )
            .await;
            let mut messages = match fetched {
                Some(Ok(messages)) => messages,
                Some(Err(_)) => {
                    if !handle_failure(&mut shutdown, &backoff, &last_error, &mut attempt).await {
                        break 'reconnect;
                    }
                    continue 'reconnect;
                }
                None => break 'reconnect,
            };

            // `UID FETCH` may answer in any order; report oldest-first.
            messages.sort_unstable_by_key(|message| message.uid);
            for message in messages {
                if message.uid <= last_uid {
                    continue;
                }
                let parsed = parse_bounded(&message, settings.max_message_bytes).await;
                let payload = WebhookPayload::from_message(
                    &settings.mailbox,
                    uid_validity,
                    &message,
                    parsed.as_ref(),
                );
                if !deliver(&webhook, &webhook_url, &payload, &last_error, &mut shutdown).await {
                    // The subscription was asked to stop while retrying.
                    break 'reconnect;
                }
                last_uid = message.uid;
            }
        }
    }
}

/// Runs `future`, returning `None` early if the subscription must stop.
async fn until_shutdown<F, T>(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    future: F,
) -> Option<T>
where
    F: std::future::Future<Output = T>,
{
    tokio::select! {
        biased;
        _ = shutdown.changed() => None,
        result = future => Some(result),
    }
}

/// Records an IMAP failure and waits the backoff delay for `attempt`.
///
/// Returns `false` when the subscription was asked to stop while waiting.
async fn handle_failure(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    backoff: &Backoff,
    last_error: &StdMutex<Option<&'static str>>,
    attempt: &mut u32,
) -> bool {
    set_last_error(last_error, LAST_ERROR_IMAP_UNAVAILABLE);
    let delay = backoff.delay(*attempt);
    let keep_watching = if delay.is_zero() {
        !*shutdown.borrow()
    } else {
        tokio::select! {
            biased;
            _ = shutdown.changed() => false,
            _ = tokio::time::sleep(delay) => true,
        }
    };
    if keep_watching {
        *attempt = attempt.saturating_add(1);
    }
    keep_watching
}

/// Parses `message`'s body off the async runtime, within the size bound.
///
/// Returns `None` when the message carries no body, exceeds `max_message_bytes`
/// (as reported or as fetched) or cannot be parsed; the caller still reports the
/// metadata with `parsed = false`.
async fn parse_bounded(message: &Message, max_message_bytes: usize) -> Option<ParsedMessage> {
    let body = message.body.as_ref()?;
    if body.len() > max_message_bytes {
        return None;
    }
    if message
        .size
        .is_some_and(|size| size as usize > max_message_bytes)
    {
        return None;
    }
    let body = body.clone();
    tokio::task::spawn_blocking(move || crate::mime::parse_message(&body).ok())
        .await
        .ok()
        .flatten()
}

/// Delivers `payload` to `url`, with bounded retries for retryable failures.
///
/// On success the webhook error is cleared. When the failure is not retryable or
/// the attempts are exhausted, [`LAST_ERROR_WEBHOOK_FAILED`] is recorded and the
/// subscription keeps running.
///
/// Returns `false` when the subscription was asked to stop while waiting between
/// retries, so the caller can unwind immediately; `true` otherwise. The retry
/// sleeps are cancellable: a stop signal during a wait ends the retries without
/// delivering further attempts.
async fn deliver(
    webhook: &Arc<dyn WebhookSender>,
    url: &str,
    payload: &WebhookPayload,
    last_error: &StdMutex<Option<&'static str>>,
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
) -> bool {
    let mut attempts_left = WEBHOOK_ATTEMPTS;
    loop {
        match webhook.send(url, payload).await {
            Ok(()) => {
                clear_last_error_if(last_error, LAST_ERROR_WEBHOOK_FAILED);
                return true;
            }
            Err(error) => {
                attempts_left -= 1;
                if error.is_retryable() && attempts_left > 0 {
                    let index = WEBHOOK_ATTEMPTS - attempts_left - 1;
                    if let Some(delay) = WEBHOOK_RETRY_DELAYS.get(index) {
                        // A stop during the retry wait aborts the delivery: the
                        // outer loop breaks out on the `false` return.
                        let stopped = tokio::select! {
                            biased;
                            _ = shutdown.changed() => true,
                            _ = tokio::time::sleep(*delay) => false,
                        };
                        if stopped {
                            return false;
                        }
                    }
                    continue;
                }
                set_last_error(last_error, LAST_ERROR_WEBHOOK_FAILED);
                return true;
            }
        }
    }
}

/// Records `code` as the current failure.
fn set_last_error(slot: &StdMutex<Option<&'static str>>, code: &'static str) {
    *slot.lock().expect("last_error lock poisoned") = Some(code);
}

/// Clears the failure when it is exactly `code`.
fn clear_last_error_if(slot: &StdMutex<Option<&'static str>>, code: &'static str) {
    let mut guard = slot.lock().expect("last_error lock poisoned");
    if *guard == Some(code) {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::config::{MailEndpoint, TlsMode};
    use crate::imap::{Address, Message, MessageEnvelope};
    use crate::mime::{ParsedAttachment, ParsedMessage};

    use super::*;

    /// A message with a complete envelope and scalar metadata.
    fn full_message() -> Message {
        Message {
            uid: 42,
            seq: 7,
            flags: vec!["\\Seen".to_string(), "\\Flagged".to_string()],
            size: Some(1234),
            internal_date: Some("2024-01-05T10:30:00+00:00".to_string()),
            envelope: Some(MessageEnvelope {
                from: vec![Address {
                    name: Some("Alice".to_string()),
                    address: Some("alice@example.com".to_string()),
                }],
                to: vec![Address {
                    name: Some("Bob".to_string()),
                    address: Some("bob@example.com".to_string()),
                }],
                cc: vec![Address {
                    name: None,
                    address: Some("carol@example.com".to_string()),
                }],
                subject: Some("Hello".to_string()),
                date: Some("Fri, 05 Jan 2024 10:30:00 +0000".to_string()),
                message_id: Some("<abc@example.com>".to_string()),
            }),
            headers: None,
            body: None,
        }
    }

    /// A parsed message carrying both bodies and one metadata-rich attachment.
    fn parsed_message() -> ParsedMessage {
        ParsedMessage {
            text: Some("Plain body".to_string()),
            html: Some("<p>Html body</p>".to_string()),
            attachments: vec![ParsedAttachment {
                id: 0,
                filename: Some("file.pdf".to_string()),
                content_type: Some("application/pdf".to_string()),
                size: 2048,
                inline: true,
                content_id: Some("cid@example.com".to_string()),
                content: vec![0xde, 0xad, 0xbe, 0xef],
            }],
        }
    }

    #[test]
    fn from_message_with_parsed_carries_full_metadata_and_bodies() {
        let message = full_message();
        let parsed = parsed_message();

        let payload = WebhookPayload::from_message("INBOX", Some(9), &message, Some(&parsed));

        assert_eq!(payload.mailbox, "INBOX");
        assert_eq!(payload.uid, 42);
        assert_eq!(payload.uid_validity, Some(9));
        assert_eq!(
            payload.flags,
            vec!["\\Seen".to_string(), "\\Flagged".to_string()]
        );
        assert_eq!(payload.size, Some(1234));
        assert_eq!(
            payload.internal_date.as_deref(),
            Some("2024-01-05T10:30:00+00:00")
        );

        let envelope = payload.envelope.expect("the envelope must be mapped");
        assert_eq!(envelope.from.len(), 1);
        assert_eq!(envelope.from[0].name.as_deref(), Some("Alice"));
        assert_eq!(
            envelope.from[0].address.as_deref(),
            Some("alice@example.com")
        );
        assert_eq!(envelope.to[0].address.as_deref(), Some("bob@example.com"));
        assert_eq!(envelope.cc[0].name, None);
        assert_eq!(envelope.cc[0].address.as_deref(), Some("carol@example.com"));
        assert_eq!(envelope.subject.as_deref(), Some("Hello"));
        assert_eq!(
            envelope.date.as_deref(),
            Some("Fri, 05 Jan 2024 10:30:00 +0000")
        );
        assert_eq!(envelope.message_id.as_deref(), Some("<abc@example.com>"));

        assert!(payload.parsed);
        assert_eq!(payload.text.as_deref(), Some("Plain body"));
        assert_eq!(payload.html.as_deref(), Some("<p>Html body</p>"));

        assert_eq!(payload.attachments.len(), 1);
        let attachment = &payload.attachments[0];
        assert_eq!(attachment.id, 0);
        assert_eq!(attachment.filename.as_deref(), Some("file.pdf"));
        assert_eq!(attachment.content_type.as_deref(), Some("application/pdf"));
        assert_eq!(attachment.size, 2048);
        assert!(attachment.inline);
        assert_eq!(attachment.content_id.as_deref(), Some("cid@example.com"));
    }

    #[test]
    fn from_message_without_parsed_is_metadata_only() {
        let message = full_message();

        let payload = WebhookPayload::from_message("INBOX", Some(9), &message, None);

        assert!(!payload.parsed);
        assert_eq!(payload.text, None);
        assert_eq!(payload.html, None);
        assert!(payload.attachments.is_empty(), "{:?}", payload.attachments);

        // The metadata is still present.
        assert_eq!(payload.uid, 42);
        assert_eq!(payload.mailbox, "INBOX");
        assert_eq!(payload.uid_validity, Some(9));
        assert_eq!(payload.flags.len(), 2);
        assert_eq!(payload.size, Some(1234));
        assert_eq!(
            payload.internal_date.as_deref(),
            Some("2024-01-05T10:30:00+00:00")
        );
        assert!(payload.envelope.is_some());
    }

    #[test]
    fn from_message_without_envelope_reports_none() {
        let mut message = full_message();
        message.envelope = None;

        let payload = WebhookPayload::from_message("INBOX", None, &message, None);

        assert_eq!(payload.envelope, None);
        assert_eq!(payload.uid_validity, None);
    }

    #[test]
    fn is_retryable_only_for_transport_and_5xx_or_429() {
        assert!(WebhookError::Transport.is_retryable());
        assert!(WebhookError::Status(500).is_retryable());
        assert!(WebhookError::Status(429).is_retryable());
        assert!(!WebhookError::Status(400).is_retryable());
        assert!(!WebhookError::Status(404).is_retryable());
    }

    #[test]
    fn payload_serializes_documented_keys_without_attachment_bytes() {
        let message = full_message();
        let parsed = parsed_message();
        let payload = WebhookPayload::from_message("INBOX", Some(9), &message, Some(&parsed));

        let json = serde_json::to_value(&payload).expect("the payload must serialize");
        let object = json.as_object().expect("the payload is a JSON object");

        for key in [
            "mailbox",
            "uid",
            "uid_validity",
            "flags",
            "size",
            "internal_date",
            "envelope",
            "parsed",
            "text",
            "html",
            "attachments",
        ] {
            assert!(object.contains_key(key), "the payload is missing `{key}`");
        }

        let serialized = json.to_string();
        assert!(
            !serialized.contains("content_base64"),
            "attachment bytes must never be serialized: {serialized}"
        );
        assert!(
            !serialized.contains("\"content\":"),
            "attachment raw content must never be serialized: {serialized}"
        );
    }

    #[test]
    fn idle_reissue_interval_is_29_minutes() {
        assert_eq!(IDLE_REISSUE_INTERVAL, Duration::from_secs(29 * 60));
    }

    #[test]
    fn tokio_idle_connector_builds_without_network() {
        let endpoint = MailEndpoint {
            host: "imap.example.com".to_string(),
            port: 993,
            tls: TlsMode::Implicit,
            username: "user".to_string(),
            password: "secret".to_string(),
        };
        let connector = TokioIdleConnector::new(endpoint, Duration::from_secs(30));
        assert_eq!(connector.timeout, Duration::from_secs(30));
        assert_eq!(connector.endpoint.host, "imap.example.com");
    }

    #[test]
    fn http_webhook_sender_builds_lazily_without_network() {
        let sender = HttpWebhookSender::new(Duration::from_secs(10));
        assert_eq!(sender.timeout, Duration::from_secs(10));
        assert!(
            sender.client.get().is_none(),
            "the HTTP client must be built lazily, not at construction"
        );
    }

    // --- IDLE supervisor fakes and tests -----------------------------------

    /// URL used by the supervisor tests; never dialed (the webhook is a fake).
    const TEST_WEBHOOK_URL: &str = "https://hooks.example.com/mail";

    /// A message with the given UID, optional body and reported size.
    fn idle_message(uid: u32, body: Option<Vec<u8>>, size: Option<u32>) -> Message {
        Message {
            uid,
            seq: uid,
            flags: vec!["\\Seen".to_string()],
            size,
            internal_date: Some("2024-05-01T10:00:00+00:00".to_string()),
            envelope: None,
            headers: None,
            body,
        }
    }

    /// A minimal, parseable `text/plain` message body.
    fn text_body(text: &str) -> Vec<u8> {
        format!(
            "From: Alice <alice@example.com>\r\n\
             To: Bob <bob@example.com>\r\n\
             Subject: New message\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=\"utf-8\"\r\n\
             \r\n\
             {text}\r\n"
        )
        .into_bytes()
    }

    /// A mailbox status with the given `UIDNEXT` and `UIDVALIDITY = 7`.
    fn idle_status(uid_next: Option<u32>) -> MailboxStatus {
        MailboxStatus {
            exists: 42,
            recent: 0,
            unseen: Some(1),
            uid_validity: Some(7),
            uid_next,
            flags: vec!["\\Seen".to_string()],
        }
    }

    /// A status for a connector whose `select` is never reached.
    fn empty_status() -> MailboxStatus {
        MailboxStatus {
            exists: 0,
            recent: 0,
            unseen: None,
            uid_validity: None,
            uid_next: None,
            flags: Vec::new(),
        }
    }

    /// Subscription settings for the tests.
    fn idle_settings(webhook_url: Option<&str>, max_message_bytes: usize) -> IdleSettings {
        IdleSettings {
            mailbox: "INBOX".to_string(),
            webhook_url: webhook_url.map(str::to_string),
            webhook_timeout: Duration::from_secs(5),
            max_message_bytes,
        }
    }

    /// A fast reconnection backoff so the tests do not slow down.
    fn fast_backoff() -> Backoff {
        Backoff {
            attempts: 3,
            base: Duration::from_millis(10),
            factor: 2,
            max: Duration::from_millis(40),
        }
    }

    /// Script shared by a fake connector and the sessions it hands out.
    struct IdleScript {
        /// Status returned by `select`.
        status: MailboxStatus,
        /// Events returned by successive `wait_for_change` calls.
        events: VecDeque<IdleEvent>,
        /// Messages the sessions can fetch from.
        messages: Vec<Message>,
    }

    /// State shared by [`FakeIdleConnector`] and its sessions.
    struct FakeConnectorState {
        /// Number of `connect` calls so far.
        connects: AtomicUsize,
        /// When `true`, every `connect` fails.
        failing: bool,
        /// The shared script.
        script: StdMutex<IdleScript>,
    }

    /// A fake [`IdleConnector`] counting connections and scripting sessions.
    #[derive(Clone)]
    struct FakeIdleConnector {
        /// Shared state.
        state: Arc<FakeConnectorState>,
    }

    impl FakeIdleConnector {
        /// A connector handing out sessions that read the given script.
        fn new(status: MailboxStatus, events: Vec<IdleEvent>, messages: Vec<Message>) -> Self {
            Self {
                state: Arc::new(FakeConnectorState {
                    connects: AtomicUsize::new(0),
                    failing: false,
                    script: StdMutex::new(IdleScript {
                        status,
                        events: events.into_iter().collect(),
                        messages,
                    }),
                }),
            }
        }

        /// A connector whose every `connect` fails.
        fn failing() -> Self {
            Self {
                state: Arc::new(FakeConnectorState {
                    connects: AtomicUsize::new(0),
                    failing: true,
                    script: StdMutex::new(IdleScript {
                        status: empty_status(),
                        events: VecDeque::new(),
                        messages: Vec::new(),
                    }),
                }),
            }
        }

        /// Number of `connect` calls observed.
        fn connects(&self) -> usize {
            self.state.connects.load(Ordering::SeqCst)
        }
    }

    impl IdleConnector for FakeIdleConnector {
        fn connect(&self) -> SendFuture<'_, Result<Box<dyn IdleSession>, ImapError>> {
            self.state.connects.fetch_add(1, Ordering::SeqCst);
            let failing = self.state.failing;
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                if failing {
                    return Err(ImapError::Unavailable(
                        "simulated connect failure".to_string(),
                    ));
                }
                Ok(Box::new(FakeIdleSession { state }) as Box<dyn IdleSession>)
            })
        }
    }

    /// A fake [`IdleSession`] reading its shared script.
    struct FakeIdleSession {
        /// Shared state with the connector that created it.
        state: Arc<FakeConnectorState>,
    }

    impl IdleSession for FakeIdleSession {
        fn select(&mut self, _mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>> {
            let status = self
                .state
                .script
                .lock()
                .expect("script poisoned")
                .status
                .clone();
            Box::pin(async move { Ok(status) })
        }

        fn wait_for_change(
            &mut self,
            _timeout: Duration,
        ) -> SendFuture<'_, Result<IdleEvent, ImapError>> {
            let event = self
                .state
                .script
                .lock()
                .expect("script poisoned")
                .events
                .pop_front();
            Box::pin(async move {
                match event {
                    Some(event) => Ok(event),
                    // Block forever so `stop` cancels the pending wait.
                    None => std::future::pending::<Result<IdleEvent, ImapError>>().await,
                }
            })
        }

        fn fetch_from(
            &mut self,
            from_uid: u32,
            _format: FetchFormat,
        ) -> SendFuture<'_, Result<Vec<Message>, ImapError>> {
            let messages = {
                let script = self.state.script.lock().expect("script poisoned");
                let mut available: Vec<Message> = script
                    .messages
                    .iter()
                    .filter(|message| message.uid >= from_uid)
                    .cloned()
                    .collect();
                available.sort_unstable_by_key(|message| message.uid);
                available
            };
            Box::pin(async move { Ok(messages) })
        }
    }

    /// A configurable failure for [`FakeWebhook`].
    #[derive(Debug, Clone, Copy)]
    enum WebhookFailure {
        /// A non-success HTTP status.
        Status(u16),
        /// A transport failure.
        Transport,
    }

    /// A fake [`WebhookSender`] recording payloads and scripting failures.
    struct FakeWebhook {
        /// Sink for the delivered payloads.
        deliveries: tokio::sync::mpsc::UnboundedSender<WebhookPayload>,
        /// Configured failure, if any.
        failure: StdMutex<Option<WebhookFailure>>,
        /// Number of `send` calls so far.
        calls: AtomicUsize,
    }

    impl FakeWebhook {
        /// A webhook that succeeds and records every payload.
        fn new(deliveries: tokio::sync::mpsc::UnboundedSender<WebhookPayload>) -> Self {
            Self {
                deliveries,
                failure: StdMutex::new(None),
                calls: AtomicUsize::new(0),
            }
        }

        /// Sets (`Some`) or clears (`None`) the configured failure.
        fn set_failure(&self, failure: Option<WebhookFailure>) {
            *self.failure.lock().expect("failure poisoned") = failure;
        }

        /// Number of `send` calls observed.
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl WebhookSender for FakeWebhook {
        fn send<'a>(
            &'a self,
            _url: &'a str,
            payload: &'a WebhookPayload,
        ) -> SendFuture<'a, Result<(), WebhookError>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let failure = *self.failure.lock().expect("failure poisoned");
            let result = match failure {
                Some(WebhookFailure::Status(code)) => Err(WebhookError::Status(code)),
                Some(WebhookFailure::Transport) => Err(WebhookError::Transport),
                None => {
                    let _ = self.deliveries.send(payload.clone());
                    Ok(())
                }
            };
            Box::pin(async move { result })
        }
    }

    /// Polls `check` until it is `true`, panicking after `timeout`.
    async fn wait_until<F, Fut>(timeout: Duration, mut check: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if check().await {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "condition was not met within {timeout:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Receives one payload, failing if none arrives within the bound.
    async fn recv_payload(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<WebhookPayload>,
    ) -> WebhookPayload {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("a payload must arrive in time")
            .expect("the delivery channel must stay open")
    }

    /// Asserts no payload arrives within a short bound.
    async fn assert_no_payload(rx: &mut tokio::sync::mpsc::UnboundedReceiver<WebhookPayload>) {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Err(_) => {}
            Ok(Some(payload)) => panic!("unexpected payload: {payload:?}"),
            Ok(None) => panic!("the delivery channel closed unexpectedly"),
        }
    }

    #[tokio::test]
    async fn fake_webhook_simulates_transport_and_status_failures() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = FakeWebhook::new(tx);
        let payload =
            WebhookPayload::from_message("INBOX", Some(1), &idle_message(1, None, None), None);

        webhook.set_failure(Some(WebhookFailure::Transport));
        let error = webhook
            .send(TEST_WEBHOOK_URL, &payload)
            .await
            .expect_err("a transport failure must surface");
        assert!(error.is_retryable(), "a transport failure is retryable");
        assert!(matches!(error, WebhookError::Transport));

        webhook.set_failure(Some(WebhookFailure::Status(400)));
        let error = webhook
            .send(TEST_WEBHOOK_URL, &payload)
            .await
            .expect_err("a 400 must surface");
        assert!(!error.is_retryable(), "a 400 is not retryable");
        assert!(matches!(error, WebhookError::Status(400)));

        webhook.set_failure(None);
        webhook
            .send(TEST_WEBHOOK_URL, &payload)
            .await
            .expect("a successful delivery");
        assert_eq!(rx.recv().await.expect("one delivery"), payload);
        assert_eq!(webhook.calls(), 3);
    }

    #[tokio::test]
    async fn start_without_webhook_is_not_configured_and_opens_no_connection() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let supervisor = IdleSupervisor::new(
            Arc::new(connector.clone()),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(None, 4096),
            fast_backoff(),
        );

        let error = supervisor
            .start()
            .await
            .expect_err("start must fail without a webhook");
        assert!(matches!(error, IdleError::NotConfigured));
        assert_eq!(connector.connects(), 0, "no connection must be opened");
        assert_eq!(supervisor.status().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn start_with_webhook_reports_running_and_status_matches() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let supervisor = IdleSupervisor::new(
            Arc::new(connector.clone()),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        let started = supervisor.start().await.expect("start with a webhook");
        assert_eq!(started.status, STATUS_RUNNING);
        assert_eq!(started.mailbox, "INBOX");
        assert_eq!(started.last_error, None);
        assert_eq!(supervisor.status().await, started);

        wait_until(Duration::from_secs(2), || async {
            connector.connects() >= 1
        })
        .await;
        assert_eq!(connector.connects(), 1);

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn start_is_idempotent_and_opens_a_single_connection() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let supervisor = IdleSupervisor::new(
            Arc::new(connector.clone()),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        let first = supervisor.start().await.expect("first start");
        let second = supervisor.start().await.expect("second start");
        assert_eq!(first, second);
        assert_eq!(second.status, STATUS_RUNNING);

        wait_until(Duration::from_secs(2), || async {
            connector.connects() >= 1
        })
        .await;
        assert_eq!(
            connector.connects(),
            1,
            "a second start must not open another connection"
        );

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn stop_is_idempotent() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        supervisor.start().await.expect("start");
        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
        assert_eq!(supervisor.status().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn new_message_is_notified_with_parsed_content_and_skips_older_uids() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let messages = vec![
            idle_message(42, Some(text_body("old")), Some(512)),
            idle_message(43, Some(text_body("Hello new")), Some(512)),
        ];
        let connector =
            FakeIdleConnector::new(idle_status(Some(43)), vec![IdleEvent::Changed], messages);
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        supervisor.start().await.expect("start");

        let payload = recv_payload(&mut rx).await;
        assert_eq!(payload.uid, 43);
        assert_eq!(payload.mailbox, "INBOX");
        assert_eq!(payload.uid_validity, Some(7));
        assert!(payload.parsed, "a parseable message must be parsed");
        assert!(
            payload
                .text
                .as_deref()
                .is_some_and(|text| text.contains("Hello new")),
            "unexpected text: {:?}",
            payload.text
        );
        assert!(
            payload.html.is_some(),
            "the html alternative must be derived"
        );

        // UID 42 was present before the subscription started: never notified.
        assert_no_payload(&mut rx).await;

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn oversized_and_empty_bodies_are_notified_with_metadata_only() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let messages = vec![
            // Reported size above the limit: never parsed.
            idle_message(10, Some(text_body("ignored")), Some(10_000)),
            // Empty body: parsing fails, metadata still reported.
            idle_message(11, Some(Vec::new()), Some(16)),
        ];
        let connector =
            FakeIdleConnector::new(idle_status(Some(10)), vec![IdleEvent::Changed], messages);
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        supervisor.start().await.expect("start");

        let first = recv_payload(&mut rx).await;
        assert_eq!(first.uid, 10);
        assert_eq!(first.mailbox, "INBOX");
        assert!(!first.parsed, "an oversized message must not be parsed");
        assert_eq!(first.text, None);
        assert_eq!(first.html, None);
        assert!(first.attachments.is_empty());
        assert_eq!(first.size, Some(10_000));

        let second = recv_payload(&mut rx).await;
        assert_eq!(second.uid, 11);
        assert!(!second.parsed, "an empty body must not be parsed");
        assert_eq!(second.text, None);
        assert_eq!(second.html, None);
        assert!(second.attachments.is_empty());
        assert_eq!(second.size, Some(16));

        assert_no_payload(&mut rx).await;
        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn webhook_failure_sets_a_stable_error_and_keeps_running() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let messages = vec![idle_message(43, Some(text_body("x")), Some(64))];
        let connector =
            FakeIdleConnector::new(idle_status(Some(43)), vec![IdleEvent::Changed], messages);
        let webhook = Arc::new(FakeWebhook::new(tx));
        webhook.set_failure(Some(WebhookFailure::Status(400)));
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            webhook.clone(),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        supervisor.start().await.expect("start");

        wait_until(Duration::from_secs(2), || async {
            supervisor.status().await.last_error == Some(LAST_ERROR_WEBHOOK_FAILED)
        })
        .await;

        let status = supervisor.status().await;
        assert_eq!(status.status, STATUS_RUNNING);
        assert_eq!(status.last_error, Some(LAST_ERROR_WEBHOOK_FAILED));
        assert!(webhook.calls() >= 1, "the webhook must have been attempted");
        assert!(
            rx.try_recv().is_err(),
            "a failed delivery must not be recorded"
        );

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn connector_failure_sets_imap_unavailable_and_keeps_retrying() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::failing();
        let supervisor = IdleSupervisor::new(
            Arc::new(connector.clone()),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        let started = supervisor.start().await.expect("start");
        assert_eq!(started.status, STATUS_RUNNING);

        wait_until(Duration::from_secs(2), || async {
            supervisor.status().await.last_error == Some(LAST_ERROR_IMAP_UNAVAILABLE)
        })
        .await;

        assert_eq!(supervisor.status().await.status, STATUS_RUNNING);
        wait_until(Duration::from_secs(2), || async {
            connector.connects() >= 2
        })
        .await;

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    /// A [`TaskHandle`] whose background task has already finished.
    async fn finished_task_handle() -> TaskHandle {
        let (shutdown, _receiver) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(async {});
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
        TaskHandle { shutdown, handle }
    }

    #[tokio::test]
    async fn status_reports_stopped_when_the_background_task_finished() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        *supervisor.task.lock().await = Some(finished_task_handle().await);

        assert_eq!(
            supervisor.status().await.status,
            STATUS_STOPPED,
            "a finished task must never read as running"
        );
    }

    #[tokio::test]
    async fn start_replaces_a_finished_task_and_clears_a_stale_error() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let supervisor = IdleSupervisor::new(
            Arc::new(connector.clone()),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        *supervisor.task.lock().await = Some(finished_task_handle().await);
        *supervisor
            .last_error
            .lock()
            .expect("last_error lock poisoned") = Some(LAST_ERROR_WEBHOOK_FAILED);

        let started = supervisor
            .start()
            .await
            .expect("start must replace the finished task");
        assert_eq!(started.status, STATUS_RUNNING);
        assert_eq!(
            started.last_error, None,
            "a fresh subscription must not inherit a stale last_error"
        );

        wait_until(Duration::from_secs(2), || async {
            connector.connects() >= 1
        })
        .await;
        assert_eq!(connector.connects(), 1, "a new task must be spawned");

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn stop_cancels_pending_webhook_retries() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let messages = vec![idle_message(43, Some(text_body("x")), Some(64))];
        let connector =
            FakeIdleConnector::new(idle_status(Some(43)), vec![IdleEvent::Changed], messages);
        let webhook = Arc::new(FakeWebhook::new(tx));
        webhook.set_failure(Some(WebhookFailure::Transport));
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            webhook.clone(),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
        );

        supervisor.start().await.expect("start");
        wait_until(Duration::from_secs(2), || async { webhook.calls() >= 1 }).await;

        // The 1 s retry wait must be cancellable: `stop` returns promptly instead
        // of waiting the retries out (or reaching its own 5 s abort).
        let stopped = tokio::time::timeout(Duration::from_secs(2), supervisor.stop())
            .await
            .expect("stop must cancel the pending retry instead of timing out");
        assert_eq!(stopped.status, STATUS_STOPPED);
        assert!(
            webhook.calls() <= 2,
            "a cancelled delivery must not run every retry: {}",
            webhook.calls()
        );
    }
}
