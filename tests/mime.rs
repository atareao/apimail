//! Integration tests for the MIME parsing endpoints
//! (`GET /api/messages/{uid}/body`, `GET /api/messages/{uid}/attachments` and
//! `GET /api/messages/{uid}/attachments/{id}`) using an in-memory fake connector,
//! so no test ever touches the network.
//!
//! The fake session serves a single RFC822 fixture and **records** the fetch
//! formats it receives, so the tests can assert that an oversized message is
//! rejected from its `Summary` size without a full fetch ever being issued.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::imap::{
    Capabilities, FetchFormat, FlagQuery, ImapConnector, ImapError, ImapSession, MailboxInfo,
    MailboxStatus, Message, SearchCriteria, SendFuture as ImapSendFuture,
};
use apimail::smtp::{MailSender, OutgoingMessage, SendFuture as MailSendFuture};
use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-mime-key";
const IMAP_HOST: &str = "imap.test.example";
const IMAP_PORT: u16 = 1993;
const IMAP_USER: &str = "imap-user@test.example";
const IMAP_PASSWORD: &str = "imap-secret";

/// A multipart RFC822 fixture: a `multipart/alternative` (quoted-printable
/// `text/plain` plus `text/html`) inside a `multipart/mixed`, with an
/// `application/pdf` attachment named `informe.pdf` and an inline `image/png`
/// carrying a `Content-ID`.
const RAW: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Multipart test
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary=\"MIX\"

--MIX
Content-Type: multipart/alternative; boundary=\"ALT\"

--ALT
Content-Type: text/plain; charset=\"utf-8\"
Content-Transfer-Encoding: quoted-printable

Hola =C2=A1mundo!
--ALT
Content-Type: text/html; charset=\"utf-8\"

<p>Hola</p>
--ALT--

--MIX
Content-Type: application/pdf
Content-Disposition: attachment; filename=\"informe.pdf\"
Content-Transfer-Encoding: base64

UERGIGNvbnRlbnQhCg==
--MIX
Content-Type: image/png
Content-Disposition: inline
Content-ID: <logo@example.com>
Content-Transfer-Encoding: base64

AAECAwQ=
--MIX--
";

/// The decoded bytes of the `application/pdf` attachment in [`RAW`].
const PDF_CONTENT: &[u8] = b"PDF content!\n";

/// A message whose only part is a binary attachment (no displayable body).
const ATTACHMENT_ONLY: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Binary only
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary=\"B\"

--B
Content-Type: application/octet-stream
Content-Disposition: attachment; filename=\"data.bin\"
Content-Transfer-Encoding: base64

AAECAwQ=
--B--
";

/// A message that only carries a plain-text body (no attachments).
const TEXT_ONLY: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Text only
MIME-Version: 1.0
Content-Type: text/plain; charset=\"utf-8\"

Hello plain world
";

/// A message that only carries an HTML body (no attachments).
const HTML_ONLY: &str = "\
From: Alice <alice@example.com>
To: Bob <bob@example.com>
Subject: Html only
MIME-Version: 1.0
Content-Type: text/html; charset=\"utf-8\"

<p>Hello <b>html</b> world</p>
";

/// Loads a valid [`Config`] with the default message-size limit.
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

/// Loads a valid [`Config`] with a tiny message-size limit, for the `413` case.
fn config_with_max_message_bytes(max: &str) -> Config {
    Config::from_lookup(|key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some(IMAP_HOST.to_string()),
        "APIMAIL_IMAP_PORT" => Some(IMAP_PORT.to_string()),
        "APIMAIL_IMAP_USER" => Some(IMAP_USER.to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some(IMAP_PASSWORD.to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.test.example".to_string()),
        "APIMAIL_SMTP_USER" => Some("smtp-user@test.example".to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-secret".to_string()),
        "APIMAIL_MAX_MESSAGE_BYTES" => Some(max.to_string()),
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

/// Outcome of a `select` call on a [`FakeSession`].
#[derive(Clone, Copy)]
enum SelectOutcome {
    /// Answer with a default status.
    Ok,
    /// Answer `NO`, as when the mailbox does not exist.
    NotFound,
}

/// Outcome of a `connect` call.
#[derive(Clone, Copy)]
enum ConnectOutcome {
    /// Hand back a session.
    Session,
    /// Fail as a server-unreachable error.
    Unavailable,
}

/// A fake session that serves the fixture and records the fetch formats.
struct FakeSession {
    /// Outcome of `select`.
    select: SelectOutcome,
    /// Whether the message exists at all.
    present: bool,
    /// Size reported by the `Summary` fetch (`None` ⇒ the raw length).
    summary_size: Option<u32>,
    /// Raw RFC822 bytes served by the `Full` fetch.
    raw: Vec<u8>,
    /// Mailboxes received by `select`, in call order.
    selected: Arc<Mutex<Vec<String>>>,
    /// Fetch formats received, in call order.
    fetches: Arc<Mutex<Vec<FetchFormat>>>,
}

/// Builds a [`Message`] carrying only the fields the parsing path reads.
fn message(uid: u32, size: Option<u32>, body: Option<Vec<u8>>) -> Message {
    Message {
        uid,
        seq: uid,
        flags: Vec::new(),
        size,
        internal_date: None,
        envelope: None,
        headers: None,
        body,
    }
}

impl ImapSession for FakeSession {
    fn noop(&mut self) -> ImapSendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move { Ok(()) })
    }

    fn list_mailboxes(&mut self) -> ImapSendFuture<'_, Result<Vec<MailboxInfo>, ImapError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn select(&mut self, mailbox: String) -> ImapSendFuture<'_, Result<MailboxStatus, ImapError>> {
        self.selected
            .lock()
            .expect("selected log poisoned")
            .push(mailbox);
        let select = self.select;
        Box::pin(async move {
            match select {
                SelectOutcome::Ok => Ok(MailboxStatus {
                    exists: 0,
                    recent: 0,
                    unseen: None,
                    uid_validity: None,
                    uid_next: None,
                    flags: Vec::new(),
                }),
                SelectOutcome::NotFound => Err(ImapError::MailboxNotFound),
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
        uids: Vec<u32>,
        format: FetchFormat,
    ) -> ImapSendFuture<'_, Result<Vec<Message>, ImapError>> {
        self.fetches
            .lock()
            .expect("fetch log poisoned")
            .push(format);
        let present = self.present;
        let uid = uids.first().copied().unwrap_or(0);
        let size = self.summary_size.unwrap_or(self.raw.len() as u32);
        let raw = self.raw.clone();
        Box::pin(async move {
            if !present {
                return Ok(Vec::new());
            }
            let messages = match format {
                FetchFormat::Summary => vec![message(uid, Some(size), None)],
                FetchFormat::Full => vec![message(uid, None, Some(raw))],
                FetchFormat::Headers => Vec::new(),
            };
            Ok(messages)
        })
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

/// A fake [`ImapConnector`] with a scripted session and request logs.
#[derive(Clone)]
struct FakeConnector {
    /// Outcome of every `connect` call.
    outcome: ConnectOutcome,
    /// `select` outcome of the established session.
    select: SelectOutcome,
    /// Whether the message exists.
    present: bool,
    /// Overridden `Summary` size (`None` ⇒ the raw length).
    summary_size: Option<u32>,
    /// Raw RFC822 bytes served by the `Full` fetch.
    raw: Vec<u8>,
    /// Mailboxes received by `select`, in call order.
    selected: Arc<Mutex<Vec<String>>>,
    /// Fetch formats received, in call order.
    fetches: Arc<Mutex<Vec<FetchFormat>>>,
}

impl FakeConnector {
    /// A connector whose session succeeds at selecting the fixture message.
    fn new(outcome: ConnectOutcome) -> Self {
        Self {
            outcome,
            select: SelectOutcome::Ok,
            present: true,
            summary_size: None,
            raw: RAW.as_bytes().to_vec(),
            selected: Arc::new(Mutex::new(Vec::new())),
            fetches: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Sets the `select` outcome.
    fn with_select(mut self, select: SelectOutcome) -> Self {
        self.select = select;
        self
    }

    /// Marks the message as absent (the server reports no such `uid`).
    fn without_message(mut self) -> Self {
        self.present = false;
        self
    }

    /// Overrides the size reported by the `Summary` fetch.
    fn with_summary_size(mut self, size: u32) -> Self {
        self.summary_size = Some(size);
        self
    }

    /// Replaces the raw RFC822 bytes served by the `Full` fetch.
    fn with_raw(mut self, raw: &[u8]) -> Self {
        self.raw = raw.to_vec();
        self
    }
}

impl ImapConnector for FakeConnector {
    fn connect(&self) -> ImapSendFuture<'_, Result<Box<dyn ImapSession>, ImapError>> {
        let outcome = self.outcome;
        let select = self.select;
        let present = self.present;
        let summary_size = self.summary_size;
        let raw = self.raw.clone();
        let selected = Arc::clone(&self.selected);
        let fetches = Arc::clone(&self.fetches);
        Box::pin(async move {
            match outcome {
                ConnectOutcome::Session => Ok(Box::new(FakeSession {
                    select,
                    present,
                    summary_size,
                    raw,
                    selected,
                    fetches,
                }) as Box<dyn ImapSession>),
                ConnectOutcome::Unavailable => Err(ImapError::Unavailable(
                    "simulated connect failure".to_string(),
                )),
            }
        })
    }
}

/// Builds the router with both services injected, without touching the network.
fn app(config: Config, connector: FakeConnector) -> Router {
    build_router(
        AppState::with_services(&config, Arc::new(NoopMailer), Arc::new(connector))
            .expect("building the app state must not touch the network"),
    )
}

/// Builds the router with the default config.
fn app_with(connector: FakeConnector) -> Router {
    app(config(), connector)
}

/// Sends a `GET` request with an optional `Authorization`.
async fn get(app: Router, uri: &str, authorization: Option<&str>) -> Response {
    let mut builder = Request::builder().method("GET").uri(uri);
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

/// The `Content-Type` of a response, if present.
fn content_type(response: &Response) -> Option<&str> {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
}

/// The bearer header value for the test API key.
fn bearer() -> String {
    format!("Bearer {API_KEY}")
}

#[tokio::test]
async fn body_reports_the_parsed_text_and_html() {
    let connector = FakeConnector::new(ConnectOutcome::Session);
    let fetches = Arc::clone(&connector.fetches);
    let app = app_with(connector);

    let response = get(app, "/api/messages/42/body?mailbox=INBOX", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), Some("application/json"));

    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["uid"], 42);
    assert_eq!(json["text"], "Hola ¡mundo!");
    assert_eq!(json["html"], "<p>Hola</p>");

    assert_eq!(
        fetches.lock().expect("fetch log poisoned").as_slice(),
        [FetchFormat::Summary, FetchFormat::Full],
        "the body handler must probe with Summary then fetch the Full source"
    );
}

#[tokio::test]
async fn body_without_a_displayable_part_is_null() {
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_raw(ATTACHMENT_ONLY.as_bytes());
    let app = app_with(connector);

    let response = get(app, "/api/messages/42/body?mailbox=INBOX", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["text"], serde_json::Value::Null);
    assert_eq!(json["html"], serde_json::Value::Null);
}

#[tokio::test]
async fn body_derives_the_missing_alternative() {
    // A text-only message must still report both bodies: the HTML alternative
    // is derived from the plain text (RFC 8621 §4.1.4).
    let text_only = get(
        app_with(FakeConnector::new(ConnectOutcome::Session).with_raw(TEXT_ONLY.as_bytes())),
        "/api/messages/42/body?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(text_only.status(), StatusCode::OK);
    let json = body_json(text_only).await;
    let text = json["text"].as_str().expect("derived text body");
    let html = json["html"].as_str().expect("derived html body");
    assert!(text.contains("Hello plain world"), "{text}");
    assert!(html.contains("Hello plain world"), "{html}");

    // And symmetrically for an HTML-only message.
    let html_only = get(
        app_with(FakeConnector::new(ConnectOutcome::Session).with_raw(HTML_ONLY.as_bytes())),
        "/api/messages/42/body?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(html_only.status(), StatusCode::OK);
    let json = body_json(html_only).await;
    let text = json["text"].as_str().expect("derived text body");
    let html = json["html"].as_str().expect("html body");
    assert!(html.contains("<b>html</b>"), "{html}");
    assert!(text.contains("Hello"), "{text}");
}

#[tokio::test]
async fn attachments_list_metadata_without_content() {
    let app = app_with(FakeConnector::new(ConnectOutcome::Session));

    let response = get(
        app,
        "/api/messages/42/attachments?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), Some("application/json"));

    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["uid"], 42);

    let attachments = json["attachments"].as_array().expect("attachments array");
    assert_eq!(attachments.len(), 2, "{attachments:?}");

    let pdf = &attachments[0];
    assert_eq!(pdf["id"], 0);
    assert_eq!(pdf["filename"], "informe.pdf");
    assert_eq!(pdf["content_type"], "application/pdf");
    assert_eq!(pdf["size"], 13);
    assert_eq!(pdf["inline"], false);
    assert_eq!(pdf["content_id"], serde_json::Value::Null);
    assert!(
        pdf.get("content").is_none() && pdf.get("content_base64").is_none(),
        "the listing must not carry the attachment content: {pdf}"
    );

    let inline = &attachments[1];
    assert_eq!(inline["id"], 1);
    assert_eq!(inline["content_type"], "image/png");
    assert_eq!(inline["size"], 5);
    assert_eq!(inline["inline"], true);
    assert_eq!(inline["content_id"], "logo@example.com");
    assert!(
        inline.get("content").is_none() && inline.get("content_base64").is_none(),
        "the listing must not carry the attachment content: {inline}"
    );
}

#[tokio::test]
async fn attachment_ids_are_stable_across_listings() {
    // The same message listed twice must keep each attachment's positional id.
    let first = get(
        app_with(FakeConnector::new(ConnectOutcome::Session)),
        "/api/messages/42/attachments?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first_ids: Vec<u64> = body_json(first).await["attachments"]
        .as_array()
        .expect("attachments array")
        .iter()
        .map(|attachment| attachment["id"].as_u64().expect("numeric id"))
        .collect();
    assert_eq!(first_ids, vec![0, 1], "positional ids must start at 0");

    let second = get(
        app_with(FakeConnector::new(ConnectOutcome::Session)),
        "/api/messages/42/attachments?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    let second_ids: Vec<u64> = body_json(second).await["attachments"]
        .as_array()
        .expect("attachments array")
        .iter()
        .map(|attachment| attachment["id"].as_u64().expect("numeric id"))
        .collect();
    assert_eq!(
        first_ids, second_ids,
        "listing the same message twice must yield the same positional ids"
    );
}

#[tokio::test]
async fn attachments_is_empty_when_the_message_has_none() {
    let connector = FakeConnector::new(ConnectOutcome::Session).with_raw(TEXT_ONLY.as_bytes());
    let app = app_with(connector);

    let response = get(
        app,
        "/api/messages/42/attachments?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(
        json["attachments"]
            .as_array()
            .expect("attachments array")
            .is_empty(),
        "{json}"
    );
}

#[tokio::test]
async fn attachment_returns_the_base64_encoded_content() {
    let app = app_with(FakeConnector::new(ConnectOutcome::Session));

    let response = get(
        app,
        "/api/messages/42/attachments/0?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), Some("application/json"));

    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["uid"], 42);
    assert_eq!(json["id"], 0);
    assert_eq!(json["filename"], "informe.pdf");
    assert_eq!(json["content_type"], "application/pdf");
    assert_eq!(json["size"], 13);

    let encoded = json["content_base64"]
        .as_str()
        .expect("content_base64 string");
    assert_eq!(STANDARD.decode(encoded).expect("valid base64"), PDF_CONTENT);
}

#[tokio::test]
async fn unknown_attachment_is_404() {
    let app = app_with(FakeConnector::new(ConnectOutcome::Session));

    let response = get(
        app,
        "/api/messages/42/attachments/9?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(content_type(&response), Some("application/json"));
    let json = body_json(response).await;
    assert_eq!(json["error"], "attachment_not_found");
    assert_eq!(json["message"], "attachment not found");
}

#[tokio::test]
async fn oversized_message_is_413_without_a_full_fetch() {
    // The fixture is well above 10 bytes; the size guard must reject it from the
    // `Summary` report, before the full source is ever requested.
    let connector = FakeConnector::new(ConnectOutcome::Session).with_summary_size(11);
    let fetches = Arc::clone(&connector.fetches);
    let app = app(config_with_max_message_bytes("10"), connector);

    let response = get(app, "/api/messages/42/body?mailbox=INBOX", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let json = body_json(response).await;
    assert_eq!(json["error"], "message_too_large");
    assert_eq!(json["message"], "message exceeds the configured size limit");

    let fetches = fetches.lock().expect("fetch log poisoned");
    assert_eq!(fetches.as_slice(), [FetchFormat::Summary]);
    assert!(
        !fetches.contains(&FetchFormat::Full),
        "an oversized message must not be fetched in full: {fetches:?}"
    );
}

#[tokio::test]
async fn oversized_message_is_413_after_the_full_fetch_when_the_reported_size_lies() {
    // `RFC822.SIZE` is checked first (here it understates the message: 5 ≤ 20),
    // so the Full fetch runs; the length of the bytes actually fetched is what
    // rejects it.
    let connector = FakeConnector::new(ConnectOutcome::Session).with_summary_size(5);
    let fetches = Arc::clone(&connector.fetches);
    let app = app(config_with_max_message_bytes("20"), connector);

    let response = get(app, "/api/messages/42/body?mailbox=INBOX", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let json = body_json(response).await;
    assert_eq!(json["error"], "message_too_large");
    assert_eq!(json["message"], "message exceeds the configured size limit");

    let fetches = fetches.lock().expect("fetch log poisoned");
    assert_eq!(
        fetches.as_slice(),
        [FetchFormat::Summary, FetchFormat::Full],
        "an understated reported size must still fetch in full and be rejected on the real length"
    );
}

#[tokio::test]
async fn oversized_message_is_413_on_the_attachment_routes() {
    for uri in [
        "/api/messages/42/attachments?mailbox=INBOX",
        "/api/messages/42/attachments/0?mailbox=INBOX",
    ] {
        let connector = FakeConnector::new(ConnectOutcome::Session).with_summary_size(11);
        let app = app(config_with_max_message_bytes("10"), connector);

        let response = get(app, uri, Some(&bearer())).await;

        assert_eq!(
            response.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "`{uri}` should be a 413"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"], "message_too_large");
        assert_eq!(json["message"], "message exceeds the configured size limit");
    }
}

#[tokio::test]
async fn unparsable_message_is_422() {
    let connector = FakeConnector::new(ConnectOutcome::Session).with_raw(b"");
    let app = app_with(connector);

    let response = get(app, "/api/messages/42/body?mailbox=INBOX", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"], "message_not_parsable");
    assert_eq!(json["message"], "message cannot be parsed");
}

#[tokio::test]
async fn unparsable_message_is_422_on_the_attachment_routes() {
    for uri in [
        "/api/messages/42/attachments?mailbox=INBOX",
        "/api/messages/42/attachments/0?mailbox=INBOX",
    ] {
        let connector = FakeConnector::new(ConnectOutcome::Session).with_raw(b"");
        let app = app_with(connector);

        let response = get(app, uri, Some(&bearer())).await;

        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "`{uri}` should be a 422"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"], "message_not_parsable");
        assert_eq!(json["message"], "message cannot be parsed");
    }
}

#[tokio::test]
async fn invalid_requests_return_400_without_sending_a_command() {
    for uri in [
        "/api/messages/abc/body?mailbox=INBOX",
        "/api/messages/0/body?mailbox=INBOX",
        "/api/messages/42/body",
        "/api/messages/42/body?mailbox=",
        "/api/messages/42/body?mailbox=%20%20",
        "/api/messages/42/body?mailbox=bad%0D%0Avalue",
        "/api/messages/abc/attachments?mailbox=INBOX",
        "/api/messages/0/attachments?mailbox=INBOX",
        "/api/messages/42/attachments",
        "/api/messages/42/attachments?mailbox=",
        "/api/messages/abc/attachments/0?mailbox=INBOX",
        "/api/messages/0/attachments/0?mailbox=INBOX",
        "/api/messages/42/attachments/abc?mailbox=INBOX",
        "/api/messages/42/attachments/-1?mailbox=INBOX",
        "/api/messages/42/attachments/0",
        "/api/messages/42/attachments/0?mailbox=",
        "/api/messages/42/attachments/0?mailbox=bad%0D%0Avalue",
    ] {
        let connector = FakeConnector::new(ConnectOutcome::Session);
        let selected = Arc::clone(&connector.selected);
        let fetches = Arc::clone(&connector.fetches);
        let app = app_with(connector);

        let response = get(app, uri, Some(&bearer())).await;

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "`{uri}` should be a 400"
        );
        assert_eq!(
            body_json(response).await["error"],
            "invalid_request",
            "`{uri}` should carry invalid_request"
        );
        assert!(
            selected.lock().expect("selected log poisoned").is_empty(),
            "`{uri}` must not send an IMAP SELECT"
        );
        assert!(
            fetches.lock().expect("fetch log poisoned").is_empty(),
            "`{uri}` must not send an IMAP FETCH"
        );
    }
}

#[tokio::test]
async fn unknown_mailbox_returns_404() {
    for uri in [
        "/api/messages/42/body?mailbox=Missing",
        "/api/messages/42/attachments?mailbox=Missing",
        "/api/messages/42/attachments/0?mailbox=Missing",
    ] {
        let connector =
            FakeConnector::new(ConnectOutcome::Session).with_select(SelectOutcome::NotFound);
        let app = app_with(connector);

        let response = get(app, uri, Some(&bearer())).await;

        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "`{uri}` should be a 404"
        );
        assert_eq!(content_type(&response), Some("application/json"));
        assert_eq!(body_json(response).await["error"], "mailbox_not_found");
    }
}

#[tokio::test]
async fn unknown_message_returns_404() {
    for uri in [
        "/api/messages/42/body?mailbox=INBOX",
        "/api/messages/42/attachments?mailbox=INBOX",
        "/api/messages/42/attachments/0?mailbox=INBOX",
    ] {
        let app = app_with(FakeConnector::new(ConnectOutcome::Session).without_message());

        let response = get(app, uri, Some(&bearer())).await;

        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "`{uri}` should be a 404"
        );
        assert_eq!(content_type(&response), Some("application/json"));
        assert_eq!(body_json(response).await["error"], "message_not_found");
    }
}

#[tokio::test]
async fn unreachable_server_returns_503_without_leaking_secrets() {
    for uri in [
        "/api/messages/42/body?mailbox=INBOX",
        "/api/messages/42/attachments?mailbox=INBOX",
        "/api/messages/42/attachments/0?mailbox=INBOX",
    ] {
        let app = app_with(FakeConnector::new(ConnectOutcome::Unavailable));

        let response = get(app, uri, Some(&bearer())).await;

        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "`{uri}` should be a 503"
        );
        assert_unavailable_without_secrets(body_json(response).await);
    }
}

/// Asserts a `503` body carries the stable code and no IMAP credential.
fn assert_unavailable_without_secrets(json: serde_json::Value) {
    assert_eq!(json["error"], "imap_unavailable");
    assert!(json["message"].is_string(), "{json}");
    let rendered = json.to_string();
    for leak in [IMAP_USER, IMAP_PASSWORD] {
        assert!(
            !rendered.contains(leak),
            "the 503 body leaked `{leak}`: {rendered}"
        );
    }
}

#[tokio::test]
async fn error_messages_are_fixed_and_free_of_third_party_text() {
    // A `413` whose body must not echo the (untrusted) MIME content.
    let oversized = get(
        app(
            config_with_max_message_bytes("10"),
            FakeConnector::new(ConnectOutcome::Session).with_summary_size(11),
        ),
        "/api/messages/42/body?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let oversized_json = body_json(oversized).await;
    assert_eq!(
        oversized_json["message"],
        "message exceeds the configured size limit"
    );
    assert!(
        !oversized_json["message"]
            .as_str()
            .expect("string")
            .contains("informe"),
        "the 413 message must not echo MIME content: {oversized_json}"
    );
    for leak in [IMAP_USER, IMAP_PASSWORD] {
        assert!(
            !oversized_json.to_string().contains(leak),
            "the 413 body leaked `{leak}`: {oversized_json}"
        );
    }

    // A `422` whose body must not echo the parser's error text.
    let unparsable = get(
        app_with(FakeConnector::new(ConnectOutcome::Session).with_raw(b"")),
        "/api/messages/42/body?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(unparsable.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        body_json(unparsable).await["message"],
        "message cannot be parsed"
    );

    // The two `404`s on this path are fixed strings as well.
    let missing_attachment = get(
        app_with(FakeConnector::new(ConnectOutcome::Session)),
        "/api/messages/42/attachments/9?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(missing_attachment.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(missing_attachment).await["message"],
        "attachment not found"
    );

    let missing_message = get(
        app_with(FakeConnector::new(ConnectOutcome::Session).without_message()),
        "/api/messages/42/body?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(missing_message.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(missing_message).await["message"],
        "message not found"
    );
}

#[tokio::test]
async fn without_api_key_returns_401() {
    for uri in [
        "/api/messages/42/body?mailbox=INBOX",
        "/api/messages/42/attachments?mailbox=INBOX",
        "/api/messages/42/attachments/0?mailbox=INBOX",
    ] {
        let app = app_with(FakeConnector::new(ConnectOutcome::Session));
        let response = get(app, uri, None).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "`{uri}` should be a 401 without a key"
        );
    }
}
