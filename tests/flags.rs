//! Integration tests for the message flag, move, copy and delete endpoints
//! (`PATCH /api/messages/{uid}/flags`, `POST /api/messages/{uid}/move`,
//! `POST /api/messages/{uid}/copy` and `DELETE /api/messages/{uid}`) using an
//! in-memory fake connector, so no test ever touches the network.
//!
//! The fake session **records** the selected mailbox, the `STORE` query, and the
//! `COPY`/`MOVE`/`UID EXPUNGE` calls, and lets a test script its capabilities, so
//! the tests can assert that the HTTP request is translated into the intended
//! IMAP command sequence.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use apimail::imap::{
    Capabilities, FetchFormat, FlagQuery, ImapConnector, ImapError, ImapSession, MailboxInfo,
    MailboxStatus, Message, SearchCriteria, SendFuture as ImapSendFuture,
};
use apimail::smtp::{MailSender, OutgoingMessage, SendFuture as MailSendFuture};
use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-flags-key";
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

/// A fake session that records every flag command it receives.
struct FakeSession {
    /// Outcome of `select`.
    select: SelectOutcome,
    /// Capabilities reported by `capabilities`.
    capabilities: Capabilities,
    /// Messages returned by `fetch` (filtered by the requested UIDs).
    messages: Vec<Message>,
    /// Mailboxes received by `select`, in call order.
    selected: Arc<Mutex<Vec<String>>>,
    /// `STORE` queries received, in call order.
    stores: Arc<Mutex<Vec<String>>>,
    /// `COPY` calls received, as `(uid, destination)`.
    copies: Arc<Mutex<Vec<(u32, String)>>>,
    /// `MOVE` calls received, as `(uid, destination)`.
    moves: Arc<Mutex<Vec<(u32, String)>>>,
    /// `UID EXPUNGE` calls received.
    expunges: Arc<Mutex<Vec<u32>>>,
    /// Every mutating command, in call order, for order assertions.
    ops: Arc<Mutex<Vec<String>>>,
}

/// A representative mailbox status.
fn default_status() -> MailboxStatus {
    MailboxStatus {
        exists: 0,
        recent: 0,
        unseen: None,
        uid_validity: None,
        uid_next: None,
        flags: Vec::new(),
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
        let outcome = self.select;
        Box::pin(async move {
            match outcome {
                SelectOutcome::Ok => Ok(default_status()),
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
        _format: FetchFormat,
    ) -> ImapSendFuture<'_, Result<Vec<Message>, ImapError>> {
        let messages = self
            .messages
            .iter()
            .filter(|message| uids.contains(&message.uid))
            .cloned()
            .collect();
        Box::pin(async move { Ok(messages) })
    }

    fn store(&mut self, uid: u32, query: FlagQuery) -> ImapSendFuture<'_, Result<(), ImapError>> {
        let rendered = query.as_str().to_string();
        self.ops
            .lock()
            .expect("ops log poisoned")
            .push(format!("store {uid} {rendered}"));
        self.stores
            .lock()
            .expect("stores log poisoned")
            .push(rendered);
        Box::pin(async move { Ok(()) })
    }

    fn copy(&mut self, uid: u32, mailbox: String) -> ImapSendFuture<'_, Result<(), ImapError>> {
        self.ops
            .lock()
            .expect("ops log poisoned")
            .push(format!("copy {uid} {mailbox}"));
        self.copies
            .lock()
            .expect("copies log poisoned")
            .push((uid, mailbox));
        Box::pin(async move { Ok(()) })
    }

    fn move_message(
        &mut self,
        uid: u32,
        mailbox: String,
    ) -> ImapSendFuture<'_, Result<(), ImapError>> {
        self.ops
            .lock()
            .expect("ops log poisoned")
            .push(format!("move {uid} {mailbox}"));
        self.moves
            .lock()
            .expect("moves log poisoned")
            .push((uid, mailbox));
        Box::pin(async move { Ok(()) })
    }

    fn uid_expunge(&mut self, uid: u32) -> ImapSendFuture<'_, Result<(), ImapError>> {
        self.ops
            .lock()
            .expect("ops log poisoned")
            .push(format!("expunge {uid}"));
        self.expunges
            .lock()
            .expect("expunges log poisoned")
            .push(uid);
        Box::pin(async move { Ok(()) })
    }

    fn capabilities(&mut self) -> ImapSendFuture<'_, Result<Capabilities, ImapError>> {
        let capabilities = self.capabilities;
        Box::pin(async move { Ok(capabilities) })
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
    /// Capabilities of the established session.
    capabilities: Capabilities,
    /// Messages the established session returns.
    messages: Vec<Message>,
    /// Mailboxes received by `select`.
    selected: Arc<Mutex<Vec<String>>>,
    /// `STORE` queries received.
    stores: Arc<Mutex<Vec<String>>>,
    /// `COPY` calls received.
    copies: Arc<Mutex<Vec<(u32, String)>>>,
    /// `MOVE` calls received.
    moves: Arc<Mutex<Vec<(u32, String)>>>,
    /// `UID EXPUNGE` calls received.
    expunges: Arc<Mutex<Vec<u32>>>,
    /// Every mutating command, in call order.
    ops: Arc<Mutex<Vec<String>>>,
    /// Number of `connect` calls observed.
    connects: Arc<AtomicUsize>,
}

impl FakeConnector {
    /// A connector whose session succeeds at selecting and starts empty.
    fn new(outcome: ConnectOutcome) -> Self {
        Self {
            outcome,
            select: SelectOutcome::Ok,
            capabilities: Capabilities::default(),
            messages: Vec::new(),
            selected: Arc::new(Mutex::new(Vec::new())),
            stores: Arc::new(Mutex::new(Vec::new())),
            copies: Arc::new(Mutex::new(Vec::new())),
            moves: Arc::new(Mutex::new(Vec::new())),
            expunges: Arc::new(Mutex::new(Vec::new())),
            ops: Arc::new(Mutex::new(Vec::new())),
            connects: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Sets the `select` outcome.
    fn with_select(mut self, select: SelectOutcome) -> Self {
        self.select = select;
        self
    }

    /// Sets the session capabilities.
    fn with_capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = capabilities;
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
        let capabilities = self.capabilities;
        let messages = self.messages.clone();
        let selected = Arc::clone(&self.selected);
        let stores = Arc::clone(&self.stores);
        let copies = Arc::clone(&self.copies);
        let moves = Arc::clone(&self.moves);
        let expunges = Arc::clone(&self.expunges);
        let ops = Arc::clone(&self.ops);
        Box::pin(async move {
            match outcome {
                ConnectOutcome::Session => Ok(Box::new(FakeSession {
                    select,
                    capabilities,
                    messages,
                    selected,
                    stores,
                    copies,
                    moves,
                    expunges,
                    ops,
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

/// A representative message with the given UID and flags.
fn message(uid: u32, flags: &[&str]) -> Message {
    Message {
        uid,
        seq: uid,
        flags: flags.iter().map(|flag| flag.to_string()).collect(),
        size: Some(1024),
        internal_date: None,
        envelope: None,
        headers: None,
        body: None,
    }
}

/// The bearer header value for the test API key.
fn bearer() -> String {
    format!("Bearer {API_KEY}")
}

/// Sends a request with an optional JSON body and `Authorization`.
async fn call(
    app: Router,
    method: Method,
    uri: &str,
    body: Option<String>,
    authorization: Option<&str>,
) -> Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    app.oneshot(
        builder
            .body(Body::from(body.unwrap_or_default()))
            .expect("valid request"),
    )
    .await
    .expect("router should respond")
}

/// Reads a response body as JSON.
async fn body_json(response: Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body should be readable")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("valid JSON body")
}

/// Asserts a JSON body carries exactly `expected` as its field names (sorted).
///
/// The shape of a body is part of the contract, so the migration of an operation
/// into the service layer may not add, rename or drop a field.
fn assert_body_keys(json: &Value, expected: &[&str], what: &str) {
    let object = json.as_object().expect("the body must be a JSON object");
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, expected, "unexpected fields in {what}: {json}");
}

// ---------------------------------------------------------------------------
// PATCH /api/messages/{uid}/flags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adding_flags_sends_a_store_and_returns_the_resulting_flags() {
    let connector = FakeConnector::new(ConnectOutcome::Session)
        .with_messages(vec![message(42, &["\\Seen", "\\Flagged"])]);
    let stores = Arc::clone(&connector.stores);
    let selected = Arc::clone(&connector.selected);
    let app = app_with(connector);

    let response = call(
        app,
        Method::PATCH,
        "/api/messages/42/flags?mailbox=INBOX",
        Some(json!({"add": ["\\Seen", "\\Flagged"]}).to_string()),
        Some(&bearer()),
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
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["uid"], 42);
    assert_eq!(json["flags"][0], "\\Seen");
    assert_eq!(json["flags"][1], "\\Flagged");
    assert_body_keys(&json, &["flags", "mailbox", "uid"], "the flags body");

    assert_eq!(
        selected.lock().expect("selected log poisoned").as_slice(),
        ["INBOX"]
    );
    assert_eq!(
        stores.lock().expect("stores log poisoned").as_slice(),
        ["+FLAGS.SILENT (\\Seen \\Flagged)"],
        "the STORE query must carry only fixed items and allowlisted flags"
    );
}

#[tokio::test]
async fn removing_flags_sends_a_negative_store() {
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42, &[])]);
    let stores = Arc::clone(&connector.stores);
    let app = app_with(connector);

    let response = call(
        app,
        Method::PATCH,
        "/api/messages/42/flags?mailbox=INBOX",
        Some(json!({"remove": ["\\Deleted"]}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(json["flags"].as_array().expect("flags array").is_empty());
    assert_eq!(
        stores.lock().expect("stores log poisoned").as_slice(),
        ["-FLAGS.SILENT (\\Deleted)"]
    );
}

#[tokio::test]
async fn adding_and_removing_flags_are_combined_in_one_store() {
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42, &["\\Seen"])]);
    let stores = Arc::clone(&connector.stores);
    let app = app_with(connector);

    let response = call(
        app,
        Method::PATCH,
        "/api/messages/42/flags?mailbox=INBOX",
        Some(json!({"add": ["\\Seen"], "remove": ["\\Deleted"]}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        stores.lock().expect("stores log poisoned").as_slice(),
        ["+FLAGS.SILENT (\\Seen) -FLAGS.SILENT (\\Deleted)"]
    );
}

#[tokio::test]
async fn flags_are_canonicalised_case_insensitively_and_deduplicated() {
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42, &["\\Seen"])]);
    let stores = Arc::clone(&connector.stores);
    let app = app_with(connector);

    let response = call(
        app,
        Method::PATCH,
        "/api/messages/42/flags?mailbox=INBOX",
        Some(json!({"add": ["\\seen", "\\FLAGGED", "\\Seen"]}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        stores.lock().expect("stores log poisoned").as_slice(),
        ["+FLAGS.SILENT (\\Seen \\Flagged)"],
        "flags must be canonicalised and deduplicated, preserving order"
    );
}

#[tokio::test]
async fn a_flag_outside_the_allowlist_is_400_without_sending_a_command() {
    for flag in ["\\Archived", "Seen", "\\", "\\Seen \\Deleted"] {
        let connector = FakeConnector::new(ConnectOutcome::Session);
        let connects = Arc::clone(&connector.connects);
        let stores = Arc::clone(&connector.stores);
        let app = app_with(connector);

        let response = call(
            app,
            Method::PATCH,
            "/api/messages/42/flags?mailbox=INBOX",
            Some(json!({"add": [flag]}).to_string()),
            Some(&bearer()),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "flag `{flag}` should be rejected"
        );
        assert_eq!(body_json(response).await["error"], "invalid_request");
        assert_eq!(
            connects.load(Ordering::SeqCst),
            0,
            "no connection for `{flag}`"
        );
        assert!(stores.lock().expect("stores log poisoned").is_empty());
    }
}

#[tokio::test]
async fn an_empty_or_contradictory_flag_request_is_400_without_sending_a_command() {
    for body in [
        json!({}),
        json!({"add": [], "remove": []}),
        json!({"add": ["\\Seen"], "remove": ["\\Seen"]}),
        json!({"add": ["\\seen"], "remove": ["\\SEEN"]}),
    ] {
        let connector = FakeConnector::new(ConnectOutcome::Session);
        let connects = Arc::clone(&connector.connects);
        let app = app_with(connector);

        let response = call(
            app,
            Method::PATCH,
            "/api/messages/42/flags?mailbox=INBOX",
            Some(body.to_string()),
            Some(&bearer()),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "body `{body}` should be rejected"
        );
        assert_eq!(body_json(response).await["error"], "invalid_request");
        assert_eq!(
            connects.load(Ordering::SeqCst),
            0,
            "no connection for `{body}`"
        );
    }
}

#[tokio::test]
async fn a_flag_removed_and_added_in_the_same_disjoint_case_is_accepted() {
    // `\seen` in `add` and `\Flagged` in `remove` are disjoint after
    // canonicalisation.
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42, &["\\Seen"])]);
    let app = app_with(connector);

    let response = call(
        app,
        Method::PATCH,
        "/api/messages/42/flags?mailbox=INBOX",
        Some(json!({"add": ["\\seen"], "remove": ["\\Flagged"]}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn updating_flags_of_an_unknown_message_is_404() {
    let app = app_with(FakeConnector::new(ConnectOutcome::Session));
    let response = call(
        app,
        Method::PATCH,
        "/api/messages/42/flags?mailbox=INBOX",
        Some(json!({"add": ["\\Seen"]}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"], "message_not_found");
}

#[tokio::test]
async fn updating_flags_in_an_unknown_mailbox_is_404() {
    let app =
        app_with(FakeConnector::new(ConnectOutcome::Session).with_select(SelectOutcome::NotFound));
    let response = call(
        app,
        Method::PATCH,
        "/api/messages/42/flags?mailbox=Missing",
        Some(json!({"add": ["\\Seen"]}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"], "mailbox_not_found");
}

#[tokio::test]
async fn a_flag_with_control_characters_is_400_without_sending_a_command() {
    let connector = FakeConnector::new(ConnectOutcome::Session);
    let connects = Arc::clone(&connector.connects);
    let stores = Arc::clone(&connector.stores);
    let app = app_with(connector);

    let response = call(
        app,
        Method::PATCH,
        "/api/messages/42/flags?mailbox=INBOX",
        Some(json!({"add": ["\\Seen\r\nX"]}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"], "invalid_request");
    assert_eq!(
        connects.load(Ordering::SeqCst),
        0,
        "a rejected flag must not open a connection"
    );
    assert!(stores.lock().expect("stores log poisoned").is_empty());
}

#[tokio::test]
async fn flag_operations_reuse_the_live_session() {
    let connector = FakeConnector::new(ConnectOutcome::Session)
        .with_messages(vec![message(42, &["\\Seen"]), message(43, &["\\Seen"])]);
    let connects = Arc::clone(&connector.connects);
    let app = app_with(connector);

    for uid in [42, 43] {
        let response = call(
            app.clone(),
            Method::PATCH,
            &format!("/api/messages/{uid}/flags?mailbox=INBOX"),
            Some(json!({"add": ["\\Seen"]}).to_string()),
            Some(&bearer()),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "uid {uid}");
    }

    assert_eq!(
        connects.load(Ordering::SeqCst),
        1,
        "consecutive flag operations must reuse the single live session"
    );
}

// ---------------------------------------------------------------------------
// POST /api/messages/{uid}/move
// ---------------------------------------------------------------------------

#[tokio::test]
async fn moving_with_move_support_sends_uid_move() {
    let connector = FakeConnector::new(ConnectOutcome::Session)
        .with_capabilities(Capabilities {
            has_move: true,
            has_uidplus: false,
        })
        .with_messages(vec![message(42, &["\\Seen"])]);
    let ops = Arc::clone(&connector.ops);
    let app = app_with(connector);

    let response = call(
        app,
        Method::POST,
        "/api/messages/42/move?mailbox=INBOX",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["uid"], 42);
    assert_eq!(json["to"], "Archive");
    assert_eq!(json["status"], "moved");
    assert_body_keys(&json, &["mailbox", "status", "to", "uid"], "the move body");
    assert_eq!(
        ops.lock().expect("ops log poisoned").as_slice(),
        ["move 42 Archive"]
    );
}

#[tokio::test]
async fn moving_without_move_but_with_uidplus_emulates_copy_store_expunge_in_order() {
    let connector = FakeConnector::new(ConnectOutcome::Session)
        .with_capabilities(Capabilities {
            has_move: false,
            has_uidplus: true,
        })
        .with_messages(vec![message(42, &["\\Seen"])]);
    let ops = Arc::clone(&connector.ops);
    let moves = Arc::clone(&connector.moves);
    let app = app_with(connector);

    let response = call(
        app,
        Method::POST,
        "/api/messages/42/move?mailbox=INBOX",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["status"], "moved");
    assert_eq!(
        ops.lock().expect("ops log poisoned").as_slice(),
        [
            "copy 42 Archive",
            "store 42 +FLAGS.SILENT (\\Deleted)",
            "expunge 42"
        ],
        "the emulation must copy, then mark, then expunge"
    );
    assert!(
        moves.lock().expect("moves log poisoned").is_empty(),
        "no UID MOVE must be sent when MOVE is not announced"
    );
}

#[tokio::test]
async fn moving_without_any_capability_is_501_without_sending_a_command() {
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42, &["\\Seen"])]);
    let copies = Arc::clone(&connector.copies);
    let stores = Arc::clone(&connector.stores);
    let moves = Arc::clone(&connector.moves);
    let expunges = Arc::clone(&connector.expunges);
    let app = app_with(connector);

    let response = call(
        app,
        Method::POST,
        "/api/messages/42/move?mailbox=INBOX",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        body_json(response).await["error"],
        "capability_not_supported"
    );
    assert!(
        copies.lock().expect("copies log poisoned").is_empty(),
        "no COPY command must be sent"
    );
    assert!(
        stores.lock().expect("stores log poisoned").is_empty(),
        "no STORE command must be sent"
    );
    assert!(
        moves.lock().expect("moves log poisoned").is_empty(),
        "no UID MOVE command must be sent"
    );
    assert!(expunges.lock().expect("expunges log poisoned").is_empty());
}

#[tokio::test]
async fn moving_an_unknown_message_is_404() {
    let connector = FakeConnector::new(ConnectOutcome::Session).with_capabilities(Capabilities {
        has_move: true,
        has_uidplus: false,
    });
    let moves = Arc::clone(&connector.moves);
    let app = app_with(connector);

    let response = call(
        app,
        Method::POST,
        "/api/messages/42/move?mailbox=INBOX",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"], "message_not_found");
    assert!(moves.lock().expect("moves log poisoned").is_empty());
}

#[tokio::test]
async fn moving_from_an_unknown_mailbox_is_404() {
    let app = app_with(
        FakeConnector::new(ConnectOutcome::Session)
            .with_select(SelectOutcome::NotFound)
            .with_capabilities(Capabilities {
                has_move: true,
                has_uidplus: false,
            }),
    );
    let response = call(
        app,
        Method::POST,
        "/api/messages/42/move?mailbox=Missing",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"], "mailbox_not_found");
}

// ---------------------------------------------------------------------------
// POST /api/messages/{uid}/copy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn copying_sends_uid_copy() {
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42, &["\\Seen"])]);
    let copies = Arc::clone(&connector.copies);
    let ops = Arc::clone(&connector.ops);
    let stores = Arc::clone(&connector.stores);
    let moves = Arc::clone(&connector.moves);
    let expunges = Arc::clone(&connector.expunges);
    let app = app_with(connector);

    let response = call(
        app,
        Method::POST,
        "/api/messages/42/copy?mailbox=INBOX",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["uid"], 42);
    assert_eq!(json["to"], "Archive");
    assert_eq!(json["status"], "copied");
    assert_body_keys(&json, &["mailbox", "status", "to", "uid"], "the copy body");
    assert_eq!(
        copies.lock().expect("copies log poisoned").as_slice(),
        [(42, "Archive".to_string())]
    );
    assert_eq!(
        ops.lock().expect("ops log poisoned").as_slice(),
        ["copy 42 Archive"],
        "a copy is a single UID COPY and nothing else"
    );
    assert!(
        moves.lock().expect("moves log poisoned").is_empty(),
        "no UID MOVE must be sent"
    );
    assert!(
        stores.lock().expect("stores log poisoned").is_empty(),
        "no STORE command must be sent"
    );
    assert!(
        expunges.lock().expect("expunges log poisoned").is_empty(),
        "no EXPUNGE (global or targeted) must be sent"
    );
}

#[tokio::test]
async fn copying_an_unknown_message_is_404() {
    let app = app_with(FakeConnector::new(ConnectOutcome::Session));
    let response = call(
        app,
        Method::POST,
        "/api/messages/42/copy?mailbox=INBOX",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"], "message_not_found");
}

#[tokio::test]
async fn copying_from_an_unknown_mailbox_is_404() {
    let app =
        app_with(FakeConnector::new(ConnectOutcome::Session).with_select(SelectOutcome::NotFound));
    let response = call(
        app,
        Method::POST,
        "/api/messages/42/copy?mailbox=Missing",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"], "mailbox_not_found");
}

// ---------------------------------------------------------------------------
// DELETE /api/messages/{uid}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_with_uidplus_marks_deleted_then_expunges() {
    let connector = FakeConnector::new(ConnectOutcome::Session)
        .with_capabilities(Capabilities {
            has_move: false,
            has_uidplus: true,
        })
        .with_messages(vec![message(42, &["\\Seen"])]);
    let ops = Arc::clone(&connector.ops);
    let app = app_with(connector);

    let response = call(
        app,
        Method::DELETE,
        "/api/messages/42?mailbox=INBOX",
        None,
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["mailbox"], "INBOX");
    assert_eq!(json["uid"], 42);
    assert_eq!(json["status"], "deleted");
    assert_eq!(
        ops.lock().expect("ops log poisoned").as_slice(),
        ["store 42 +FLAGS.SILENT (\\Deleted)", "expunge 42"]
    );
}

#[tokio::test]
async fn deleting_without_uidplus_is_501_without_sending_an_expunge() {
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42, &["\\Seen"])]);
    let ops = Arc::clone(&connector.ops);
    let stores = Arc::clone(&connector.stores);
    let expunges = Arc::clone(&connector.expunges);
    let app = app_with(connector);

    let response = call(
        app,
        Method::DELETE,
        "/api/messages/42?mailbox=INBOX",
        None,
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    assert_eq!(
        body_json(response).await["error"],
        "capability_not_supported"
    );
    assert!(
        expunges.lock().expect("expunges log poisoned").is_empty(),
        "no EXPUNGE (global or targeted) must be sent"
    );
    assert!(stores.lock().expect("stores log poisoned").is_empty());
    assert!(
        ops.lock().expect("ops log poisoned").is_empty(),
        "a delete that purges the message must not send a single command"
    );
}

#[tokio::test]
async fn deleting_an_unknown_message_is_404() {
    let connector = FakeConnector::new(ConnectOutcome::Session).with_capabilities(Capabilities {
        has_move: false,
        has_uidplus: true,
    });
    let stores = Arc::clone(&connector.stores);
    let app = app_with(connector);

    let response = call(
        app,
        Method::DELETE,
        "/api/messages/42?mailbox=INBOX",
        None,
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"], "message_not_found");
    assert!(stores.lock().expect("stores log poisoned").is_empty());
}

#[tokio::test]
async fn deleting_from_an_unknown_mailbox_is_404() {
    let app = app_with(
        FakeConnector::new(ConnectOutcome::Session)
            .with_select(SelectOutcome::NotFound)
            .with_capabilities(Capabilities {
                has_move: false,
                has_uidplus: true,
            }),
    );
    let response = call(
        app,
        Method::DELETE,
        "/api/messages/42?mailbox=Missing",
        None,
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(response).await["error"], "mailbox_not_found");
}

// ---------------------------------------------------------------------------
// Request validation (shared envelope, no command sent)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_requests_return_400_without_connecting() {
    // (method, uri, body)
    let cases: &[(Method, &str, Option<Value>)] = &[
        // uid is not a positive numeric identifier
        (
            Method::PATCH,
            "/api/messages/abc/flags?mailbox=INBOX",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::PATCH,
            "/api/messages/0/flags?mailbox=INBOX",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::POST,
            "/api/messages/abc/move?mailbox=INBOX",
            Some(json!({"to": "Archive"})),
        ),
        (
            Method::POST,
            "/api/messages/0/copy?mailbox=INBOX",
            Some(json!({"to": "Archive"})),
        ),
        (Method::DELETE, "/api/messages/abc?mailbox=INBOX", None),
        (Method::DELETE, "/api/messages/0?mailbox=INBOX", None),
        // mailbox missing, blank or with control characters
        (
            Method::PATCH,
            "/api/messages/42/flags",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::PATCH,
            "/api/messages/42/flags?mailbox=",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::PATCH,
            "/api/messages/42/flags?mailbox=%20%20",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::PATCH,
            "/api/messages/42/flags?mailbox=IN%0DBOX",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::POST,
            "/api/messages/42/move",
            Some(json!({"to": "Archive"})),
        ),
        (Method::DELETE, "/api/messages/42", None),
        (Method::DELETE, "/api/messages/42?mailbox=", None),
        // `to` missing, blank or with control characters
        (
            Method::POST,
            "/api/messages/42/move?mailbox=INBOX",
            Some(json!({})),
        ),
        (
            Method::POST,
            "/api/messages/42/move?mailbox=INBOX",
            Some(json!({"to": ""})),
        ),
        (
            Method::POST,
            "/api/messages/42/move?mailbox=INBOX",
            Some(json!({"to": "   "})),
        ),
        (
            Method::POST,
            "/api/messages/42/move?mailbox=INBOX",
            Some(json!({"to": "Arch\r\nive"})),
        ),
        (
            Method::POST,
            "/api/messages/42/copy?mailbox=INBOX",
            Some(json!({"to": "Arch\0ive"})),
        ),
    ];

    for (method, uri, body) in cases {
        let connector = FakeConnector::new(ConnectOutcome::Session);
        let connects = Arc::clone(&connector.connects);
        let app = app_with(connector);

        let response = call(
            app,
            method.clone(),
            uri,
            body.as_ref().map(Value::to_string),
            Some(&bearer()),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "`{method} {uri}` should be a 400"
        );
        assert_eq!(
            body_json(response).await["error"],
            "invalid_request",
            "`{method} {uri}` should carry invalid_request"
        );
        assert_eq!(
            connects.load(Ordering::SeqCst),
            0,
            "`{method} {uri}` must not open a connection"
        );
    }
}

#[tokio::test]
async fn a_malformed_json_body_is_400_without_connecting() {
    for (method, uri) in [
        (Method::PATCH, "/api/messages/42/flags?mailbox=INBOX"),
        (Method::POST, "/api/messages/42/move?mailbox=INBOX"),
        (Method::POST, "/api/messages/42/copy?mailbox=INBOX"),
    ] {
        let connector = FakeConnector::new(ConnectOutcome::Session);
        let connects = Arc::clone(&connector.connects);
        let app = app_with(connector);

        let response = call(
            app,
            method.clone(),
            uri,
            Some("{not json".to_string()),
            Some(&bearer()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{method} {uri}");
        assert_eq!(body_json(response).await["error"], "invalid_request");
        assert_eq!(connects.load(Ordering::SeqCst), 0);
    }
}

// ---------------------------------------------------------------------------
// Error bodies: the exact `error`/`message` pair is part of the contract
// ---------------------------------------------------------------------------

/// Pins the exact 404 of a missing **message** on all three routes: the code and
/// the message are both published, so the migration into the service layer must
/// not drift either of them.
#[tokio::test]
async fn a_missing_message_is_404_with_the_documented_message() {
    for (method, uri, body) in [
        (
            Method::PATCH,
            "/api/messages/42/flags?mailbox=INBOX",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::POST,
            "/api/messages/42/move?mailbox=INBOX",
            Some(json!({"to": "Archive"})),
        ),
        (
            Method::POST,
            "/api/messages/42/copy?mailbox=INBOX",
            Some(json!({"to": "Archive"})),
        ),
    ] {
        let connector =
            FakeConnector::new(ConnectOutcome::Session).with_capabilities(Capabilities {
                has_move: true,
                has_uidplus: false,
            });
        let app = app_with(connector);
        let response = call(
            app,
            method.clone(),
            uri,
            body.map(|value| value.to_string()),
            Some(&bearer()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {uri}");
        let json = body_json(response).await;
        assert_eq!(json["error"], "message_not_found", "{method} {uri}");
        assert_eq!(json["message"], "message not found", "{method} {uri}");
        assert_body_keys(&json, &["error", "message"], "the message 404 body");
    }
}

/// Pins the exact 404 of a missing **mailbox** on all three routes.
#[tokio::test]
async fn a_missing_mailbox_is_404_with_the_documented_message() {
    for (method, uri, body) in [
        (
            Method::PATCH,
            "/api/messages/42/flags?mailbox=Missing",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::POST,
            "/api/messages/42/move?mailbox=Missing",
            Some(json!({"to": "Archive"})),
        ),
        (
            Method::POST,
            "/api/messages/42/copy?mailbox=Missing",
            Some(json!({"to": "Archive"})),
        ),
    ] {
        let app = app_with(
            FakeConnector::new(ConnectOutcome::Session)
                .with_select(SelectOutcome::NotFound)
                .with_capabilities(Capabilities {
                    has_move: true,
                    has_uidplus: true,
                }),
        );
        let response = call(
            app,
            method.clone(),
            uri,
            body.map(|value| value.to_string()),
            Some(&bearer()),
        )
        .await;

        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {uri}");
        let json = body_json(response).await;
        assert_eq!(json["error"], "mailbox_not_found", "{method} {uri}");
        assert_eq!(json["message"], "mailbox not found", "{method} {uri}");
    }
}

/// Pins the exact 501 of a server that announces neither `MOVE` nor `UIDPLUS`:
/// the code, the message and the invariant that no command is sent.
#[tokio::test]
async fn a_missing_capability_is_501_with_the_documented_message() {
    let connector =
        FakeConnector::new(ConnectOutcome::Session).with_messages(vec![message(42, &["\\Seen"])]);
    let ops = Arc::clone(&connector.ops);
    let app = app_with(connector);

    let response = call(
        app,
        Method::POST,
        "/api/messages/42/move?mailbox=INBOX",
        Some(json!({"to": "Archive"}).to_string()),
        Some(&bearer()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let json = body_json(response).await;
    assert_eq!(json["error"], "capability_not_supported");
    assert_eq!(
        json["message"], "the server does not support this operation",
        "the 501 message is the fixed one of the missing extension"
    );
    assert_body_keys(&json, &["error", "message"], "the 501 body");
    assert!(
        ops.lock().expect("ops log poisoned").is_empty(),
        "a 501 must not send a single command"
    );
}

// ---------------------------------------------------------------------------
// Unavailability and authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unreachable_server_is_503_for_every_operation() {
    for (method, uri, body) in [
        (
            Method::PATCH,
            "/api/messages/42/flags?mailbox=INBOX",
            Some(json!({"add": ["\\Seen"]})),
        ),
        (
            Method::POST,
            "/api/messages/42/move?mailbox=INBOX",
            Some(json!({"to": "Archive"})),
        ),
        (
            Method::POST,
            "/api/messages/42/copy?mailbox=INBOX",
            Some(json!({"to": "Archive"})),
        ),
        (Method::DELETE, "/api/messages/42?mailbox=INBOX", None),
    ] {
        let app = app_with(FakeConnector::new(ConnectOutcome::Unavailable));
        let response = call(
            app,
            method.clone(),
            uri,
            body.map(|value| value.to_string()),
            Some(&bearer()),
        )
        .await;

        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {uri}"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"], "imap_unavailable", "{method} {uri}");
        assert_eq!(
            json["message"], "could not connect to the IMAP server",
            "the 503 carries the stable, credential-free family message"
        );
        assert_body_keys(&json, &["error", "message"], "the 503 body");
    }
}

#[tokio::test]
async fn without_an_api_key_every_operation_is_401() {
    for (method, uri) in [
        (Method::PATCH, "/api/messages/42/flags?mailbox=INBOX"),
        (Method::POST, "/api/messages/42/move?mailbox=INBOX"),
        (Method::POST, "/api/messages/42/copy?mailbox=INBOX"),
        (Method::DELETE, "/api/messages/42?mailbox=INBOX"),
    ] {
        let app = app_with(FakeConnector::new(ConnectOutcome::Session));
        let response = call(app, method.clone(), uri, None, None).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
}

// ---------------------------------------------------------------------------
// MCP write tools (§7)
// ---------------------------------------------------------------------------

/// The MCP **write** tools, driven in process against the very same
/// [`FakeConnector`] the HTTP tests above use.
///
/// Reusing the double is the point: the `stores`/`copies`/`moves`/`expunges`
/// logs it already records are what proves a tool reaches the *same* IMAP
/// command sequence as its route, and — for `delete_message` — that the purge
/// really is a purge: a `\Deleted` mark followed by a targeted `EXPUNGE`, with no
/// `COPY` or `MOVE` placing a copy in a trash mailbox.
///
/// Each call runs over the real transport (`rmcp` over an in-memory pipe), not
/// against the handler directly, so the tool router, the derived argument schema
/// and the rendering of a failure are all exercised. The module is gated on the
/// `mcp` feature, so the default test run compiles none of it.
#[cfg(feature = "mcp")]
mod mcp_write {
    use std::time::{Duration, Instant};

    use apimail::ApimailMcp;
    use rmcp::ServiceExt;
    use tokio::io::{
        AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt,
        BufReader as AsyncBufReader, Lines,
    };

    use super::*;

    /// How long a test waits for a reply before failing, so a wedged transport
    /// fails the test instead of hanging the suite.
    const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

    /// The protocol revision the test client asks for, as in
    /// `tests/mcp_server.rs`.
    const PROTOCOL_VERSION: &str = "2026-07-28";

    /// A [`MailSender`] that counts the messages it accepts and never fails.
    struct CountingMailer {
        /// Number of messages handed to SMTP.
        sends: Arc<AtomicUsize>,
    }

    impl MailSender for CountingMailer {
        fn send(&self, _message: OutgoingMessage) -> MailSendFuture<'_> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(()) })
        }
    }

    /// Builds the application state with both services injected, so nothing in
    /// this module touches the network.
    fn state_with(mailer: Arc<dyn MailSender>, connector: FakeConnector) -> AppState {
        AppState::with_services(&config(), mailer, Arc::new(connector))
            .expect("building the app state must not touch the network")
    }

    /// Writes one newline-delimited JSON-RPC message.
    async fn write<W>(writer: &mut W, message: &Value)
    where
        W: AsyncWrite + Unpin,
    {
        let mut line = message.to_string();
        line.push('\n');
        writer
            .write_all(line.as_bytes())
            .await
            .expect("the request should be written to the server");
        writer
            .flush()
            .await
            .expect("the request should reach the server");
    }

    /// Reads messages until the reply to `id` arrives, skipping notifications.
    async fn reply<R>(lines: &mut Lines<R>, id: i64) -> Value
    where
        R: AsyncRead + AsyncBufRead + Unpin,
    {
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = tokio::time::timeout(remaining, lines.next_line())
                .await
                .expect("timed out waiting for a reply from the MCP server")
                .expect("failed to read from the MCP server")
                .expect("the MCP server closed the stream before answering");
            if line.trim().is_empty() {
                continue;
            }
            let value: Value =
                serde_json::from_str(&line).expect("the stream should carry JSON-RPC");
            if value["id"] == json!(id) {
                return value;
            }
        }
    }

    /// Drives one `tools/call` over a fresh in-process MCP session and returns
    /// the tool result (`isError`, `content`, `structuredContent`).
    async fn call_tool(state: &AppState, name: &str, arguments: Value) -> Value {
        let (server_end, client_end) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server_end);
        let server = tokio::spawn({
            let state = state.clone();
            async move {
                // The `RunningService` must be held for the whole session: dropping
                // it cancels the server, which would close the pipe mid-flight.
                if let Ok(running) = ApimailMcp::new(state)
                    .serve((server_read, server_write))
                    .await
                {
                    let _ = running.waiting().await;
                }
            }
        });

        let (client_read, mut client_write) = tokio::io::split(client_end);
        let mut lines = AsyncBufReader::new(client_read).lines();

        // The lifecycle a real client drives: initialize, acknowledge, call.
        write(
            &mut client_write,
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": { "name": "flags-tests", "version": "0.1.0" },
                },
            }),
        )
        .await;
        let _ = reply(&mut lines, 1).await;
        write(
            &mut client_write,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;

        write(
            &mut client_write,
            &json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            }),
        )
        .await;
        let answer = reply(&mut lines, 2).await;

        drop(client_write);
        drop(lines);
        let _ = tokio::time::timeout(REPLY_TIMEOUT, server).await;

        answer["result"].clone()
    }

    /// Asserts `result` is a tool error and returns the `"{kind}: {message}"` it
    /// carries, which must be the stable code of the operation.
    fn tool_error_text(result: &Value) -> String {
        assert_eq!(
            result["isError"],
            json!(true),
            "expected a tool error, got: {result}"
        );
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("a tool error should carry text: {result}"))
            .to_string()
    }

    /// Asserts `result` is a success and returns its structured content.
    fn structured(result: &Value) -> &Value {
        assert_ne!(
            result["isError"],
            json!(true),
            "the tool reported a failure: {result}"
        );
        result.get("structuredContent").unwrap_or_else(|| {
            panic!("a successful call should carry structured content: {result}")
        })
    }

    #[tokio::test]
    async fn mcp_update_flags_sends_only_the_named_flag() {
        let connector = FakeConnector::new(ConnectOutcome::Session)
            .with_messages(vec![message(42, &["\\Seen"])]);
        let stores = Arc::clone(&connector.stores);
        let state = state_with(Arc::new(NoopMailer), connector);

        let result = call_tool(
            &state,
            "update_flags",
            json!({ "mailbox": "INBOX", "uid": 42, "add": ["\\Flagged"] }),
        )
        .await;

        let content = structured(&result);
        assert_eq!(content["mailbox"], "INBOX");
        assert_eq!(content["uid"], json!(42));
        assert_eq!(content["flags"], json!(["\\Seen"]));
        assert_eq!(
            stores.lock().expect("stores log poisoned").as_slice(),
            ["+FLAGS.SILENT (\\Flagged)"],
            "the STORE must name exactly the flag the call named, and nothing else"
        );
    }

    #[tokio::test]
    async fn mcp_delete_message_purges_and_leaves_no_copy_in_a_trash_mailbox() {
        let connector = FakeConnector::new(ConnectOutcome::Session)
            .with_capabilities(Capabilities {
                has_move: false,
                has_uidplus: true,
            })
            .with_messages(vec![message(42, &["\\Seen"])]);
        let ops = Arc::clone(&connector.ops);
        let copies = Arc::clone(&connector.copies);
        let moves = Arc::clone(&connector.moves);
        let state = state_with(Arc::new(NoopMailer), connector);

        let result = call_tool(
            &state,
            "delete_message",
            json!({ "mailbox": "INBOX", "uid": 42 }),
        )
        .await;

        assert_eq!(structured(&result)["status"], "deleted");
        assert_eq!(
            ops.lock().expect("ops log poisoned").as_slice(),
            ["store 42 +FLAGS.SILENT (\\Deleted)", "expunge 42"],
            "the deletion must mark the message and then target it with UID EXPUNGE"
        );
        assert!(
            copies.lock().expect("copies log poisoned").is_empty(),
            "a purge must not COPY the message anywhere — there is no Trash mailbox to file it in"
        );
        assert!(
            moves.lock().expect("moves log poisoned").is_empty(),
            "a purge must not MOVE the message to a Trash mailbox either"
        );
    }

    #[tokio::test]
    async fn mcp_delete_message_without_uidplus_is_501_and_sends_no_command() {
        let connector = FakeConnector::new(ConnectOutcome::Session)
            .with_messages(vec![message(42, &["\\Seen"])]);
        let ops = Arc::clone(&connector.ops);
        let expunges = Arc::clone(&connector.expunges);
        let state = state_with(Arc::new(NoopMailer), connector);

        let result = call_tool(
            &state,
            "delete_message",
            json!({ "mailbox": "INBOX", "uid": 42 }),
        )
        .await;

        assert!(
            tool_error_text(&result).starts_with("capability_not_supported:"),
            "the stable code of a server without UIDPLUS must survive: {result}"
        );
        assert!(
            ops.lock().expect("ops log poisoned").is_empty(),
            "no command — and no global EXPUNGE — may be sent"
        );
        assert!(expunges.lock().expect("expunges log poisoned").is_empty());
    }

    #[tokio::test]
    async fn mcp_move_message_without_a_destination_is_invalid_request() {
        let connector = FakeConnector::new(ConnectOutcome::Session)
            .with_messages(vec![message(42, &["\\Seen"])]);
        let connects = Arc::clone(&connector.connects);
        let state = state_with(Arc::new(NoopMailer), connector);

        let result = call_tool(
            &state,
            "move_message",
            json!({ "mailbox": "INBOX", "uid": 42 }),
        )
        .await;

        assert!(
            tool_error_text(&result).starts_with("invalid_request:"),
            "a missing destination is a request the tool rejects: {result}"
        );
        assert_eq!(
            connects.load(Ordering::SeqCst),
            0,
            "a rejected request must not open a connection"
        );
    }

    #[tokio::test]
    async fn mcp_send_message_needs_a_recipient_and_reaches_smtp_when_it_has_one() {
        let sends = Arc::new(AtomicUsize::new(0));
        let mailer = Arc::new(CountingMailer {
            sends: Arc::clone(&sends),
        });
        let state = state_with(mailer, FakeConnector::new(ConnectOutcome::Session));

        let rejected = call_tool(
            &state,
            "send_message",
            json!({ "to": [], "subject": "no recipient" }),
        )
        .await;
        assert!(
            tool_error_text(&rejected).starts_with("invalid_request:"),
            "a message with no recipient must be rejected: {rejected}"
        );
        assert_eq!(
            sends.load(Ordering::SeqCst),
            0,
            "a rejected message must not reach SMTP"
        );

        let accepted = call_tool(
            &state,
            "send_message",
            json!({
                "to": ["dest@test.example"],
                "subject": "hello",
                "text": "body",
            }),
        )
        .await;
        assert_eq!(structured(&accepted)["status"], "sent");
        assert_eq!(sends.load(Ordering::SeqCst), 1);
    }
}
