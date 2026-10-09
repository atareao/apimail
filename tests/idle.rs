//! Integration tests for the IDLE control endpoints (`POST /api/idle/start`,
//! `POST /api/idle/stop` and `GET /api/idle/status`) using in-memory fakes for
//! every service, so no test ever touches the network.
//!
//! The fake [`IdleConnector`] counts the connections it hands out and serves a
//! scripted mailbox; the fake [`WebhookSender`] records every payload it is
//! asked to deliver.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::idle::{
    IdleConnector, IdleEvent, IdleSession, WebhookError, WebhookPayload, WebhookSender,
};
use apimail::imap::{
    FetchFormat, ImapConnector, ImapError, ImapSession, MailboxStatus, Message, SendFuture,
};
use apimail::smtp::{MailSender, OutgoingMessage, SendFuture as MailSendFuture};
use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-idle-key";
const IMAP_HOST: &str = "imap.test.example";
const IMAP_PORT: u16 = 1993;
const IMAP_USER: &str = "imap-user@test.example";
const IMAP_PASSWORD: &str = "imap-secret";
const WEBHOOK_URL: &str = "https://hooks.test.example/mail";

/// Loads a valid [`Config`], optionally configuring the webhook URL.
fn config_with_webhook(webhook_url: Option<&str>) -> Config {
    Config::from_lookup(move |key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some(IMAP_HOST.to_string()),
        "APIMAIL_IMAP_PORT" => Some(IMAP_PORT.to_string()),
        "APIMAIL_IMAP_USER" => Some(IMAP_USER.to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some(IMAP_PASSWORD.to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.test.example".to_string()),
        "APIMAIL_SMTP_USER" => Some("smtp-user@test.example".to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-secret".to_string()),
        "APIMAIL_WEBHOOK_URL" => webhook_url.map(str::to_string),
        _ => None,
    })
    .expect("valid test config")
}

/// A [`MailSender`] that never touches the network and never fails.
struct NoopMailer;

impl MailSender for NoopMailer {
    fn send(&self, _message: OutgoingMessage) -> MailSendFuture<'_> {
        Box::pin(async move { Ok(()) })
    }
}

/// An [`ImapConnector`] that is never dialed by the IDLE routes; it only exists
/// to build the full [`AppState`].
struct NoopImapConnector;

impl ImapConnector for NoopImapConnector {
    fn connect(&self) -> SendFuture<'_, Result<Box<dyn ImapSession>, ImapError>> {
        Box::pin(async move { Err(ImapError::Unavailable("not used in tests".to_string())) })
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

/// A fake [`IdleConnector`] counting connections and scripting sessions.
#[derive(Clone)]
struct FakeIdleConnector {
    /// Number of `connect` calls so far.
    connects: Arc<AtomicUsize>,
    /// The shared script.
    script: Arc<Mutex<IdleScript>>,
}

impl FakeIdleConnector {
    /// A connector handing out sessions that read the given script.
    fn new(status: MailboxStatus, events: Vec<IdleEvent>, messages: Vec<Message>) -> Self {
        Self {
            connects: Arc::new(AtomicUsize::new(0)),
            script: Arc::new(Mutex::new(IdleScript {
                status,
                events: events.into_iter().collect(),
                messages,
            })),
        }
    }

    /// Number of `connect` calls observed.
    fn connects(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }
}

impl IdleConnector for FakeIdleConnector {
    fn connect(&self) -> SendFuture<'_, Result<Box<dyn IdleSession>, ImapError>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let script = Arc::clone(&self.script);
        Box::pin(async move { Ok(Box::new(FakeIdleSession { script }) as Box<dyn IdleSession>) })
    }
}

/// A fake [`IdleSession`] reading its shared script.
struct FakeIdleSession {
    /// Shared state with the connector that created it.
    script: Arc<Mutex<IdleScript>>,
}

impl IdleSession for FakeIdleSession {
    fn select(&mut self, _mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>> {
        let status = self.script.lock().expect("script poisoned").status.clone();
        Box::pin(async move { Ok(status) })
    }

    fn wait_for_change(
        &mut self,
        _timeout: Duration,
    ) -> SendFuture<'_, Result<IdleEvent, ImapError>> {
        let event = self
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
            let script = self.script.lock().expect("script poisoned");
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

/// A fake [`WebhookSender`] recording every payload it is asked to deliver.
struct FakeWebhook {
    /// Sink for the delivered payloads.
    deliveries: tokio::sync::mpsc::UnboundedSender<WebhookPayload>,
}

impl WebhookSender for FakeWebhook {
    fn send<'a>(
        &'a self,
        _url: &'a str,
        payload: &'a WebhookPayload,
    ) -> SendFuture<'a, Result<(), WebhookError>> {
        let _ = self.deliveries.send(payload.clone());
        Box::pin(async move { Ok(()) })
    }
}

/// Builds the router with every service injected, without touching the network.
fn app(config: &Config, connector: FakeIdleConnector, webhook: Arc<FakeWebhook>) -> Router {
    build_router(AppState::with_idle(
        config,
        Arc::new(NoopMailer),
        Arc::new(NoopImapConnector),
        Arc::new(connector),
        webhook,
    ))
}

/// A message with the given UID and optional body.
fn message(uid: u32, body: Option<Vec<u8>>) -> Message {
    Message {
        uid,
        seq: uid,
        flags: vec!["\\Seen".to_string()],
        size: Some(512),
        internal_date: None,
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
fn mailbox_status(uid_next: Option<u32>) -> MailboxStatus {
    MailboxStatus {
        exists: 1,
        recent: 0,
        unseen: None,
        uid_validity: Some(7),
        uid_next,
        flags: vec!["\\Seen".to_string()],
    }
}

/// The bearer header value for the test API key.
fn bearer() -> String {
    format!("Bearer {API_KEY}")
}

/// Sends a `POST` request with an optional `Authorization`.
async fn post(app: &Router, uri: &str, authorization: Option<&str>) -> Response {
    let mut builder = Request::builder().method("POST").uri(uri);
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).expect("valid request"))
        .await
        .expect("router should respond")
}

/// Sends a `GET` request with an optional `Authorization`.
async fn get(app: &Router, uri: &str, authorization: Option<&str>) -> Response {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    app.clone()
        .oneshot(builder.body(Body::empty()).expect("valid request"))
        .await
        .expect("router should respond")
}

/// Reads a response body as JSON.
async fn body_json(response: Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body should be readable")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("valid JSON body")
}

/// The `Content-Type` of a response, if present.
fn content_type(response: &Response) -> Option<&str> {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
}

/// Waits (bounded) until `check` holds, panicking with `what` otherwise.
async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn start_reports_running_with_a_configured_webhook() {
    let config = config_with_webhook(Some(WEBHOOK_URL));
    let connector = FakeIdleConnector::new(mailbox_status(Some(43)), Vec::new(), Vec::new());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let app = app(
        &config,
        connector.clone(),
        Arc::new(FakeWebhook { deliveries: tx }),
    );

    let response = post(&app, "/api/idle/start", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), Some("application/json"));
    let json = body_json(response).await;
    assert_eq!(json["status"], "running");
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["last_error"], serde_json::Value::Null);

    // A dedicated connection is established for the subscription.
    wait_until("the dedicated connection", || connector.connects() >= 1).await;

    let _ = post(&app, "/api/idle/stop", Some(&bearer())).await;
}

#[tokio::test]
async fn start_is_idempotent_and_opens_a_single_connection() {
    let config = config_with_webhook(Some(WEBHOOK_URL));
    let connector = FakeIdleConnector::new(mailbox_status(Some(43)), Vec::new(), Vec::new());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let app = app(
        &config,
        connector.clone(),
        Arc::new(FakeWebhook { deliveries: tx }),
    );

    let first = post(&app, "/api/idle/start", Some(&bearer())).await;
    assert_eq!(first.status(), StatusCode::OK);
    wait_until("the first connection", || connector.connects() >= 1).await;

    let second = post(&app, "/api/idle/start", Some(&bearer())).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(body_json(second).await["status"], "running");
    assert_eq!(
        connector.connects(),
        1,
        "a second start must not open another connection"
    );

    let _ = post(&app, "/api/idle/stop", Some(&bearer())).await;
}

#[tokio::test]
async fn status_reports_state_mailbox_and_last_error() {
    let config = config_with_webhook(Some(WEBHOOK_URL));
    let connector = FakeIdleConnector::new(mailbox_status(Some(43)), Vec::new(), Vec::new());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let app = app(
        &config,
        connector.clone(),
        Arc::new(FakeWebhook { deliveries: tx }),
    );

    let stopped = get(&app, "/api/idle/status", Some(&bearer())).await;
    assert_eq!(stopped.status(), StatusCode::OK);
    assert_eq!(content_type(&stopped), Some("application/json"));
    let json = body_json(stopped).await;
    assert_eq!(json["status"], "stopped");
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["last_error"], serde_json::Value::Null);

    let _ = post(&app, "/api/idle/start", Some(&bearer())).await;

    let running = get(&app, "/api/idle/status", Some(&bearer())).await;
    assert_eq!(running.status(), StatusCode::OK);
    let json = body_json(running).await;
    assert_eq!(json["status"], "running");
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["last_error"], serde_json::Value::Null);

    let _ = post(&app, "/api/idle/stop", Some(&bearer())).await;
}

#[tokio::test]
async fn stop_reports_stopped_and_is_idempotent() {
    let config = config_with_webhook(Some(WEBHOOK_URL));
    let connector = FakeIdleConnector::new(mailbox_status(Some(43)), Vec::new(), Vec::new());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let app = app(
        &config,
        connector.clone(),
        Arc::new(FakeWebhook { deliveries: tx }),
    );

    let _ = post(&app, "/api/idle/start", Some(&bearer())).await;

    let stopped = post(&app, "/api/idle/stop", Some(&bearer())).await;
    assert_eq!(stopped.status(), StatusCode::OK);
    assert_eq!(content_type(&stopped), Some("application/json"));
    assert_eq!(body_json(stopped).await["status"], "stopped");

    let again = post(&app, "/api/idle/stop", Some(&bearer())).await;
    assert_eq!(again.status(), StatusCode::OK);
    assert_eq!(body_json(again).await["status"], "stopped");
}

#[tokio::test]
async fn start_without_a_webhook_is_501_and_opens_no_connection() {
    let config = config_with_webhook(None);
    let connector = FakeIdleConnector::new(mailbox_status(Some(43)), Vec::new(), Vec::new());
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let app = app(
        &config,
        connector.clone(),
        Arc::new(FakeWebhook { deliveries: tx }),
    );

    let response = post(&app, "/api/idle/start", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(content_type(&response), Some("application/json"));
    let json = body_json(response).await;
    assert_eq!(json["error"], "idle_not_configured");
    assert!(json["message"].is_string(), "{json}");
    assert_eq!(connector.connects(), 0, "no connection must be opened");
}

#[tokio::test]
async fn idle_routes_without_an_api_key_are_401() {
    for uri in ["/api/idle/start", "/api/idle/stop", "/api/idle/status"] {
        let config = config_with_webhook(Some(WEBHOOK_URL));
        let connector = FakeIdleConnector::new(mailbox_status(Some(43)), Vec::new(), Vec::new());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let app = app(&config, connector, Arc::new(FakeWebhook { deliveries: tx }));

        let response = if uri == "/api/idle/status" {
            get(&app, uri, None).await
        } else {
            post(&app, uri, None).await
        };

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "`{uri}` should be a 401 without a key"
        );
    }
}

#[tokio::test]
async fn a_changed_event_notifies_the_webhook_with_parsed_content() {
    let config = config_with_webhook(Some(WEBHOOK_URL));
    let messages = vec![message(43, Some(text_body("Hello new")))];
    let connector =
        FakeIdleConnector::new(mailbox_status(Some(43)), vec![IdleEvent::Changed], messages);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let app = app(
        &config,
        connector.clone(),
        Arc::new(FakeWebhook { deliveries: tx }),
    );

    let started = post(&app, "/api/idle/start", Some(&bearer())).await;
    assert_eq!(started.status(), StatusCode::OK);

    let payload = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("a payload must arrive in time")
        .expect("the delivery channel must stay open");

    assert_eq!(payload.uid, 43);
    assert_eq!(payload.mailbox, "INBOX");
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

    let stopped = post(&app, "/api/idle/stop", Some(&bearer())).await;
    assert_eq!(stopped.status(), StatusCode::OK);
    assert_eq!(body_json(stopped).await["status"], "stopped");
}
