//! Integration tests for `POST /api/messages` using an in-memory fake mailer.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::smtp::{MailSender, OutgoingMessage, SendFuture, SmtpError};
use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-send-key";
const SMTP_USER: &str = "smtp-user@test.example";
/// Third-party text (a host plus a server banner) that the failing fake reports
/// as the SMTP error. It must never reach the client, so the `502` tests pin its
/// absence explicitly.
const SMTP_SENTINEL: &str = "smtp.internal.example said: 550 rejected";
/// Stable, server-independent message the `502` body must carry (design.md).
const SMTP_PUBLIC_MESSAGE: &str = "SMTP delivery failed";

/// A [`MailSender`] that records every message it is asked to send and can be
/// configured to fail, all without touching the network.
#[derive(Clone, Default)]
struct FakeMailer {
    /// Messages captured so far, shared across clones.
    sent: Arc<Mutex<Vec<OutgoingMessage>>>,
    /// When `true`, every `send` fails with an [`SmtpError::Delivery`].
    fail: bool,
}

impl FakeMailer {
    /// A fake that always fails, to exercise the `502` path.
    fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    /// A snapshot of the captured messages.
    fn messages(&self) -> Vec<OutgoingMessage> {
        self.sent.lock().expect("mailer lock poisoned").clone()
    }
}

impl MailSender for FakeMailer {
    fn send(&self, message: OutgoingMessage) -> SendFuture<'_> {
        let sent = Arc::clone(&self.sent);
        let fail = self.fail;
        Box::pin(async move {
            sent.lock().expect("mailer lock poisoned").push(message);
            if fail {
                Err(SmtpError::Delivery(SMTP_SENTINEL.to_string()))
            } else {
                Ok(())
            }
        })
    }
}

/// Loads a valid [`Config`] with a known SMTP user and attachment limit.
fn config(max_attachment_bytes: usize) -> Config {
    let max = max_attachment_bytes.to_string();
    Config::from_lookup(move |key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some("imap.example.com".to_string()),
        "APIMAIL_IMAP_USER" => Some("imap-user".to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some("imap-pass".to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.example.com".to_string()),
        "APIMAIL_SMTP_USER" => Some(SMTP_USER.to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-pass".to_string()),
        "APIMAIL_MAX_ATTACHMENT_BYTES" => Some(max.clone()),
        _ => None,
    })
    .expect("valid test config")
}

/// Builds the router with the fake mailer injected, without touching the network.
fn app(mailer: &FakeMailer, max_attachment_bytes: usize) -> Router {
    build_router(
        AppState::with_mailer(&config(max_attachment_bytes), Arc::new(mailer.clone()))
            .expect("building the app state must not touch the network"),
    )
}

/// Sends a JSON body to `POST /api/messages`, optionally authenticated.
async fn post_json(app: Router, body: &serde_json::Value, authorized: bool) -> Response {
    raw_request(app, &body.to_string(), authorized).await
}

/// Sends a raw byte body to `POST /api/messages`, optionally authenticated.
async fn raw_request(app: Router, body: &str, authorized: bool) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/messages")
        .header(header::CONTENT_TYPE, "application/json");
    if authorized {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {API_KEY}"));
    }
    app.oneshot(
        builder
            .body(Body::from(body.to_string()))
            .expect("valid request"),
    )
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

#[tokio::test]
async fn minimal_message_is_sent_and_defaults_the_sender() {
    let mailer = FakeMailer::default();
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({ "to": ["dest@test.example"], "subject": "hi" }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(body_json(response).await["status"], "sent");

    let sent = mailer.messages();
    assert_eq!(sent.len(), 1, "the fake should have received one message");
    assert_eq!(sent[0].from.as_deref(), Some(SMTP_USER));
    assert_eq!(sent[0].to, vec!["dest@test.example".to_string()]);
    assert_eq!(sent[0].subject, "hi");
}

#[tokio::test]
async fn explicit_sender_is_used() {
    let mailer = FakeMailer::default();
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({
            "from": "explicit@test.example",
            "to": ["dest@test.example"],
            "subject": "hi"
        }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        mailer.messages()[0].from.as_deref(),
        Some("explicit@test.example")
    );
}

#[tokio::test]
async fn message_with_attachment_and_body_parts_is_sent() {
    let mailer = FakeMailer::default();
    let encoded = STANDARD.encode(b"attachment bytes");
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({
            "to": ["dest@test.example"],
            "subject": "with attachment",
            "text": "plain",
            "html": "<p>html</p>",
            "attachments": [
                { "filename": "note.txt", "content_type": "text/plain", "data_base64": encoded }
            ]
        }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let sent = mailer.messages();
    assert_eq!(sent[0].attachments[0].filename, "note.txt");
    assert_eq!(sent[0].attachments[0].data, b"attachment bytes");
    assert_eq!(sent[0].text.as_deref(), Some("plain"));
    assert_eq!(sent[0].html.as_deref(), Some("<p>html</p>"));
}

#[tokio::test]
async fn missing_recipient_is_400() {
    let mailer = FakeMailer::default();
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({ "subject": "no recipient" }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"], "invalid_request");
    assert!(mailer.messages().is_empty());
}

#[tokio::test]
async fn invalid_address_is_400() {
    let mailer = FakeMailer::default();
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({ "to": ["not an address"], "subject": "hi" }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"], "invalid_request");
    assert!(mailer.messages().is_empty());
}

#[tokio::test]
async fn malformed_json_is_400() {
    let mailer = FakeMailer::default();
    let response = raw_request(app(&mailer, 1024), "{ not valid json", true).await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"], "invalid_request");
}

#[tokio::test]
async fn invalid_base64_attachment_is_400() {
    let mailer = FakeMailer::default();
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({
            "to": ["dest@test.example"],
            "subject": "hi",
            "attachments": [
                { "filename": "broken.bin", "data_base64": "not base64!!" }
            ]
        }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"], "invalid_request");
    assert!(mailer.messages().is_empty());
}

#[tokio::test]
async fn oversized_attachment_is_413() {
    let mailer = FakeMailer::default();
    let encoded = STANDARD.encode([0u8; 10]);
    let response = post_json(
        app(&mailer, 4),
        &serde_json::json!({
            "to": ["dest@test.example"],
            "subject": "too big",
            "attachments": [{ "filename": "big.bin", "data_base64": encoded }]
        }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body_json(response).await["error"], "payload_too_large");
    assert!(mailer.messages().is_empty());
}

#[tokio::test]
async fn attachment_exactly_at_the_limit_is_sent() {
    let mailer = FakeMailer::default();
    let encoded = STANDARD.encode([7u8; 1024]);
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({
            "to": ["dest@test.example"],
            "subject": "at the limit",
            "attachments": [{ "filename": "exact.bin", "data_base64": encoded }]
        }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let sent = mailer.messages();
    assert_eq!(sent[0].attachments[0].data.len(), 1024);
}

#[tokio::test]
async fn attachment_one_byte_over_the_limit_is_413() {
    let mailer = FakeMailer::default();
    let encoded = STANDARD.encode([7u8; 1025]);
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({
            "to": ["dest@test.example"],
            "subject": "one byte over",
            "attachments": [{ "filename": "over.bin", "data_base64": encoded }]
        }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body_json(response).await["error"], "payload_too_large");
    assert!(mailer.messages().is_empty());
}

#[tokio::test]
async fn oversized_raw_body_is_413_with_json_envelope() {
    let mailer = FakeMailer::default();
    // Far larger than the body limit for a 1024-byte attachment limit.
    let huge = "A".repeat(200_000);
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({
            "to": ["dest@test.example"],
            "subject": "huge body",
            "attachments": [{ "filename": "big.bin", "data_base64": huge }]
        }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let json = body_json(response).await;
    assert_eq!(json["error"], "payload_too_large");
    assert!(json["message"].is_string(), "message should be a string");
    assert!(mailer.messages().is_empty());
}

#[test]
fn app_state_debug_redacts_mail_secrets() {
    let state = AppState::from_config(&config(1024)).expect("valid app state");
    let debug = format!("{state:?}");
    for secret in [SMTP_USER, "smtp-pass", API_KEY] {
        assert!(!debug.contains(secret), "Debug leaked `{secret}`: {debug}");
    }
    assert!(
        debug.contains("***"),
        "Debug is missing the redaction marker: {debug}"
    );
}

#[tokio::test]
async fn smtp_failure_is_502() {
    let mailer = FakeMailer::failing();
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({ "to": ["dest@test.example"], "subject": "hi" }),
        true,
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let json = body_json(response).await;
    assert_eq!(json["error"], "smtp_error");
    assert_eq!(json["message"], SMTP_PUBLIC_MESSAGE);
    let message = json["message"]
        .as_str()
        .expect("message should be a string");
    assert!(
        !message.contains("smtp.internal.example"),
        "the 502 leaked a host name: {message}"
    );
    assert!(
        !message.contains("550 rejected"),
        "the 502 leaked the server banner: {message}"
    );
    assert!(
        !message.contains(SMTP_SENTINEL),
        "the 502 leaked the raw upstream error: {message}"
    );
}

#[tokio::test]
async fn missing_api_key_is_401() {
    let mailer = FakeMailer::default();
    let response = post_json(
        app(&mailer, 1024),
        &serde_json::json!({ "to": ["dest@test.example"], "subject": "hi" }),
        false,
    )
    .await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(mailer.messages().is_empty());
}
