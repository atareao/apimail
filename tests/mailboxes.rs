//! Integration tests for the mailbox endpoints (`GET /api/mailboxes` and
//! `POST /api/mailboxes/select`) using an in-memory fake connector, so no test
//! ever touches the network.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::imap::{
    Capabilities, FetchFormat, FlagQuery, ImapConnector, ImapError, ImapSession, Message,
    SearchCriteria, SendFuture as ImapSendFuture,
};
use apimail::smtp::{MailSender, OutgoingMessage, SendFuture as MailSendFuture};
use apimail::{AppState, Config, MailboxInfo, MailboxStatus, build_router};

const API_KEY: &str = "test-mailboxes-key";
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

/// A [`MailSender`] that never touches the network and never fails.
struct NoopMailer;

impl MailSender for NoopMailer {
    fn send(&self, _message: OutgoingMessage) -> MailSendFuture<'_> {
        Box::pin(async move { Ok(()) })
    }
}

/// Scripted outcome of a `select` call on a [`FakeSession`].
#[derive(Clone)]
enum SelectResponse {
    /// Return this status.
    Status(MailboxStatus),
    /// Answer `NO`, as when the mailbox does not exist.
    NotFound,
}

/// A fake session with a fixed mailbox list and a scripted `select`.
struct FakeSession {
    /// Mailboxes returned by `list_mailboxes`.
    mailboxes: Vec<MailboxInfo>,
    /// Outcome of `select`.
    select: SelectResponse,
}

impl ImapSession for FakeSession {
    fn noop(&mut self) -> ImapSendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move { Ok(()) })
    }

    fn list_mailboxes(&mut self) -> ImapSendFuture<'_, Result<Vec<MailboxInfo>, ImapError>> {
        let mailboxes = self.mailboxes.clone();
        Box::pin(async move { Ok(mailboxes) })
    }

    fn select(&mut self, _mailbox: String) -> ImapSendFuture<'_, Result<MailboxStatus, ImapError>> {
        let script = self.select.clone();
        Box::pin(async move {
            match script {
                SelectResponse::Status(status) => Ok(status),
                SelectResponse::NotFound => Err(ImapError::MailboxNotFound),
            }
        })
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

/// Scripted outcome of a `connect` call.
#[derive(Clone)]
enum ConnectOutcome {
    /// Hand back a session reporting the connector's mailboxes.
    Session,
    /// Fail with a server-unreachable error.
    Unavailable,
    /// Fail with a protocol-level error that carries third-party text.
    Protocol(ProtocolFailure),
}

/// Protocol failures whose backing error embeds text (hosts, addresses,
/// certificates) that must never reach the client.
#[derive(Clone, Copy)]
enum ProtocolFailure {
    /// An `async-imap` `BAD` rejection.
    Bad,
    /// A TLS handshake failure.
    Tls,
    /// An I/O failure such as a broken socket.
    Io,
}

/// A fake [`ImapConnector`] that can fail outright or hand back a scripted
/// session, and counts calls.
#[derive(Clone)]
struct FakeConnector {
    /// Outcome of every `connect` call.
    outcome: ConnectOutcome,
    /// Mailboxes the established session reports.
    mailboxes: Vec<MailboxInfo>,
    /// `select` outcome of the established session.
    select: SelectResponse,
    /// Number of `connect` calls observed.
    connects: Arc<AtomicUsize>,
}

impl FakeConnector {
    /// A connector whose session lists `mailboxes` and selects `INBOX`.
    fn with_mailboxes(mailboxes: Vec<MailboxInfo>, select: SelectResponse) -> Self {
        Self {
            outcome: ConnectOutcome::Session,
            mailboxes,
            select,
            connects: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A connector that always fails to connect (server unreachable).
    fn unreachable() -> Self {
        Self {
            outcome: ConnectOutcome::Unavailable,
            mailboxes: Vec::new(),
            select: SelectResponse::NotFound,
            connects: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// A connector that always fails with the given protocol error.
    fn protocol(failure: ProtocolFailure) -> Self {
        Self {
            outcome: ConnectOutcome::Protocol(failure),
            mailboxes: Vec::new(),
            select: SelectResponse::NotFound,
            connects: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl ImapConnector for FakeConnector {
    fn connect(&self) -> ImapSendFuture<'_, Result<Box<dyn ImapSession>, ImapError>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let outcome = self.outcome.clone();
        let mailboxes = self.mailboxes.clone();
        let select = self.select.clone();
        Box::pin(async move {
            match outcome {
                ConnectOutcome::Session => {
                    Ok(Box::new(FakeSession { mailboxes, select }) as Box<dyn ImapSession>)
                }
                ConnectOutcome::Unavailable => Err(ImapError::Unavailable(
                    "simulated connect failure".to_string(),
                )),
                ConnectOutcome::Protocol(failure) => Err(match failure {
                    ProtocolFailure::Bad => ImapError::Imap(async_imap::error::Error::Bad(
                        "server said BAD for imap.internal.example".to_string(),
                    )),
                    ProtocolFailure::Tls => {
                        ImapError::Tls("certificate for imap.internal.example rejected".to_string())
                    }
                    ProtocolFailure::Io => {
                        ImapError::Io(std::io::Error::other("socket to 10.0.0.5 failed"))
                    }
                }),
            }
        })
    }
}

/// Builds the router with both services injected, without touching the network.
fn app_with(connector: FakeConnector) -> Router {
    build_router(AppState::with_services(
        &config(),
        Arc::new(NoopMailer),
        Arc::new(connector),
    ))
}

/// One representative mailbox.
fn inbox() -> MailboxInfo {
    MailboxInfo {
        name: "INBOX".to_string(),
        delimiter: Some("/".to_string()),
        attributes: vec!["\\HasNoChildren".to_string()],
    }
}

/// A mailbox in a flat namespace: no delimiter and no attributes.
fn flat_mailbox() -> MailboxInfo {
    MailboxInfo {
        name: "INBOX".to_string(),
        delimiter: None,
        attributes: Vec::new(),
    }
}

/// Sends a `GET /api/mailboxes` request with an optional `Authorization`.
async fn get_mailboxes(app: Router, authorization: Option<&str>) -> Response {
    let mut builder = Request::builder().method("GET").uri("/api/mailboxes");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    app.oneshot(builder.body(Body::empty()).expect("valid request"))
        .await
        .expect("router should respond")
}

/// Sends a `POST /api/mailboxes/select` request with a raw body.
async fn post_select(app: Router, body: &str, authorization: Option<&str>) -> Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/mailboxes/select")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
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

/// Reads a response body as text.
async fn body_text(response: Response) -> String {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body should be readable")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("body should be UTF-8")
}

#[tokio::test]
async fn listing_returns_the_mailboxes() {
    let response = get_mailboxes(
        app_with(FakeConnector::with_mailboxes(
            vec![inbox()],
            SelectResponse::NotFound,
        )),
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
    let mailboxes = json["mailboxes"].as_array().expect("mailboxes array");
    assert_eq!(mailboxes.len(), 1);
    assert_eq!(mailboxes[0]["name"], "INBOX");
    assert_eq!(mailboxes[0]["delimiter"], "/");
    assert_eq!(mailboxes[0]["attributes"][0], "\\HasNoChildren");
}

#[tokio::test]
async fn listing_reports_null_delimiter_and_empty_attributes() {
    let response = get_mailboxes(
        app_with(FakeConnector::with_mailboxes(
            vec![flat_mailbox()],
            SelectResponse::NotFound,
        )),
        Some(&format!("Bearer {API_KEY}")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let mailbox = &json["mailboxes"][0];
    assert_eq!(mailbox["name"], "INBOX");
    assert!(
        mailbox["delimiter"].is_null(),
        "a flat namespace must serialise the delimiter as null: {json}"
    );
    let attributes = mailbox["attributes"]
        .as_array()
        .expect("attributes should be an array");
    assert!(
        attributes.is_empty(),
        "a mailbox without attributes must serialise an empty array: {json}"
    );
}

#[tokio::test]
async fn protocol_errors_return_stable_503_without_third_party_text() {
    for (failure, expected) in [
        (
            ProtocolFailure::Bad,
            "IMAP authentication or command failed",
        ),
        (ProtocolFailure::Tls, "IMAP TLS handshake failed"),
        (ProtocolFailure::Io, "IMAP I/O error"),
    ] {
        let response = get_mailboxes(
            app_with(FakeConnector::protocol(failure)),
            Some(&format!("Bearer {API_KEY}")),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "a protocol error must map to 503"
        );
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let json = body_json(response).await;
        assert_eq!(json["error"], "imap_unavailable");
        assert_eq!(
            json["message"], expected,
            "the 503 body must carry the stable public message"
        );

        let rendered = json.to_string();
        for leak in [
            IMAP_USER,
            IMAP_PASSWORD,
            "imap.internal.example",
            "10.0.0.5",
            "server said",
            "certificate",
            "socket",
        ] {
            assert!(
                !rendered.contains(leak),
                "the 503 body leaked third-party text `{leak}`: {rendered}"
            );
        }
    }
}

#[tokio::test]
async fn listing_reuses_the_live_session() {
    let connector = FakeConnector::with_mailboxes(vec![inbox()], SelectResponse::NotFound);
    let connects = Arc::clone(&connector.connects);
    let app = app_with(connector);

    get_mailboxes(app.clone(), Some(&format!("Bearer {API_KEY}"))).await;
    get_mailboxes(app, Some(&format!("Bearer {API_KEY}"))).await;

    assert_eq!(
        connects.load(Ordering::SeqCst),
        1,
        "a live session must be reused across listings"
    );
}

#[tokio::test]
async fn selecting_returns_the_status() {
    let status = MailboxStatus {
        exists: 42,
        recent: 1,
        unseen: Some(3),
        uid_validity: Some(7),
        uid_next: Some(100),
        flags: vec!["\\Seen".to_string(), "\\Flagged".to_string()],
    };
    let response = post_select(
        app_with(FakeConnector::with_mailboxes(
            vec![inbox()],
            SelectResponse::Status(status),
        )),
        "{\"mailbox\":\"INBOX\"}",
        Some(&format!("Bearer {API_KEY}")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["exists"], 42);
    assert_eq!(json["recent"], 1);
    assert_eq!(json["unseen"], 3);
    assert_eq!(json["uid_validity"], 7);
    assert_eq!(json["uid_next"], 100);
    assert_eq!(json["flags"][0], "\\Seen");
    assert_eq!(json["flags"][1], "\\Flagged");
}

#[tokio::test]
async fn unknown_mailbox_returns_404() {
    let response = post_select(
        app_with(FakeConnector::with_mailboxes(
            vec![inbox()],
            SelectResponse::NotFound,
        )),
        "{\"mailbox\":\"Missing\"}",
        Some(&format!("Bearer {API_KEY}")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let json = body_json(response).await;
    assert_eq!(json["error"], "mailbox_not_found");
    assert!(
        json["message"].is_string(),
        "message should be a string: {json}"
    );
}

#[tokio::test]
async fn malformed_body_returns_400() {
    let response = post_select(
        app_with(FakeConnector::with_mailboxes(
            vec![inbox()],
            SelectResponse::NotFound,
        )),
        "{not json",
        Some(&format!("Bearer {API_KEY}")),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"], "invalid_request");
}

#[tokio::test]
async fn empty_or_blank_mailbox_returns_400() {
    for body in ["{}", "{\"mailbox\":\"\"}", "{\"mailbox\":\"   \"}"] {
        let response = post_select(
            app_with(FakeConnector::with_mailboxes(
                vec![inbox()],
                SelectResponse::Status(MailboxStatus {
                    exists: 0,
                    recent: 0,
                    unseen: None,
                    uid_validity: None,
                    uid_next: None,
                    flags: Vec::new(),
                }),
            )),
            body,
            Some(&format!("Bearer {API_KEY}")),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "body `{body}` should be rejected"
        );
        assert_eq!(body_json(response).await["error"], "invalid_request");
    }
}

#[tokio::test]
async fn unreachable_server_returns_503_without_leaking_secrets() {
    let listing = get_mailboxes(
        app_with(FakeConnector::unreachable()),
        Some(&format!("Bearer {API_KEY}")),
    )
    .await;
    assert_eq!(listing.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_unavailable_without_secrets(body_text(listing).await);

    let selecting = post_select(
        app_with(FakeConnector::unreachable()),
        "{\"mailbox\":\"INBOX\"}",
        Some(&format!("Bearer {API_KEY}")),
    )
    .await;
    assert_eq!(selecting.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_unavailable_without_secrets(body_text(selecting).await);
}

/// Asserts a `503` body carries the stable code and no IMAP credential.
fn assert_unavailable_without_secrets(text: String) {
    assert!(
        text.contains("imap_unavailable"),
        "body should carry the stable code: {text}"
    );
    for secret in [IMAP_USER, IMAP_PASSWORD] {
        assert!(
            !text.contains(secret),
            "the 503 body leaked `{secret}`: {text}"
        );
    }
}

#[tokio::test]
async fn without_api_key_returns_401() {
    let listing = get_mailboxes(
        app_with(FakeConnector::with_mailboxes(
            vec![inbox()],
            SelectResponse::NotFound,
        )),
        None,
    )
    .await;
    assert_eq!(listing.status(), StatusCode::UNAUTHORIZED);

    let selecting = post_select(
        app_with(FakeConnector::with_mailboxes(
            vec![inbox()],
            SelectResponse::NotFound,
        )),
        "{\"mailbox\":\"INBOX\"}",
        None,
    )
    .await;
    assert_eq!(selecting.status(), StatusCode::UNAUTHORIZED);
}
