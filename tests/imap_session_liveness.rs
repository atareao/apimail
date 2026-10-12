//! Integration tests for IMAP session liveness against a **real** socket.
//!
//! The in-memory [`FakeSession`](crate) doubles used elsewhere cannot exercise
//! this defect: the fix lives in the real `SessionHandle`, so a fake that
//! imitated the broken behaviour would stay broken after the fix too. These
//! tests therefore drive the real `TokioImapConnector` (built by
//! `AppState::with_mailer`) against an **in-process fake IMAP server** listening
//! on an ephemeral loopback port, in cleartext (`APIMAIL_IMAP_TLS=none`).
//!
//! The defect (`async-imap` 0.12, `src/imap_stream.rs`, `do_poll_next`): a peer
//! that closes its write side (FIN, no RST) while the read buffer is empty is
//! seen as a clean end of stream, so `noop()`, `select()`, `uid_search()` and
//! `list()` return `Ok` with an empty/default result instead of a connection
//! error. The fake server reproduces that by half-closing *after* `LOGIN` while
//! keeping the socket open to drain the client's writes, so the client's writes
//! never raise `EPIPE` and the truncation is what the client actually observes.
//!
//! All four tests assert the **correct** behaviour required by the
//! `imap-session-liveness` spec, so they fail against the current (broken) code.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use http_body_util::BodyExt;
use tower::ServiceExt;

use apimail::smtp::{MailSender, OutgoingMessage, SendFuture as MailSendFuture};
use apimail::{AppState, Config, build_router};

const API_KEY: &str = "test-liveness-key";
const IMAP_USER: &str = "imap-user@test.example";
const IMAP_PASSWORD: &str = "imap-secret";

/// Loads a valid [`Config`] pointing at the loopback fake server, cleartext.
fn config(port: u16) -> Config {
    Config::from_lookup(|key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some("127.0.0.1".to_string()),
        "APIMAIL_IMAP_PORT" => Some(port.to_string()),
        "APIMAIL_IMAP_USER" => Some(IMAP_USER.to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some(IMAP_PASSWORD.to_string()),
        "APIMAIL_IMAP_TLS" => Some("none".to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.test.example".to_string()),
        "APIMAIL_SMTP_USER" => Some("smtp-user@test.example".to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-secret".to_string()),
        "APIMAIL_SMTP_TLS" => Some("none".to_string()),
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

/// Per-connection behaviour of the fake IMAP server.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Behaviour {
    /// Completes `LOGIN`, then half-closes its write side (FIN, no RST) while
    /// keeping the socket open to drain the client's writes. This is the
    /// "zombie" the liveness probe must detect.
    Zombie,
    /// Serves the whole dialogue (`LOGIN`, `NOOP`, `SELECT`, `UID SEARCH`,
    /// `LIST`) with real data.
    Healthy,
    /// Closes the connection immediately on accept, without a greeting, so a
    /// reconnect attempt fails: the peer is gone for good.
    Reject,
}

/// An in-process fake IMAP server over TCP.
///
/// It serves one [`Behaviour`] per accepted connection, in order; once the list
/// is exhausted it repeats the last entry. This lets a test script "the first
/// connection is a zombie and every later one is rejected/healthy".
struct FakeImapServer {
    /// Ephemeral loopback port the server listens on.
    port: u16,
    /// Number of accepted connections, in accept order.
    connections: Arc<AtomicUsize>,
}

impl FakeImapServer {
    /// Binds an ephemeral loopback port and serves `behaviours` per connection.
    fn start(behaviours: Vec<Behaviour>) -> Self {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("the fake server binds a loopback port");
        let port = listener
            .local_addr()
            .expect("the bound listener has a local address")
            .port();
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&connections);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let index = counter.fetch_add(1, Ordering::SeqCst);
                let behaviour = behaviours
                    .get(index)
                    .or_else(|| behaviours.last())
                    .copied()
                    .unwrap_or(Behaviour::Reject);
                std::thread::spawn(move || serve_connection(stream, behaviour));
            }
        });
        Self { port, connections }
    }

    /// Number of connections accepted so far.
    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

/// Serves one connection according to `behaviour`.
fn serve_connection(stream: TcpStream, behaviour: Behaviour) {
    if behaviour == Behaviour::Reject {
        // Accept then drop: the client's greeting read sees EOF and the connect
        // attempt fails.
        drop(stream);
        return;
    }

    let read_stream = stream
        .try_clone()
        .expect("the accepted socket can be cloned for reading");
    let mut writer = stream;
    let mut reader = BufReader::new(read_stream);
    let mut line = String::new();

    // Greeting.
    if writer
        .write_all(b"* OK [CAPABILITY IMAP4rev1] apimail test server ready\r\n")
        .is_err()
    {
        return;
    }

    // LOGIN: reply with the same tag the client used.
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let tag = first_token(&line);
    if writer
        .write_all(format!("{tag} OK LOGIN completed\r\n").as_bytes())
        .is_err()
    {
        return;
    }

    if behaviour == Behaviour::Zombie {
        // The defect trigger: a clean FIN to the client (no RST) while the
        // socket stays open for reading, so the client's later writes do not
        // raise `EPIPE` and the truncation is what the client observes.
        let _ = writer.shutdown(Shutdown::Write);
        // Drain whatever the client writes so the socket is never reset; it is
        // the client that eventually closes and ends this loop.
        let mut sink = [0u8; 1024];
        loop {
            match reader.read(&mut sink) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        return;
    }

    // Healthy: serve commands with real data.
    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let mut parts = line.split_whitespace();
        let tag = parts.next().unwrap_or("").to_string();
        let command = parts.next().unwrap_or("").to_ascii_uppercase();
        let response = match command.as_str() {
            "NOOP" => format!("* OK NOOP completed\r\n{tag} OK NOOP completed\r\n"),
            "LIST" => format!(
                "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
                 * LIST (\\HasNoChildren) \"/\" \"Sent\"\r\n\
                 {tag} OK LIST completed\r\n"
            ),
            "SELECT" => format!(
                "* 3 EXISTS\r\n\
                 * 0 RECENT\r\n\
                 * OK [UIDVALIDITY 1] UIDs valid\r\n\
                 * OK [UIDNEXT 4] Predicted next UID\r\n\
                 * FLAGS (\\Seen)\r\n\
                 {tag} OK [READ-WRITE] SELECT completed\r\n"
            ),
            "UID" => {
                let subcommand = parts.next().unwrap_or("").to_ascii_uppercase();
                if subcommand == "SEARCH" {
                    format!("* SEARCH\r\n{tag} OK SEARCH completed\r\n")
                } else {
                    format!("{tag} OK completed\r\n")
                }
            }
            _ => format!("{tag} OK completed\r\n"),
        };
        if writer.write_all(response.as_bytes()).is_err() {
            break;
        }
    }
}

/// The first whitespace-delimited token of a command line (its tag).
fn first_token(line: &str) -> String {
    line.split_whitespace().next().unwrap_or("").to_string()
}

/// Builds the router backed by the **real** IMAP connector, aimed at `server`.
fn app_for(server: &FakeImapServer) -> Router {
    build_router(
        AppState::with_mailer(&config(server.port), Arc::new(NoopMailer))
            .expect("building the app state must not touch the network"),
    )
}

/// Sends a `GET` request with the test API key.
async fn get(app: &Router, uri: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {API_KEY}"))
                .body(Body::empty())
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

/// Establishes *and caches* a session on the fake server's first connection.
///
/// A bare `GET /api/imap/status` with no cached session opens a connection and
/// caches it without running any data command, so a zombie first connection
/// ends up cached and idle — exactly the production state the probe must catch.
async fn cache_first_connection(app: &Router) {
    let response = get(app, "/api/imap/status").await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the prewarm status call must cache the freshly opened session"
    );
}

#[tokio::test]
async fn liveness_probe_rejects_the_zombie_session() {
    // First connection: zombie (cached by the prewarm). Any reconnect is
    // rejected, because the peer is gone.
    let server = FakeImapServer::start(vec![Behaviour::Zombie, Behaviour::Reject]);
    let app = app_for(&server);

    cache_first_connection(&app).await;

    let response = get(&app, "/api/imap/status").await;
    let status = response.status();
    let json = body_json(response).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "la sonda de vida debe descartar la sesión zombi y, al no poder reconectar, \
         responder 503; obtuve {status} con cuerpo {json}"
    );
    assert_eq!(
        json["connected"], false,
        "una conexión cerrada por el par no puede reportarse como conectada: {json}"
    );
    assert_eq!(
        json["error"], "imap_unavailable",
        "el 503 debe llevar el código estable imap_unavailable: {json}"
    );
}

#[tokio::test]
async fn listing_messages_over_a_dead_peer_is_503_not_an_empty_page() {
    let server = FakeImapServer::start(vec![Behaviour::Zombie, Behaviour::Reject]);
    let app = app_for(&server);

    cache_first_connection(&app).await;

    let response = get(&app, "/api/messages?mailbox=INBOX&unseen=true&limit=50").await;
    let status = response.status();
    let json = body_json(response).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "un listado sobre una conexión cerrada por el par debe ser 503, nunca un 200 \
         con la página vacía; obtuve {status} con cuerpo {json}"
    );
    assert_ne!(
        status,
        StatusCode::OK,
        "el listado truncado no puede presentarse como un 200 válido: {json}"
    );
    assert_eq!(
        json["error"], "imap_unavailable",
        "el 503 debe llevar el código estable imap_unavailable: {json}"
    );
    assert!(
        json["message"].is_string(),
        "el 503 debe llevar un mensaje de texto: {json}"
    );
}

#[tokio::test]
async fn listing_mailboxes_over_a_dead_peer_is_503_not_an_empty_list() {
    // Closes item #10 of `plans/PLAN-001.md` (`mailboxes-empty-response`).
    let server = FakeImapServer::start(vec![Behaviour::Zombie, Behaviour::Reject]);
    let app = app_for(&server);

    cache_first_connection(&app).await;

    let response = get(&app, "/api/mailboxes").await;
    let status = response.status();
    let json = body_json(response).await;

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "un listado de buzones sobre una conexión cerrada por el par debe ser 503, \
         nunca un 200 con {{\"mailboxes\":[]}}; obtuve {status} con cuerpo {json}"
    );
    assert_eq!(
        json["error"], "imap_unavailable",
        "el 503 debe llevar el código estable imap_unavailable: {json}"
    );
    assert!(
        json.get("mailboxes").is_none()
            || json["mailboxes"].as_array().is_none_or(|m| !m.is_empty()),
        "un 503 no puede acompañarse de una lista de buzones vacía presentada como válida: {json}"
    );
}

#[tokio::test]
async fn dead_session_is_discarded_and_the_reconnect_serves_real_data() {
    // First connection: zombie (cached by the prewarm). The reconnect gets a
    // healthy session.
    let server = FakeImapServer::start(vec![Behaviour::Zombie, Behaviour::Healthy]);
    let app = app_for(&server);

    cache_first_connection(&app).await;

    let response = get(&app, "/api/mailboxes").await;
    let status = response.status();
    let json = body_json(response).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "tras descartar la zombi y reconectar a la sesión sana, la operación debe \
         devolver los datos reales; obtuve {status} con cuerpo {json}"
    );

    let mailboxes = json["mailboxes"]
        .as_array()
        .expect("el 200 debe llevar un array `mailboxes`");
    let names: Vec<&str> = mailboxes
        .iter()
        .filter_map(|mailbox| mailbox["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["INBOX", "Sent"],
        "los buzones deben venir de la sesión sana reconectada, nunca de los vacíos \
         de la zombi: {json}"
    );
    assert!(
        server.connections() >= 2,
        "descartar la zombi debe abrir una segunda conexión; el servidor aceptó {}",
        server.connections()
    );
}
