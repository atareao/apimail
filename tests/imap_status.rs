//! Integration tests for `GET /api/imap/status` using an in-memory fake
//! connector, so no test ever touches the network.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::imap::{
    Capabilities, FetchFormat, FlagQuery, ImapConnector, ImapError, ImapSession, MailboxInfo,
    MailboxStatus, Message, SearchCriteria, SendFuture as ImapSendFuture,
};
use apimail::smtp::{MailSender, OutgoingMessage, SendFuture as MailSendFuture};
use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-imap-key";
const IMAP_HOST: &str = "imap.test.example";
const IMAP_PORT: u16 = 1993;
const IMAP_USER: &str = "imap-user@test.example";
const IMAP_PASSWORD: &str = "imap-secret";

/// Loads a valid [`Config`] with a known IMAP endpoint and password.
fn config() -> Config {
    Config::from_lookup(|key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some(IMAP_HOST.to_string()),
        "APIMAIL_IMAP_PORT" => Some(IMAP_PORT.to_string()),
        "APIMAIL_IMAP_USER" => Some(IMAP_USER.to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some(IMAP_PASSWORD.to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.test.example".to_string()),
        "APIMAIL_SMTP_USER" => Some("smtp-user@test.example".to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-secret".to_string()),
        _ => None,
    })
    .expect("valid test config")
}

/// Loads a [`Config`] whose IMAP host cannot resolve, to prove construction does
/// not open the network.
fn config_with_unreachable_host() -> Config {
    Config::from_lookup(|key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some("imap.invalid".to_string()),
        "APIMAIL_IMAP_USER" => Some(IMAP_USER.to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some(IMAP_PASSWORD.to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.test.example".to_string()),
        "APIMAIL_SMTP_USER" => Some("smtp-user@test.example".to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-secret".to_string()),
        _ => None,
    })
    .expect("valid test config")
}

/// Loads a [`Config`] with an explicit IMAP TLS mode.
fn config_with_tls(mode: &str) -> Config {
    Config::from_lookup(move |key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some(IMAP_HOST.to_string()),
        "APIMAIL_IMAP_USER" => Some(IMAP_USER.to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some(IMAP_PASSWORD.to_string()),
        "APIMAIL_IMAP_TLS" => Some(mode.to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.test.example".to_string()),
        "APIMAIL_SMTP_USER" => Some("smtp-user@test.example".to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-secret".to_string()),
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

/// A fake session whose `noop` outcome is fixed.
struct FakeSession {
    /// When `true`, `noop` fails.
    dead: bool,
}

impl ImapSession for FakeSession {
    fn noop(&mut self) -> ImapSendFuture<'_, Result<(), ImapError>> {
        let dead = self.dead;
        Box::pin(async move {
            if dead {
                Err(ImapError::Unavailable("simulated dead session".to_string()))
            } else {
                Ok(())
            }
        })
    }

    fn list_mailboxes(&mut self) -> ImapSendFuture<'_, Result<Vec<MailboxInfo>, ImapError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn select(&mut self, _mailbox: String) -> ImapSendFuture<'_, Result<MailboxStatus, ImapError>> {
        Box::pin(async move { Err(ImapError::MailboxNotFound) })
    }

    fn search(
        &mut self,
        _criteria: SearchCriteria,
    ) -> ImapSendFuture<'_, Result<Vec<u32>, ImapError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn fetch(
        &mut self,
        _uids: Vec<u32>,
        _format: FetchFormat,
    ) -> ImapSendFuture<'_, Result<Vec<Message>, ImapError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn store(&mut self, _uid: u32, _query: FlagQuery) -> ImapSendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move { Ok(()) })
    }

    fn copy(&mut self, _uid: u32, _mailbox: String) -> ImapSendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move { Ok(()) })
    }

    fn move_message(
        &mut self,
        _uid: u32,
        _mailbox: String,
    ) -> ImapSendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move { Ok(()) })
    }

    fn uid_expunge(&mut self, _uid: u32) -> ImapSendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move { Ok(()) })
    }

    fn capabilities(&mut self) -> ImapSendFuture<'_, Result<Capabilities, ImapError>> {
        Box::pin(async move { Ok(Capabilities::default()) })
    }
}

/// A fake [`ImapConnector`] that succeeds or fails on demand and counts calls.
#[derive(Clone)]
struct FakeConnector {
    /// When `true`, every `connect` fails.
    fail: bool,
    /// Number of `connect` calls observed.
    connects: Arc<AtomicUsize>,
}

impl FakeConnector {
    /// A connector that always succeeds.
    fn ok() -> Self {
        Self {
            fail: false,
            connects: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A connector that always fails.
    fn failing() -> Self {
        Self {
            fail: true,
            connects: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ImapConnector for FakeConnector {
    fn connect(&self) -> ImapSendFuture<'_, Result<Box<dyn ImapSession>, ImapError>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let fail = self.fail;
        Box::pin(async move {
            if fail {
                Err(ImapError::Unavailable(
                    "simulated connect failure".to_string(),
                ))
            } else {
                Ok(Box::new(FakeSession { dead: false }) as Box<dyn ImapSession>)
            }
        })
    }
}

/// Builds the router with both services injected, without touching the network.
fn app_with(config: &Config, connector: FakeConnector) -> Router {
    build_router(
        AppState::with_services(config, Arc::new(NoopMailer), Arc::new(connector))
            .expect("building the app state must not touch the network"),
    )
}

/// Sends a `GET /api/imap/status` request with an optional `Authorization`.
async fn status(app: Router, authorization: Option<&str>) -> Response {
    let mut builder = Request::builder().uri("/api/imap/status");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    app.oneshot(builder.body(Body::empty()).expect("valid request"))
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
async fn connected_returns_200_with_endpoint_details() {
    let response = status(
        app_with(&config(), FakeConnector::ok()),
        Some(&format!("Bearer {API_KEY}")),
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

    let json = body_json(response).await;
    assert_eq!(json["connected"], true);
    assert_eq!(json["host"], IMAP_HOST);
    assert_eq!(json["port"], IMAP_PORT);
    assert_eq!(json["tls"], "implicit");

    let text = json.to_string();
    for secret in [IMAP_USER, IMAP_PASSWORD] {
        assert!(
            !text.contains(secret),
            "status response leaked `{secret}`: {text}"
        );
    }
}

#[tokio::test]
async fn status_reports_the_configured_tls_mode() {
    let response = status(
        app_with(&config_with_tls("starttls"), FakeConnector::ok()),
        Some(&format!("Bearer {API_KEY}")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["tls"], "starttls");
}

#[tokio::test]
async fn unreachable_server_returns_503() {
    let response = status(
        app_with(&config(), FakeConnector::failing()),
        Some(&format!("Bearer {API_KEY}")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );

    let json = body_json(response).await;
    assert_eq!(json["connected"], false);
    assert_eq!(json["error"], "imap_unavailable");
    assert!(
        json["message"].is_string(),
        "message should be a string: {json}"
    );
    let text = json.to_string();
    for secret in [IMAP_USER, IMAP_PASSWORD] {
        assert!(
            !text.contains(secret),
            "the 503 body leaked `{secret}`: {text}"
        );
    }
}

#[tokio::test]
async fn without_api_key_returns_401() {
    let response = status(app_with(&config(), FakeConnector::ok()), None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn from_config_with_unreachable_host_is_ok_and_does_not_leak_secrets() {
    // Building the state must not open a connection: an unresolvable host is
    // fine, and the construction succeeds.
    let state = AppState::from_config(&config_with_unreachable_host())
        .expect("building the app state must not touch the network");

    let debug = format!("{state:?}");
    for secret in [IMAP_USER, IMAP_PASSWORD, API_KEY] {
        assert!(!debug.contains(secret), "Debug leaked `{secret}`: {debug}");
    }
}
