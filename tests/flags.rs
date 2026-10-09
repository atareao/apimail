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
    assert_eq!(
        copies.lock().expect("copies log poisoned").as_slice(),
        [(42, "Archive".to_string())]
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
        assert_eq!(body_json(response).await["error"], "imap_unavailable");
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
