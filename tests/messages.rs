//! Integration tests for the message endpoints (`GET /api/messages` and
//! `GET /api/messages/{uid}`) using an in-memory fake connector, so no test ever
//! touches the network.
//!
//! The fake session **records** the [`SearchCriteria`] and [`FetchFormat`] it
//! receives, so the tests can assert that the HTTP query is translated into the
//! intended IMAP request.

use std::sync::atomic::{AtomicUsize, Ordering};
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
    Address, Capabilities, FetchFormat, FlagQuery, ImapConnector, ImapError, ImapSession,
    MailboxInfo, MailboxStatus, Message, MessageEnvelope, SearchCriteria, SearchDate,
    SendFuture as ImapSendFuture,
};
use apimail::smtp::{MailSender, OutgoingMessage, SendFuture as MailSendFuture};
use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-messages-key";
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

/// Outcome of a `select` call on a [`FakeSession`].
#[derive(Clone, Copy)]
enum SelectOutcome {
    /// Answer with a default status.
    Ok,
    /// Answer `NO`, as when the mailbox does not exist.
    NotFound,
}

/// A fake session that records the criteria and formats it receives.
struct FakeSession {
    /// Outcome of `select`.
    select: SelectOutcome,
    /// UIDs returned by `search`.
    uids: Vec<u32>,
    /// Messages returned by `fetch`.
    messages: Vec<Message>,
    /// Mailboxes received by `select`.
    select_log: Arc<Mutex<Vec<String>>>,
    /// Criteria received by `search`.
    search_log: Arc<Mutex<Vec<SearchCriteria>>>,
    /// Formats received by `fetch`.
    fetch_log: Arc<Mutex<Vec<FetchFormat>>>,
    /// UID sets received by `fetch`, in call order.
    fetch_uids: Arc<Mutex<Vec<Vec<u32>>>>,
}

impl ImapSession for FakeSession {
    fn noop(&mut self) -> ImapSendFuture<'_, Result<(), ImapError>> {
        Box::pin(async move { Ok(()) })
    }

    fn list_mailboxes(&mut self) -> ImapSendFuture<'_, Result<Vec<MailboxInfo>, ImapError>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn select(&mut self, mailbox: String) -> ImapSendFuture<'_, Result<MailboxStatus, ImapError>> {
        self.select_log
            .lock()
            .expect("select log poisoned")
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
        criteria: SearchCriteria,
    ) -> ImapSendFuture<'_, Result<Vec<u32>, ImapError>> {
        self.search_log
            .lock()
            .expect("search log poisoned")
            .push(criteria);
        let uids = self.uids.clone();
        Box::pin(async move { Ok(uids) })
    }

    fn fetch(
        &mut self,
        uids: Vec<u32>,
        format: FetchFormat,
    ) -> ImapSendFuture<'_, Result<Vec<Message>, ImapError>> {
        self.fetch_log
            .lock()
            .expect("fetch log poisoned")
            .push(format);
        self.fetch_uids
            .lock()
            .expect("fetch uids log poisoned")
            .push(uids.clone());
        // Only the requested messages are returned, in the scripted order, so a
        // wrong pagination window or a missing sort is observable end to end.
        let messages = self
            .messages
            .iter()
            .filter(|message| uids.contains(&message.uid))
            .cloned()
            .collect();
        Box::pin(async move { Ok(messages) })
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

/// Outcome of a `connect` call.
#[derive(Clone, Copy)]
enum ConnectOutcome {
    /// Hand back a session.
    Session,
    /// Fail as a server-unreachable error.
    Unavailable,
}

/// A fake [`ImapConnector`] with a scripted session and request logs.
#[derive(Clone)]
struct FakeConnector {
    /// Outcome of every `connect` call.
    outcome: ConnectOutcome,
    /// `select` outcome of the established session.
    select: SelectOutcome,
    /// UIDs the established session reports.
    uids: Vec<u32>,
    /// Messages the established session returns.
    messages: Vec<Message>,
    /// Mailboxes received by `select`.
    select_log: Arc<Mutex<Vec<String>>>,
    /// Criteria received by `search`.
    search_log: Arc<Mutex<Vec<SearchCriteria>>>,
    /// Formats received by `fetch`.
    fetch_log: Arc<Mutex<Vec<FetchFormat>>>,
    /// UID sets received by `fetch`, in call order.
    fetch_uids: Arc<Mutex<Vec<Vec<u32>>>>,
    /// Number of `connect` calls observed.
    connects: Arc<AtomicUsize>,
}

impl FakeConnector {
    /// A connector whose session succeeds at selecting and starts empty.
    fn new(outcome: ConnectOutcome) -> Self {
        Self {
            outcome,
            select: SelectOutcome::Ok,
            uids: Vec::new(),
            messages: Vec::new(),
            select_log: Arc::new(Mutex::new(Vec::new())),
            search_log: Arc::new(Mutex::new(Vec::new())),
            fetch_log: Arc::new(Mutex::new(Vec::new())),
            fetch_uids: Arc::new(Mutex::new(Vec::new())),
            connects: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Sets the `select` outcome.
    fn with_select(mut self, select: SelectOutcome) -> Self {
        self.select = select;
        self
    }

    /// Sets the UIDs the session reports.
    fn with_uids(mut self, uids: Vec<u32>) -> Self {
        self.uids = uids;
        self
    }

    /// Sets the messages the session returns.
    fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }
}

impl ImapConnector for FakeConnector {
    fn connect(&self) -> ImapSendFuture<'_, Result<Box<dyn ImapSession>, ImapError>> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let outcome = self.outcome;
        let select = self.select;
        let uids = self.uids.clone();
        let messages = self.messages.clone();
        let select_log = Arc::clone(&self.select_log);
        let search_log = Arc::clone(&self.search_log);
        let fetch_log = Arc::clone(&self.fetch_log);
        let fetch_uids = Arc::clone(&self.fetch_uids);
        Box::pin(async move {
            match outcome {
                ConnectOutcome::Session => Ok(Box::new(FakeSession {
                    select,
                    uids,
                    messages,
                    select_log,
                    search_log,
                    fetch_log,
                    fetch_uids,
                }) as Box<dyn ImapSession>),
                ConnectOutcome::Unavailable => Err(ImapError::Unavailable(
                    "simulated connect failure".to_string(),
                )),
            }
        })
    }
}

/// Builds the router with both services injected, without touching the network.
fn app_with(connector: FakeConnector) -> Router {
    build_router(
        AppState::with_services(&config(), Arc::new(NoopMailer), Arc::new(connector))
            .expect("building the app state must not touch the network"),
    )
}

/// A representative message with the given UID.
fn message(uid: u32) -> Message {
    Message {
        uid,
        seq: uid,
        flags: vec!["\\Seen".to_string()],
        size: Some(2048),
        internal_date: Some("2026-10-08T12:00:00+00:00".to_string()),
        envelope: Some(envelope()),
        headers: None,
        body: None,
    }
}

/// A representative envelope.
fn envelope() -> MessageEnvelope {
    MessageEnvelope {
        from: vec![Address {
            name: Some("Alice".to_string()),
            address: Some("alice@example.com".to_string()),
        }],
        to: vec![Address {
            name: None,
            address: Some("bob@example.com".to_string()),
        }],
        cc: Vec::new(),
        subject: Some("hi".to_string()),
        date: Some("2026-10-08T11:00:00+00:00".to_string()),
        message_id: Some("<id@example.com>".to_string()),
    }
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
async fn listing_returns_paginated_messages_and_translates_filters() {
    let connector = FakeConnector::new(ConnectOutcome::Session)
        .with_uids(vec![9, 7, 5, 3, 1])
        // Deliberately not newest-first: the manager must impose the order.
        .with_messages(vec![message(5), message(7)]);
    let search_log = Arc::clone(&connector.search_log);
    let select_log = Arc::clone(&connector.select_log);
    let fetch_uids = Arc::clone(&connector.fetch_uids);
    let app = app_with(connector);

    let response = get(
        app,
        "/api/messages?mailbox=INBOX&from=alice@example.com&since=2024-01-05&seen=false&limit=2&offset=1",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), Some("application/json"));

    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["total"], 5);
    assert_eq!(json["limit"], 2);
    assert_eq!(json["offset"], 1);

    let messages = json["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0]["uid"], 7,
        "the response must be newest-first regardless of the FETCH order"
    );
    assert_eq!(messages[1]["uid"], 5);
    assert_eq!(messages[0]["seq"], 7);
    assert_eq!(messages[0]["flags"][0], "\\Seen");
    assert_eq!(messages[0]["size"], 2048);
    assert_eq!(messages[0]["internal_date"], "2026-10-08T12:00:00+00:00");
    assert_eq!(messages[0]["envelope"]["from"][0]["name"], "Alice");
    assert_eq!(
        messages[0]["envelope"]["from"][0]["address"],
        "alice@example.com"
    );
    assert_eq!(
        messages[0]["envelope"]["to"][0]["name"],
        serde_json::Value::Null
    );
    assert!(
        messages[0]["envelope"]["cc"]
            .as_array()
            .expect("cc array")
            .is_empty()
    );

    let criteria = search_log.lock().expect("search log poisoned");
    assert_eq!(criteria.len(), 1);
    assert_eq!(criteria[0].from.as_deref(), Some("alice@example.com"));
    assert_eq!(criteria[0].since, SearchDate::parse("2024-01-05").ok());
    assert_eq!(criteria[0].seen, Some(false));
    let selected = select_log.lock().expect("select log poisoned");
    assert_eq!(selected.as_slice(), ["INBOX"]);
    assert_eq!(
        fetch_uids.lock().expect("fetch uids log poisoned").clone(),
        vec![vec![7, 5]],
        "the FETCH must request exactly the paginated window"
    );
}

#[tokio::test]
async fn listing_translates_every_filter() {
    let connector = FakeConnector::new(ConnectOutcome::Session).with_uids(vec![4]);
    let search_log = Arc::clone(&connector.search_log);
    let app = app_with(connector);

    let response = get(
        app,
        "/api/messages?mailbox=INBOX&to=bob&subject=hi&text=body&before=2024-12-31&unseen=true&flagged=true",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);

    let criteria = search_log.lock().expect("search log poisoned");
    assert_eq!(criteria.len(), 1);
    let criteria = &criteria[0];
    assert_eq!(criteria.to.as_deref(), Some("bob"));
    assert_eq!(criteria.subject.as_deref(), Some("hi"));
    assert_eq!(criteria.text.as_deref(), Some("body"));
    assert_eq!(criteria.before, SearchDate::parse("2024-12-31").ok());
    // `unseen=true` is the alias of `seen=false`.
    assert_eq!(criteria.seen, Some(false));
    assert_eq!(criteria.flagged, Some(true));
}

#[tokio::test]
async fn listing_uses_default_pagination() {
    let connector = FakeConnector::new(ConnectOutcome::Session).with_uids(vec![1]);
    let app = app_with(connector);

    let response = get(app, "/api/messages?mailbox=INBOX", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["limit"], 50);
    assert_eq!(json["offset"], 0);
    assert_eq!(json["total"], 1);
}

#[tokio::test]
async fn fetching_summary_returns_metadata_without_bodies() {
    let connector = FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42)]);
    let fetch_log = Arc::clone(&connector.fetch_log);
    let app = app_with(connector);

    let response = get(app, "/api/messages/42?mailbox=INBOX", Some(&bearer())).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(content_type(&response), Some("application/json"));

    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["uid"], 42);
    assert_eq!(json["seq"], 42);
    assert_eq!(json["format"], "summary");
    assert_eq!(json["size"], 2048);
    assert!(json.get("headers_base64").is_none(), "{json}");
    assert!(json.get("raw_base64").is_none(), "{json}");

    let formats = fetch_log.lock().expect("fetch log poisoned");
    assert_eq!(formats.as_slice(), [FetchFormat::Summary]);
}

#[tokio::test]
async fn fetching_headers_returns_base64_header_block() {
    let header_bytes = b"Subject: hi\r\nFrom: alice@example.com\r\n\r\n".to_vec();
    let mut with_headers = message(42);
    with_headers.headers = Some(header_bytes.clone());
    let connector = FakeConnector::new(ConnectOutcome::Session).with_messages(vec![with_headers]);
    let fetch_log = Arc::clone(&connector.fetch_log);
    let app = app_with(connector);

    let response = get(
        app,
        "/api/messages/42?mailbox=INBOX&format=headers",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["format"], "headers");
    let encoded = json["headers_base64"]
        .as_str()
        .expect("headers_base64 string");
    assert_eq!(
        STANDARD.decode(encoded).expect("valid base64"),
        header_bytes
    );
    assert!(json.get("raw_base64").is_none(), "{json}");

    let formats = fetch_log.lock().expect("fetch log poisoned");
    assert_eq!(formats.as_slice(), [FetchFormat::Headers]);
}

#[tokio::test]
async fn fetching_full_returns_base64_raw_message() {
    let raw_body = b"Subject: hi\r\n\r\nfull body".to_vec();
    let mut with_body = message(42);
    with_body.body = Some(raw_body.clone());
    let connector = FakeConnector::new(ConnectOutcome::Session).with_messages(vec![with_body]);
    let fetch_log = Arc::clone(&connector.fetch_log);
    let app = app_with(connector);

    let response = get(
        app,
        "/api/messages/42?mailbox=INBOX&format=full",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["format"], "full");
    let encoded = json["raw_base64"].as_str().expect("raw_base64 string");
    assert_eq!(STANDARD.decode(encoded).expect("valid base64"), raw_body);
    assert!(json.get("headers_base64").is_none(), "{json}");

    let formats = fetch_log.lock().expect("fetch log poisoned");
    assert_eq!(formats.as_slice(), [FetchFormat::Full]);
}

#[tokio::test]
async fn fetching_headers_or_full_without_body_omits_the_base64_field() {
    // The fake returns a message whose `headers`/`body` are `None`: the response
    // must be a `200` that simply omits the corresponding field.
    let headers = get(
        app_with(FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42)])),
        "/api/messages/42?mailbox=INBOX&format=headers",
        Some(&bearer()),
    )
    .await;
    assert_eq!(headers.status(), StatusCode::OK);
    let headers_json = body_json(headers).await;
    assert_eq!(headers_json["format"], "headers");
    assert!(
        headers_json.get("headers_base64").is_none(),
        "a missing header block must omit headers_base64: {headers_json}"
    );

    let full = get(
        app_with(FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42)])),
        "/api/messages/42?mailbox=INBOX&format=full",
        Some(&bearer()),
    )
    .await;
    assert_eq!(full.status(), StatusCode::OK);
    let full_json = body_json(full).await;
    assert_eq!(full_json["format"], "full");
    assert!(
        full_json.get("raw_base64").is_none(),
        "a missing raw message must omit raw_base64: {full_json}"
    );
}

#[tokio::test]
async fn control_characters_are_rejected_without_sending_a_search() {
    let connector = FakeConnector::new(ConnectOutcome::Session);
    let search_log = Arc::clone(&connector.search_log);
    let app = app_with(connector);

    let response = get(
        app,
        "/api/messages?mailbox=INBOX&subject=bad%0D%0Avalue",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"], "invalid_request");
    assert!(
        search_log.lock().expect("search log poisoned").is_empty(),
        "a rejected request must not send an IMAP SEARCH command"
    );
}

#[tokio::test]
async fn unknown_mailbox_returns_404() {
    let listing = get(
        app_with(FakeConnector::new(ConnectOutcome::Session).with_select(SelectOutcome::NotFound)),
        "/api/messages?mailbox=Missing",
        Some(&bearer()),
    )
    .await;
    assert_eq!(listing.status(), StatusCode::NOT_FOUND);
    assert_eq!(content_type(&listing), Some("application/json"));
    assert_eq!(body_json(listing).await["error"], "mailbox_not_found");

    let detail = get(
        app_with(FakeConnector::new(ConnectOutcome::Session).with_select(SelectOutcome::NotFound)),
        "/api/messages/42?mailbox=Missing",
        Some(&bearer()),
    )
    .await;
    assert_eq!(detail.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(detail).await["error"], "mailbox_not_found");
}

#[tokio::test]
async fn unknown_message_returns_404() {
    let response = get(
        app_with(FakeConnector::new(ConnectOutcome::Session)),
        "/api/messages/42?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(content_type(&response), Some("application/json"));
    assert_eq!(body_json(response).await["error"], "message_not_found");
}

#[tokio::test]
async fn invalid_listing_requests_return_400() {
    for uri in [
        "/api/messages",
        "/api/messages?mailbox=",
        "/api/messages?mailbox=%20%20",
        "/api/messages?mailbox=INBOX&since=not-a-date",
        "/api/messages?mailbox=INBOX&before=2024-13-01",
        "/api/messages?mailbox=INBOX&seen=maybe",
        "/api/messages?mailbox=INBOX&flagged=1",
        "/api/messages?mailbox=INBOX&limit=0",
        "/api/messages?mailbox=INBOX&limit=201",
        "/api/messages?mailbox=INBOX&limit=abc",
        "/api/messages?mailbox=INBOX&offset=-1",
        "/api/messages?mailbox=INBOX&offset=abc",
        "/api/messages?mailbox=INBOX&seen=true&unseen=true",
        "/api/messages?mailbox=INBOX&seen=false&unseen=false",
        "/api/messages?mailbox=INBOX&subject=bad%0D%0Avalue",
    ] {
        let response = get(
            app_with(FakeConnector::new(ConnectOutcome::Session)),
            uri,
            Some(&bearer()),
        )
        .await;
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
    }
}

#[tokio::test]
async fn invalid_retrieval_requests_return_400() {
    for uri in [
        "/api/messages/abc?mailbox=INBOX",
        "/api/messages/0?mailbox=INBOX",
        "/api/messages/42",
        "/api/messages/42?mailbox=",
        "/api/messages/42?mailbox=INBOX&format=xml",
    ] {
        let response = get(
            app_with(FakeConnector::new(ConnectOutcome::Session)),
            uri,
            Some(&bearer()),
        )
        .await;
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
    }
}

#[tokio::test]
async fn unreachable_server_returns_503_without_leaking_secrets() {
    let listing = get(
        app_with(FakeConnector::new(ConnectOutcome::Unavailable)),
        "/api/messages?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(listing.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_unavailable_without_secrets(body_json(listing).await);

    let detail = get(
        app_with(FakeConnector::new(ConnectOutcome::Unavailable)),
        "/api/messages/42?mailbox=INBOX",
        Some(&bearer()),
    )
    .await;
    assert_eq!(detail.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_unavailable_without_secrets(body_json(detail).await);
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
async fn without_api_key_returns_401() {
    let listing = get(
        app_with(FakeConnector::new(ConnectOutcome::Session)),
        "/api/messages?mailbox=INBOX",
        None,
    )
    .await;
    assert_eq!(listing.status(), StatusCode::UNAUTHORIZED);

    let detail = get(
        app_with(FakeConnector::new(ConnectOutcome::Session)),
        "/api/messages/42?mailbox=INBOX",
        None,
    )
    .await;
    assert_eq!(detail.status(), StatusCode::UNAUTHORIZED);
}
