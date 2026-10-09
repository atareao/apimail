//! Real integration tests for the durable delivery queue behind `GET
//! /api/idle/queue`.
//!
//! The delivery worker is independent of the IMAP connection, so this file
//! injects a minimal [`IdleConnector`] whose `connect` always fails (the
//! dedicated connection is never exercised here) plus a [`WebhookSender`] whose
//! accept/refuse behaviour is scripted. The real queue is obtained from the
//! supervisor with `state.idle().queue()`, so the tests drive the actual
//! persistent/in-memory storage instead of a fake.
//!
//! Every wait is bounded with `tokio::time::timeout`/a deadline so a test can
//! never hang, and each persistent test uses a unique temporary path so the
//! tests never collide.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::idle::{IdleConnector, IdleSession, WebhookError, WebhookPayload, WebhookSender};
use apimail::imap::{ImapConnector, ImapError, ImapSession, Message, SendFuture};
use apimail::smtp::{MailSender, OutgoingMessage, SendFuture as MailSendFuture};
use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-queue-key";
const IMAP_HOST: &str = "imap.test.example";
const IMAP_PORT: u16 = 1993;
const IMAP_USER: &str = "imap-user@test.example";
const IMAP_PASSWORD: &str = "imap-secret";
const WEBHOOK_URL: &str = "https://hooks.test.example/mail";

/// A distinctive value that must never reach the queue file.
const LEAKY_PASSWORD: &str = "leaky-imap-secret";

/// Sequence used to build collision-free temporary paths.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A unique temporary queue file path for one test.
fn temp_queue_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "apimail-queue-{tag}-{}-{}.jsonl",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// Loads a valid [`Config`], optionally with a queue path and extra overrides.
fn config_from(queue_path: Option<&Path>, overrides: &[(&str, &str)]) -> Config {
    let mut entries: Vec<(String, String)> = vec![
        ("APIMAIL_API_KEY".to_string(), API_KEY.to_string()),
        ("APIMAIL_IMAP_HOST".to_string(), IMAP_HOST.to_string()),
        ("APIMAIL_IMAP_PORT".to_string(), IMAP_PORT.to_string()),
        ("APIMAIL_IMAP_USER".to_string(), IMAP_USER.to_string()),
        (
            "APIMAIL_IMAP_PASSWORD".to_string(),
            IMAP_PASSWORD.to_string(),
        ),
        (
            "APIMAIL_SMTP_HOST".to_string(),
            "smtp.test.example".to_string(),
        ),
        (
            "APIMAIL_SMTP_USER".to_string(),
            "smtp-user@test.example".to_string(),
        ),
        (
            "APIMAIL_SMTP_PASSWORD".to_string(),
            "smtp-secret".to_string(),
        ),
        ("APIMAIL_WEBHOOK_URL".to_string(), WEBHOOK_URL.to_string()),
    ];
    if let Some(path) = queue_path {
        entries.push((
            "APIMAIL_QUEUE_PATH".to_string(),
            path.to_string_lossy().into_owned(),
        ));
    }
    for (key, value) in overrides {
        entries.push(((*key).to_string(), (*value).to_string()));
    }
    let map: HashMap<String, String> = entries.into_iter().collect();
    Config::from_lookup(move |key| map.get(key).cloned()).expect("valid test config")
}

/// A [`MailSender`] that never touches the network and never fails.
struct NoopMailer;

impl MailSender for NoopMailer {
    fn send(&self, _message: OutgoingMessage) -> MailSendFuture<'_> {
        Box::pin(async move { Ok(()) })
    }
}

/// An [`ImapConnector`] that is never dialed by the IDLE routes.
struct NoopImapConnector;

impl ImapConnector for NoopImapConnector {
    fn connect(&self) -> SendFuture<'_, Result<Box<dyn ImapSession>, ImapError>> {
        Box::pin(async move { Err(ImapError::Unavailable("not used in tests".to_string())) })
    }
}

/// An [`IdleConnector`] whose every connection fails.
///
/// The delivery worker is independent of the IMAP link, so failing the dedicated
/// connection never stops the queue from being drained.
struct FailingIdleConnector;

impl IdleConnector for FailingIdleConnector {
    fn connect(&self) -> SendFuture<'_, Result<Box<dyn IdleSession>, ImapError>> {
        Box::pin(async move {
            Err(ImapError::Unavailable(
                "idle is not used in queue tests".to_string(),
            ))
        })
    }
}

/// A [`WebhookSender`] recording accepted payloads and able to fail on demand.
struct FakeWebhook {
    /// Sink for the payloads the webhook accepted.
    accepted: Mutex<Vec<WebhookPayload>>,
    /// When `true`, every delivery fails as a retryable transport error.
    fail: AtomicBool,
}

impl FakeWebhook {
    /// A webhook that accepts every delivery.
    fn accepting() -> Self {
        Self {
            accepted: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        }
    }

    /// A webhook that refuses every delivery with a retryable error.
    fn failing() -> Self {
        Self {
            accepted: Mutex::new(Vec::new()),
            fail: AtomicBool::new(true),
        }
    }

    /// Number of payloads accepted so far.
    fn accepted_count(&self) -> usize {
        self.accepted.lock().expect("webhook lock poisoned").len()
    }
}

impl WebhookSender for FakeWebhook {
    fn send<'a>(
        &'a self,
        _url: &'a str,
        payload: &'a WebhookPayload,
    ) -> SendFuture<'a, Result<(), WebhookError>> {
        Box::pin(async move {
            if self.fail.load(Ordering::SeqCst) {
                return Err(WebhookError::Transport);
            }
            self.accepted
                .lock()
                .expect("webhook lock poisoned")
                .push(payload.clone());
            Ok(())
        })
    }
}

/// Builds the application state with every service injected, no network.
fn build_state(config: &Config, webhook: Arc<dyn WebhookSender>) -> AppState {
    AppState::with_idle(
        config,
        Arc::new(NoopMailer),
        Arc::new(NoopImapConnector),
        Arc::new(FailingIdleConnector),
        webhook,
    )
    .expect("building the app state must not fail")
}

/// A metadata-only notification with the given UID.
fn notification(uid: u32) -> WebhookPayload {
    let message = Message {
        uid,
        seq: uid,
        flags: vec!["\\Seen".to_string()],
        size: Some(512),
        internal_date: None,
        envelope: None,
        headers: None,
        body: None,
    };
    WebhookPayload::from_message("INBOX", Some(7), &message, None)
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

/// Fetches the queue state from the protected endpoint.
async fn queue_state(app: &Router) -> serde_json::Value {
    let response = get(app, "/api/idle/queue", Some(&bearer())).await;
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

/// Polls the queue state (bounded) until `ready` holds, returning the last state.
async fn wait_for_queue(
    app: &Router,
    what: &str,
    mut ready: impl FnMut(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let state = queue_state(app).await;
        if ready(&state) {
            return state;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}; last state: {state}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn queue_state_is_reported_with_an_api_key() {
    let config = config_from(None, &[]);
    let state = build_state(&config, Arc::new(FakeWebhook::accepting()));
    let app = build_router(state);

    let response = get(&app, "/api/idle/queue", Some(&bearer())).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), Some("application/json"));

    let json = body_json(response).await;
    assert_eq!(json["persistent"], serde_json::json!(false));
    assert_eq!(json["pending"], serde_json::json!(0));
    assert_eq!(json["delivered"], serde_json::json!(0));
    assert_eq!(json["dropped"], serde_json::json!(0));
    assert_eq!(json["failed"], serde_json::json!(0));
    assert_eq!(json["oldest_pending_secs"], serde_json::Value::Null);
}

#[tokio::test]
async fn queue_state_requires_an_api_key() {
    let config = config_from(None, &[]);
    let state = build_state(&config, Arc::new(FakeWebhook::accepting()));
    let app = build_router(state);

    let response = get(&app, "/api/idle/queue", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(body_json(response).await["error"], "unauthorized");
}

#[tokio::test]
async fn a_persistent_queue_is_reported_as_persistent() {
    let path = temp_queue_path("persistent");
    let config = config_from(Some(&path), &[]);
    let state = build_state(&config, Arc::new(FakeWebhook::accepting()));

    assert!(
        path.exists(),
        "the queue file must exist right after constructing the state"
    );

    let app = build_router(state);
    let json = queue_state(&app).await;
    assert_eq!(json["persistent"], serde_json::json!(true));
    assert_eq!(json["pending"], serde_json::json!(0));

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn undelivered_notifications_survive_a_restart() {
    let path = temp_queue_path("restart");
    let config = config_from(Some(&path), &[]);

    // State A: a persistent queue and a webhook that always refuses. The
    // notification is enqueued directly into the real queue and the supervisor
    // (worker + observer) is started, but the worker can never deliver it.
    let failing = Arc::new(FakeWebhook::failing());
    let state_a = build_state(&config, failing.clone());
    state_a
        .idle()
        .queue()
        .enqueue(notification(43))
        .await
        .expect("enqueue");
    let app_a = build_router(state_a.clone());

    let started = post(&app_a, "/api/idle/start", Some(&bearer())).await;
    assert_eq!(started.status(), StatusCode::OK);

    let stats = wait_for_queue(&app_a, "pending = 1", |json| {
        json["pending"] == serde_json::json!(1)
    })
    .await;
    assert_eq!(stats["pending"], serde_json::json!(1));
    assert_eq!(
        failing.accepted_count(),
        0,
        "the failing webhook must not have accepted anything"
    );

    // Simulate a restart: stop A so it stops touching the file, then drop it.
    let _ = post(&app_a, "/api/idle/stop", Some(&bearer())).await;
    drop(app_a);
    drop(state_a);

    // State B: a fresh state over the same path reloads the pending item.
    let accepting = Arc::new(FakeWebhook::accepting());
    let state_b = build_state(&config, accepting.clone());
    assert_eq!(
        state_b.idle().queue().stats().pending,
        1,
        "the queue must be reloaded from disk at construction"
    );
    let app_b = build_router(state_b.clone());

    let before = queue_state(&app_b).await;
    assert_eq!(before["persistent"], serde_json::json!(true));
    assert_eq!(
        before["pending"],
        serde_json::json!(1),
        "the pending notification must be reported before starting"
    );

    let started_b = post(&app_b, "/api/idle/start", Some(&bearer())).await;
    assert_eq!(started_b.status(), StatusCode::OK);

    let after = wait_for_queue(&app_b, "delivered = 1", |json| {
        json["delivered"] == serde_json::json!(1) && json["pending"] == serde_json::json!(0)
    })
    .await;
    assert_eq!(after["delivered"], serde_json::json!(1));
    assert_eq!(after["pending"], serde_json::json!(0));
    assert_eq!(
        accepting.accepted_count(),
        1,
        "the accepted webhook must have received the reloaded notification"
    );

    let _ = post(&app_b, "/api/idle/stop", Some(&bearer())).await;
    let _ = std::fs::remove_file(&path);
}

#[cfg(unix)]
#[tokio::test]
async fn the_queue_file_is_owner_only_and_holds_no_secrets() {
    use std::os::unix::fs::PermissionsExt as _;

    let path = temp_queue_path("perms");
    let config = config_from(Some(&path), &[("APIMAIL_IMAP_PASSWORD", LEAKY_PASSWORD)]);
    let state = build_state(&config, Arc::new(FakeWebhook::accepting()));
    state
        .idle()
        .queue()
        .enqueue(notification(43))
        .await
        .expect("enqueue");

    let mode = std::fs::metadata(&path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "the queue file must be owner-only");

    let contents = std::fs::read_to_string(&path).expect("the queue file must be readable");
    assert!(
        !contents.is_empty(),
        "the enqueued notification must have been written"
    );
    assert!(
        !contents.contains(LEAKY_PASSWORD),
        "the queue file must not contain the IMAP password"
    );

    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn dropping_is_counted_on_overflow() {
    let path = temp_queue_path("overflow");
    let config = config_from(Some(&path), &[("APIMAIL_QUEUE_MAX_ITEMS", "1")]);
    let state = build_state(&config, Arc::new(FakeWebhook::accepting()));

    state
        .idle()
        .queue()
        .enqueue(notification(41))
        .await
        .expect("enqueue");
    state
        .idle()
        .queue()
        .enqueue(notification(42))
        .await
        .expect("enqueue");

    let app = build_router(state);
    let json = queue_state(&app).await;
    assert_eq!(json["pending"], serde_json::json!(1));
    assert_eq!(json["dropped"], serde_json::json!(1));

    let _ = std::fs::remove_file(&path);
}
