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
use serde::{Deserialize, Serialize};

use crate::config::MailEndpoint;
use crate::imap::{
    Address, Backoff, FetchFormat, ImapError, ImapStream, MailboxStatus, Message, MessageEnvelope,
    SendFuture, build_tls_config, connect_and_login, fetch_messages, mailbox_status,
};
use crate::mime::{ParsedAttachment, ParsedMessage};
use crate::queue::{NotificationQueue, StoredWatermark};

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// [`IdleStatus::status`] while the subscription is active.
pub const STATUS_RUNNING: &str = "running";
/// [`IdleStatus::status`] when the subscription is not active.
pub const STATUS_STOPPED: &str = "stopped";
/// [`IdleStatus::last_error`] after an IMAP connection or command failure.
pub const LAST_ERROR_IMAP_UNAVAILABLE: &str = "imap_unavailable";
/// [`IdleStatus::last_error`] after the webhook retries are exhausted.
pub const LAST_ERROR_WEBHOOK_FAILED: &str = "webhook_failed";
/// [`IdleStatus::last_error`] when the delivery queue itself is unreachable.
pub const LAST_ERROR_QUEUE_UNAVAILABLE: &str = "queue_unavailable";
/// How often the delivery worker checks for pending work while the queue is empty.
pub const DELIVERY_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Bounded attempts for a non-retryable failure before the notification is
/// discarded and counted as failed.
pub const POISON_ATTEMPTS: u32 = 3;

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
/// `last_error` is always a stable code ([`LAST_ERROR_IMAP_UNAVAILABLE`],
/// [`LAST_ERROR_WEBHOOK_FAILED`] or [`LAST_ERROR_QUEUE_UNAVAILABLE`]) or `None`:
/// no raw error rendering ever leaves the supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdleStatus {
    /// [`STATUS_RUNNING`] or [`STATUS_STOPPED`].
    pub status: &'static str,
    /// Mailbox watched by the subscription.
    pub mailbox: String,
    /// Last failure code, or `None` when the last operation succeeded.
    pub last_error: Option<&'static str>,
}

/// Running tasks and their shutdown channel.
struct TaskHandle {
    /// Notifies the tasks that they must stop.
    shutdown: tokio::sync::watch::Sender<bool>,
    /// Join handle of the supervisor (observer) task.
    observer: tokio::task::JoinHandle<()>,
    /// Join handle of the webhook delivery worker.
    worker: tokio::task::JoinHandle<()>,
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
    /// Backoff applied to webhook retries by the delivery worker.
    delivery_backoff: Backoff,
    /// Durable delivery queue drained by the worker.
    queue: Arc<dyn NotificationQueue>,
    /// Last failure code, shared with the tasks.
    last_error: Arc<StdMutex<Option<&'static str>>>,
    /// Running tasks and their shutdown channel, if any.
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
    /// Builds a supervisor whose delivery worker drains `queue`.
    ///
    /// Nothing is opened or scheduled until [`start`](Self::start) is called.
    /// The `queue` is mandatory: the caller decides whether it is in memory or
    /// persistent, so the supervisor never silently picks a storage backend.
    pub fn new(
        connector: Arc<dyn IdleConnector>,
        webhook: Arc<dyn WebhookSender>,
        settings: IdleSettings,
        backoff: Backoff,
        queue: Arc<dyn NotificationQueue>,
    ) -> Self {
        Self {
            connector,
            webhook,
            settings,
            backoff,
            delivery_backoff: default_delivery_backoff(),
            queue,
            last_error: Arc::new(StdMutex::new(None)),
            task: tokio::sync::Mutex::new(None),
        }
    }

    /// The durable delivery queue drained by the worker.
    pub fn queue(&self) -> &Arc<dyn NotificationQueue> {
        &self.queue
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
        let Some(webhook_url) = self.settings.webhook_url.clone() else {
            return Err(IdleError::NotConfigured);
        };

        let mut task = self.task.lock().await;
        if let Some(existing) = task.as_ref() {
            if !existing.observer.is_finished() {
                return Ok(self.snapshot(STATUS_RUNNING));
            }
            // The previous observer died: drop its handle so a new pair of tasks
            // can be spawned. Dropping the old sender stops any worker it left
            // behind.
            *task = None;
        }

        // Starting fresh: never drag a `webhook_failed`/`imap_unavailable` from a
        // previous execution into the new subscription.
        *self.last_error.lock().expect("last_error lock poisoned") = None;

        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let worker = tokio::spawn(run_delivery_worker(
            Arc::clone(&self.queue),
            Arc::clone(&self.webhook),
            webhook_url,
            Arc::clone(&self.last_error),
            self.delivery_backoff,
            receiver.clone(),
        ));
        let observer = tokio::spawn(run_supervisor(
            Arc::clone(&self.connector),
            Arc::clone(&self.queue),
            self.settings.clone(),
            self.backoff,
            Arc::clone(&self.last_error),
            receiver,
        ));
        *task = Some(TaskHandle {
            shutdown,
            observer,
            worker,
        });
        Ok(self.snapshot(STATUS_RUNNING))
    }

    /// Stops the subscription and waits for both tasks, bounded by
    /// [`STOP_TIMEOUT`]; the tasks are aborted if they do not finish in time.
    ///
    /// Idempotent: stopping a subscription that is not running is a no-op.
    pub async fn stop(&self) -> IdleStatus {
        let handle = self.task.lock().await.take();
        if let Some(TaskHandle {
            shutdown,
            observer,
            worker,
        }) = handle
        {
            // Closing the connection ends IDLE; a `DONE` is sent on the normal
            // timeout path inside `wait_for_change`.
            let _ = shutdown.send(true);
            let mut observer = observer;
            let mut worker = worker;
            let stopped = tokio::time::timeout(STOP_TIMEOUT, async {
                let _ = (&mut observer).await;
                let _ = (&mut worker).await;
            })
            .await;
            if stopped.is_err() {
                observer.abort();
                worker.abort();
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
            .map(|task| !task.observer.is_finished())
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

/// Background loop: connect, watch the mailbox and enqueue new messages.
///
/// A new message is handed to the durable `queue` (never delivered inline): the
/// delivery worker drains it on its own task. The mailbox watermark only advances
/// after the enqueue succeeds, so a failed enqueue re-fetches the message on the
/// next connection instead of losing it.
async fn run_supervisor(
    connector: Arc<dyn IdleConnector>,
    queue: Arc<dyn NotificationQueue>,
    settings: IdleSettings,
    backoff: Backoff,
    last_error: Arc<StdMutex<Option<&'static str>>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    if settings.webhook_url.is_none() {
        // `start` guards this, but never run without a webhook.
        return;
    }

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
                if !handle_failure(
                    &mut shutdown,
                    &backoff,
                    &last_error,
                    LAST_ERROR_IMAP_UNAVAILABLE,
                    &mut attempt,
                )
                .await
                {
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
                    if !handle_failure(
                        &mut shutdown,
                        &backoff,
                        &last_error,
                        LAST_ERROR_IMAP_UNAVAILABLE,
                        &mut attempt,
                    )
                    .await
                    {
                        break 'reconnect;
                    }
                    continue 'reconnect;
                }
                None => break 'reconnect,
            };

        // The starting point is the highest UID present when the subscription
        // first started (`UIDNEXT - 1`), or, when a persistent queue holds a
        // watermark for the same mailbox and `UIDVALIDITY`, that stored watermark,
        // so the mail that arrived while the service was down is still notified. A
        // change of `UIDVALIDITY` invalidates every stored UID, so the point is
        // re-derived from the new status.
        //
        // An in-memory queue never resumes: the point is always re-derived from
        // `UIDNEXT - 1`, keeping the default behaviour intact.
        if first_select || status.uid_validity != uid_validity {
            let resumed = if queue.is_persistent() {
                queue.watermark().filter(|watermark| {
                    watermark.mailbox == settings.mailbox
                        && watermark.uid_validity == status.uid_validity
                })
            } else {
                None
            };
            match resumed {
                Some(watermark) => {
                    last_uid = watermark.last_uid;
                    uid_validity = watermark.uid_validity;
                }
                None => {
                    last_uid = status.uid_next.map_or(0, |next| next.saturating_sub(1));
                    uid_validity = status.uid_validity;
                    if queue.is_persistent() {
                        let watermark = StoredWatermark {
                            mailbox: settings.mailbox.clone(),
                            uid_validity,
                            last_uid,
                        };
                        if queue.set_watermark(watermark).await.is_err() {
                            set_last_error(&last_error, LAST_ERROR_QUEUE_UNAVAILABLE);
                        }
                    }
                }
            }
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
                    if !handle_failure(
                        &mut shutdown,
                        &backoff,
                        &last_error,
                        LAST_ERROR_IMAP_UNAVAILABLE,
                        &mut attempt,
                    )
                    .await
                    {
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
                    if !handle_failure(
                        &mut shutdown,
                        &backoff,
                        &last_error,
                        LAST_ERROR_IMAP_UNAVAILABLE,
                        &mut attempt,
                    )
                    .await
                    {
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
                match queue.enqueue(payload).await {
                    Ok(()) => {
                        // Durability order: the notification is enqueued first and
                        // only then the watermark advances, so a crash can never
                        // acknowledge mail that was never handed over.
                        last_uid = message.uid;
                        if queue.is_persistent() {
                            let watermark = StoredWatermark {
                                mailbox: settings.mailbox.clone(),
                                uid_validity,
                                last_uid,
                            };
                            match queue.set_watermark(watermark).await {
                                Ok(()) => {
                                    clear_last_error_if(&last_error, LAST_ERROR_QUEUE_UNAVAILABLE)
                                }
                                Err(_) => set_last_error(&last_error, LAST_ERROR_QUEUE_UNAVAILABLE),
                            }
                        }
                    }
                    Err(_) => {
                        // The notification was not acknowledged: keep the old
                        // watermark and reconnect with backoff, so the message is
                        // re-fetched instead of lost.
                        set_last_error(&last_error, LAST_ERROR_QUEUE_UNAVAILABLE);
                        if !handle_failure(
                            &mut shutdown,
                            &backoff,
                            &last_error,
                            LAST_ERROR_QUEUE_UNAVAILABLE,
                            &mut attempt,
                        )
                        .await
                        {
                            break 'reconnect;
                        }
                        continue 'reconnect;
                    }
                }
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

/// Records a failure with the stable `code` and waits the backoff delay for
/// `attempt`.
///
/// Returns `false` when the subscription was asked to stop while waiting.
async fn handle_failure(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    backoff: &Backoff,
    last_error: &StdMutex<Option<&'static str>>,
    code: &'static str,
    attempt: &mut u32,
) -> bool {
    set_last_error(last_error, code);
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

/// Dedicated backoff for the delivery worker's webhook retries: unlimited
/// attempts with a capped exponential delay.
fn default_delivery_backoff() -> Backoff {
    Backoff {
        attempts: u32::MAX,
        base: Duration::from_secs(1),
        factor: 2,
        max: Duration::from_secs(300),
    }
}

/// Background loop: drains the queue, delivering each notification in FIFO order.
///
/// A retryable failure is retried forever with the capped `retry_backoff`; a
/// non-retryable one is attempted up to [`POISON_ATTEMPTS`] times and then
/// discarded, counted as failed, so a poison notification never blocks the FIFO.
/// Every wait is cancellable with the shutdown signal: a stop returns at once.
async fn run_delivery_worker(
    queue: Arc<dyn NotificationQueue>,
    webhook: Arc<dyn WebhookSender>,
    webhook_url: String,
    last_error: Arc<StdMutex<Option<&'static str>>>,
    retry_backoff: Backoff,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        match queue.peek_oldest().await {
            Ok(Some(item)) => match webhook.send(&webhook_url, &item.payload).await {
                Ok(()) => {
                    let _ = queue.ack(item.seq).await;
                    clear_last_error_if(&last_error, LAST_ERROR_WEBHOOK_FAILED);
                }
                Err(error) if error.is_retryable() => {
                    set_last_error(&last_error, LAST_ERROR_WEBHOOK_FAILED);
                    let delay = retry_backoff.delay(item.attempts.saturating_add(1));
                    if wait_or_shutdown(&mut shutdown, delay).await {
                        return;
                    }
                }
                Err(_) => {
                    set_last_error(&last_error, LAST_ERROR_WEBHOOK_FAILED);
                    if item.attempts >= POISON_ATTEMPTS {
                        let _ = queue.discard(item.seq).await;
                    } else if wait_or_shutdown(&mut shutdown, DELIVERY_POLL_INTERVAL).await {
                        return;
                    }
                }
            },
            Ok(None) => {
                if wait_or_shutdown(&mut shutdown, DELIVERY_POLL_INTERVAL).await {
                    return;
                }
            }
            Err(_) => {
                set_last_error(&last_error, LAST_ERROR_QUEUE_UNAVAILABLE);
                if wait_or_shutdown(&mut shutdown, DELIVERY_POLL_INTERVAL).await {
                    return;
                }
            }
        }
    }
}

/// Waits `delay`, returning `true` when the shutdown signal fires first.
async fn wait_or_shutdown(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    delay: Duration,
) -> bool {
    tokio::select! {
        biased;
        _ = shutdown.changed() => true,
        _ = tokio::time::sleep(delay) => false,
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
    use crate::queue::{QueueError, QueueLimits, QueueStats, QueuedNotification, WebhookQueue};

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
        /// The `fetch_from` starting UIDs observed, in call order.
        fetches: StdMutex<Vec<u32>>,
        /// Notified whenever an event is queued, so a session already waiting in
        /// `wait_for_change` wakes up instead of missing it.
        changed: tokio::sync::Notify,
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
                    fetches: StdMutex::new(Vec::new()),
                    changed: tokio::sync::Notify::new(),
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
                    fetches: StdMutex::new(Vec::new()),
                    changed: tokio::sync::Notify::new(),
                }),
            }
        }

        /// Number of `connect` calls observed.
        fn connects(&self) -> usize {
            self.state.connects.load(Ordering::SeqCst)
        }

        /// Records one mailbox change carrying a single message: the message is
        /// added to the fetchable set and a `Changed` event is queued. The two
        /// are pushed under the same lock so a session never sees the event
        /// before the message exists.
        fn push_change(&self, message: Message) {
            {
                let mut script = self.state.script.lock().expect("script poisoned");
                script.messages.push(message);
                script.events.push_back(IdleEvent::Changed);
            }
            self.state.changed.notify_one();
        }

        /// The `fetch_from` starting UIDs observed, in call order.
        fn fetches(&self) -> Vec<u32> {
            self.state.fetches.lock().expect("fetches poisoned").clone()
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
            let state = Arc::clone(&self.state);
            Box::pin(async move {
                loop {
                    if let Some(event) = state
                        .script
                        .lock()
                        .expect("script poisoned")
                        .events
                        .pop_front()
                    {
                        return Ok(event);
                    }
                    // Block until `push_change` queues an event; the shutdown
                    // path cancels this wait from the outside.
                    state.changed.notified().await;
                }
            })
        }

        fn fetch_from(
            &mut self,
            from_uid: u32,
            _format: FetchFormat,
        ) -> SendFuture<'_, Result<Vec<Message>, ImapError>> {
            self.state
                .fetches
                .lock()
                .expect("fetches poisoned")
                .push(from_uid);
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
        /// Configured failure, if any, applied to every call.
        failure: StdMutex<Option<WebhookFailure>>,
        /// One-shot failures consumed by the next `send` calls, before `failure`.
        script: StdMutex<VecDeque<WebhookFailure>>,
        /// Number of `send` calls so far.
        calls: AtomicUsize,
    }

    impl FakeWebhook {
        /// A webhook that succeeds and records every payload.
        fn new(deliveries: tokio::sync::mpsc::UnboundedSender<WebhookPayload>) -> Self {
            Self {
                deliveries,
                failure: StdMutex::new(None),
                script: StdMutex::new(VecDeque::new()),
                calls: AtomicUsize::new(0),
            }
        }

        /// Sets (`Some`) or clears (`None`) a failure applied to every call.
        fn set_failure(&self, failure: Option<WebhookFailure>) {
            *self.failure.lock().expect("failure poisoned") = failure;
        }

        /// Queues one-shot failures consumed by the next `send` calls.
        fn fail_next(&self, failures: impl IntoIterator<Item = WebhookFailure>) {
            *self.script.lock().expect("script poisoned") = failures.into_iter().collect();
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
            let failure = {
                let mut script = self.script.lock().expect("script poisoned");
                script
                    .pop_front()
                    .or_else(|| *self.failure.lock().expect("failure poisoned"))
            };
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
            worker_queue(),
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
            worker_queue(),
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
            worker_queue(),
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
            worker_queue(),
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
            worker_queue(),
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
            worker_queue(),
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
            worker_queue(),
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
            worker_queue(),
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

    /// A [`TaskHandle`] whose tasks have already finished.
    async fn finished_task_handle() -> TaskHandle {
        let (shutdown, _receiver) = tokio::sync::watch::channel(false);
        let observer = tokio::spawn(async {});
        let worker = tokio::spawn(async {});
        while !observer.is_finished() {
            tokio::task::yield_now().await;
        }
        TaskHandle {
            shutdown,
            observer,
            worker,
        }
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
            worker_queue(),
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
            worker_queue(),
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

    // --- delivery worker tests ---------------------------------------------

    /// An in-memory queue with generous limits for the worker tests.
    fn worker_queue() -> Arc<dyn NotificationQueue> {
        Arc::new(WebhookQueue::in_memory(QueueLimits {
            max_items: 100,
            max_bytes: u64::MAX,
        }))
    }

    /// A 1 ms-base backoff so worker retries stay fast in tests.
    fn fast_worker_backoff() -> Backoff {
        Backoff {
            attempts: u32::MAX,
            base: Duration::from_millis(1),
            factor: 2,
            max: Duration::from_millis(20),
        }
    }

    /// Spawns a delivery worker over `queue`/`webhook`, returning its shutdown
    /// sender and join handle.
    fn spawn_worker(
        queue: Arc<dyn NotificationQueue>,
        webhook: Arc<dyn WebhookSender>,
    ) -> (
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ) {
        let (shutdown, receiver) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(run_delivery_worker(
            queue,
            webhook,
            TEST_WEBHOOK_URL.to_string(),
            Arc::new(StdMutex::new(None)),
            fast_worker_backoff(),
            receiver,
        ));
        (shutdown, handle)
    }

    /// Signals a worker to stop and waits for it, bounded.
    async fn stop_worker(
        shutdown: tokio::sync::watch::Sender<bool>,
        handle: tokio::task::JoinHandle<()>,
    ) {
        let _ = shutdown.send(true);
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("the worker must stop promptly")
            .expect("the worker must not panic");
    }

    /// A payload for the worker tests.
    fn worker_payload() -> WebhookPayload {
        WebhookPayload::from_message("INBOX", Some(1), &idle_message(1, None, None), None)
    }

    #[tokio::test]
    async fn delivery_worker_delivers_a_queued_notification() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        let queue = worker_queue();
        let payload = worker_payload();
        queue.enqueue(payload.clone()).await.expect("enqueue");

        let (shutdown, handle) = spawn_worker(Arc::clone(&queue), webhook);
        wait_until(Duration::from_secs(2), || async {
            queue.stats().delivered == 1
        })
        .await;

        let stats = queue.stats();
        assert_eq!(stats.delivered, 1);
        assert_eq!(stats.pending, 0);
        assert_eq!(rx.recv().await.expect("one delivery"), payload);
        stop_worker(shutdown, handle).await;
    }

    #[tokio::test]
    async fn delivery_worker_retries_a_retryable_failure_until_delivered() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        webhook.fail_next([WebhookFailure::Transport, WebhookFailure::Transport]);
        let queue = worker_queue();
        let payload = worker_payload();
        queue.enqueue(payload.clone()).await.expect("enqueue");

        let (shutdown, handle) = spawn_worker(Arc::clone(&queue), webhook.clone());
        wait_until(Duration::from_secs(2), || async {
            queue.stats().delivered == 1
        })
        .await;

        let stats = queue.stats();
        assert_eq!(stats.delivered, 1);
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.failed, 0, "a retryable failure is never discarded");
        assert_eq!(rx.recv().await.expect("one delivery"), payload);
        assert_eq!(webhook.calls(), 3, "two failures then a success");
        stop_worker(shutdown, handle).await;
    }

    #[tokio::test]
    async fn delivery_worker_discards_a_poison_notification_after_bounded_attempts() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        webhook.set_failure(Some(WebhookFailure::Status(400)));
        let queue = worker_queue();
        queue.enqueue(worker_payload()).await.expect("enqueue");

        let (shutdown, handle) = spawn_worker(Arc::clone(&queue), webhook.clone());
        wait_until(Duration::from_secs(3), || async {
            queue.stats().failed == 1
        })
        .await;

        let stats = queue.stats();
        assert_eq!(
            stats.failed, 1,
            "a poison notification is counted as failed"
        );
        assert_eq!(stats.delivered, 0);
        assert_eq!(stats.pending, 0, "the poison notification is discarded");
        assert_eq!(
            webhook.calls() as u32,
            POISON_ATTEMPTS,
            "a non-retryable failure is attempted exactly the bounded number of times"
        );
        stop_worker(shutdown, handle).await;
    }

    #[tokio::test]
    async fn stop_cancels_the_delivery_worker() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let webhook = Arc::new(FakeWebhook::new(tx));
        webhook.set_failure(Some(WebhookFailure::Transport));
        let queue = worker_queue();
        queue.enqueue(worker_payload()).await.expect("enqueue");
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            webhook.clone(),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
            Arc::clone(&queue),
        );

        supervisor.start().await.expect("start");
        wait_until(Duration::from_secs(2), || async { webhook.calls() >= 1 }).await;

        let stopped = tokio::time::timeout(Duration::from_secs(2), supervisor.stop())
            .await
            .expect("stop must cancel the delivery worker promptly");
        assert_eq!(stopped.status, STATUS_STOPPED);
        assert_eq!(queue.stats().delivered, 0, "nothing was delivered");

        let calls = webhook.calls();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(webhook.calls(), calls, "the worker stopped retrying");
    }

    #[tokio::test]
    async fn supervisor_exposes_its_delivery_queue() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let queue = worker_queue();
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            Arc::new(FakeWebhook::new(tx)),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            fast_backoff(),
            Arc::clone(&queue),
        );

        assert!(Arc::ptr_eq(supervisor.queue(), &queue));
    }

    // --- ingestion, watermark and resume tests -----------------------------

    /// A feedback backoff (1 ms base) so reconnection tests stay fast.
    fn quick_backoff() -> Backoff {
        Backoff {
            attempts: u32::MAX,
            base: Duration::from_millis(1),
            factor: 2,
            max: Duration::from_millis(5),
        }
    }

    /// A self-cleaning temporary directory for the persistent-queue tests.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::AtomicU64;
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let mut path = std::env::temp_dir();
            path.push(format!(
                "apimail-idle-test-{tag}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Generous limits for an in-memory queue used by the ingestion tests.
    fn ingest_queue() -> Arc<WebhookQueue> {
        Arc::new(WebhookQueue::in_memory(QueueLimits {
            max_items: 100,
            max_bytes: u64::MAX,
        }))
    }

    /// A generous persistent queue bound to `path`.
    fn persistent_queue(path: std::path::PathBuf) -> Arc<WebhookQueue> {
        Arc::new(
            WebhookQueue::persistent(
                path,
                QueueLimits {
                    max_items: 100,
                    max_bytes: u64::MAX,
                },
            )
            .expect("a persistent queue"),
        )
    }

    /// A [`NotificationQueue`] wrapping a real queue, with a failure switch and
    /// a record of every `set_watermark` call.
    struct FakeQueue {
        /// The real queue doing the work when injection is off.
        inner: Arc<WebhookQueue>,
        /// When `true`, every `enqueue` fails.
        fail_enqueue: bool,
        /// Every `set_watermark` argument observed, in order.
        watermark_writes: StdMutex<Vec<StoredWatermark>>,
    }

    impl FakeQueue {
        fn new(inner: Arc<WebhookQueue>, fail_enqueue: bool) -> Self {
            Self {
                inner,
                fail_enqueue,
                watermark_writes: StdMutex::new(Vec::new()),
            }
        }

        /// The `set_watermark` arguments observed, in order.
        fn watermark_writes(&self) -> Vec<StoredWatermark> {
            self.watermark_writes
                .lock()
                .expect("watermark writes poisoned")
                .clone()
        }
    }

    impl NotificationQueue for FakeQueue {
        fn enqueue<'a>(
            &'a self,
            payload: WebhookPayload,
        ) -> SendFuture<'a, Result<(), QueueError>> {
            if self.fail_enqueue {
                Box::pin(async { Err(QueueError::Storage) })
            } else {
                Box::pin(self.inner.enqueue(payload))
            }
        }

        fn peek_oldest<'a>(
            &'a self,
        ) -> SendFuture<'a, Result<Option<QueuedNotification>, QueueError>> {
            Box::pin(self.inner.peek_oldest())
        }

        fn ack<'a>(&'a self, seq: u64) -> SendFuture<'a, Result<(), QueueError>> {
            Box::pin(self.inner.ack(seq))
        }

        fn discard<'a>(&'a self, seq: u64) -> SendFuture<'a, Result<(), QueueError>> {
            Box::pin(self.inner.discard(seq))
        }

        fn set_watermark<'a>(
            &'a self,
            watermark: StoredWatermark,
        ) -> SendFuture<'a, Result<(), QueueError>> {
            self.watermark_writes
                .lock()
                .expect("watermark writes poisoned")
                .push(watermark.clone());
            Box::pin(self.inner.set_watermark(watermark))
        }

        fn stats(&self) -> QueueStats {
            self.inner.stats()
        }

        fn watermark(&self) -> Option<StoredWatermark> {
            self.inner.watermark()
        }

        fn is_persistent(&self) -> bool {
            self.inner.is_persistent()
        }
    }

    #[tokio::test]
    async fn a_new_message_is_enqueued_and_ingestion_never_blocks_on_the_webhook() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        // The webhook always fails: delivery can never drain the queue.
        webhook.set_failure(Some(WebhookFailure::Transport));
        let connector = FakeIdleConnector::new(idle_status(Some(43)), Vec::new(), Vec::new());
        let queue = ingest_queue();
        let supervisor = IdleSupervisor::new(
            Arc::new(connector.clone()),
            webhook.clone(),
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            quick_backoff(),
            queue.clone(),
        );

        supervisor.start().await.expect("start");

        connector.push_change(idle_message(44, Some(text_body("first")), Some(64)));
        wait_until(Duration::from_secs(2), || async {
            queue.stats().pending == 1
        })
        .await;

        // The webhook is failing, yet the subscription keeps ingesting.
        connector.push_change(idle_message(45, Some(text_body("second")), Some(64)));
        wait_until(Duration::from_secs(2), || async {
            queue.stats().pending == 2
        })
        .await;

        assert_eq!(supervisor.status().await.status, STATUS_RUNNING);
        // The worker drains on its own task; wait for its first (failing)
        // attempt instead of assuming it already ran.
        wait_until(Duration::from_secs(2), || async { webhook.calls() >= 1 }).await;
        assert_eq!(queue.stats().delivered, 0, "nothing can be delivered");

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn the_watermark_advances_only_after_an_enqueue() {
        let dir = TempDir::new("watermark");
        let queue = persistent_queue(dir.path().join("queue.jsonl"));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        webhook.set_failure(Some(WebhookFailure::Transport));
        let connector = FakeIdleConnector::new(
            idle_status(Some(100)),
            vec![IdleEvent::Changed],
            vec![
                idle_message(100, Some(text_body("a")), Some(64)),
                idle_message(101, Some(text_body("b")), Some(64)),
            ],
        );
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            webhook,
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            quick_backoff(),
            queue.clone(),
        );

        supervisor.start().await.expect("start");

        wait_until(Duration::from_secs(2), || async {
            queue
                .watermark()
                .is_some_and(|watermark| watermark.last_uid == 101)
        })
        .await;

        let watermark = queue.watermark().expect("a watermark must be persisted");
        assert_eq!(watermark.mailbox, "INBOX");
        assert_eq!(watermark.uid_validity, Some(7));
        assert_eq!(
            watermark.last_uid, 101,
            "the watermark must end at the last enqueued message"
        );

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn a_persistent_watermark_resumes_from_the_stored_uid() {
        let dir = TempDir::new("resume");
        let queue = persistent_queue(dir.path().join("queue.jsonl"));
        queue
            .set_watermark(StoredWatermark {
                mailbox: "INBOX".to_string(),
                uid_validity: Some(7),
                last_uid: 42,
            })
            .await
            .expect("seed the watermark");

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        webhook.set_failure(Some(WebhookFailure::Transport));
        let connector = FakeIdleConnector::new(
            idle_status(Some(100)),
            vec![IdleEvent::Changed],
            vec![idle_message(43, Some(text_body("resumed")), Some(64))],
        );
        let supervisor = IdleSupervisor::new(
            Arc::new(connector.clone()),
            webhook,
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            quick_backoff(),
            queue.clone(),
        );

        supervisor.start().await.expect("start");

        wait_until(Duration::from_secs(2), || async {
            connector.fetches().contains(&43)
        })
        .await;

        let fetches = connector.fetches();
        assert!(
            fetches.contains(&43),
            "resume must fetch from the watermark: {fetches:?}"
        );
        assert!(
            !fetches.contains(&100),
            "resume must not skip to UIDNEXT: {fetches:?}"
        );

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn an_in_memory_queue_resets_to_uidnext_and_does_not_resume() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        let connector = FakeIdleConnector::new(
            idle_status(Some(100)),
            vec![IdleEvent::Changed],
            vec![idle_message(100, Some(text_body("new")), Some(64))],
        );
        let queue: Arc<dyn NotificationQueue> = ingest_queue();
        let supervisor = IdleSupervisor::new(
            Arc::new(connector.clone()),
            webhook,
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            quick_backoff(),
            queue,
        );

        supervisor.start().await.expect("start");

        wait_until(Duration::from_secs(2), || async {
            connector.fetches().contains(&100)
        })
        .await;

        let fetches = connector.fetches();
        assert!(
            fetches.contains(&100),
            "an in-memory queue must fetch from UIDNEXT: {fetches:?}"
        );
        assert!(
            !fetches.contains(&99),
            "an in-memory queue must never resume: {fetches:?}"
        );

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn a_failed_enqueue_records_queue_unavailable_without_advancing_the_watermark() {
        let dir = TempDir::new("enqueue-fail");
        let inner = persistent_queue(dir.path().join("queue.jsonl"));
        inner
            .set_watermark(StoredWatermark {
                mailbox: "INBOX".to_string(),
                uid_validity: Some(7),
                last_uid: 42,
            })
            .await
            .expect("seed the watermark");
        let fake = Arc::new(FakeQueue::new(Arc::clone(&inner), true));
        let queue: Arc<dyn NotificationQueue> = fake.clone();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        let connector = FakeIdleConnector::new(
            idle_status(Some(100)),
            vec![IdleEvent::Changed],
            vec![idle_message(43, Some(text_body("x")), Some(64))],
        );
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            webhook,
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            quick_backoff(),
            queue,
        );

        supervisor.start().await.expect("start");

        wait_until(Duration::from_secs(2), || async {
            supervisor.status().await.last_error == Some(LAST_ERROR_QUEUE_UNAVAILABLE)
        })
        .await;

        assert_eq!(
            inner.watermark().map(|watermark| watermark.last_uid),
            Some(42),
            "a failed enqueue must not advance the watermark"
        );
        assert!(
            fake.watermark_writes().is_empty(),
            "no watermark write must be attempted"
        );

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }

    #[tokio::test]
    async fn an_in_memory_queue_never_writes_a_watermark() {
        let fake = Arc::new(FakeQueue::new(
            WebhookQueue::in_memory(QueueLimits {
                max_items: 100,
                max_bytes: u64::MAX,
            })
            .into(),
            false,
        ));
        let queue: Arc<dyn NotificationQueue> = fake.clone();

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let webhook = Arc::new(FakeWebhook::new(tx));
        webhook.set_failure(Some(WebhookFailure::Transport));
        let connector = FakeIdleConnector::new(
            idle_status(Some(100)),
            vec![IdleEvent::Changed],
            vec![idle_message(100, Some(text_body("x")), Some(64))],
        );
        let supervisor = IdleSupervisor::new(
            Arc::new(connector),
            webhook,
            idle_settings(Some(TEST_WEBHOOK_URL), 4096),
            quick_backoff(),
            queue,
        );

        supervisor.start().await.expect("start");

        wait_until(Duration::from_secs(2), || async {
            fake.stats().pending == 1
        })
        .await;
        // Give the supervisor a beat in case it would write a watermark late.
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert!(
            fake.watermark_writes().is_empty(),
            "an in-memory queue must never persist a watermark"
        );

        assert_eq!(supervisor.stop().await.status, STATUS_STOPPED);
    }
}
