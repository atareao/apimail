//! Integration tests for the MCP **Streamable HTTP** transport nested in the
//! HTTP API's router.
//!
//! The five tests below cover the two things §5 has to prove, and nothing else:
//!
//! * the transport is served on the **same** router — hence the same listener —
//!   as the HTTP API, at the configured path (`/mcp` by default) and nowhere
//!   else, and only when the configuration enabled it;
//! * the existing `require_api_key` middleware **does** cover the nested service.
//!   `route_layer` applies the middleware to the routes present when it is
//!   called, and "a `nest_service` counts as one of them" is a security
//!   assumption: it is *demonstrated* here rather than trusted. A `401` on the
//!   MCP path is the proof, because `route_layer` only runs the middleware for a
//!   matched route — an unmounted endpoint answers `404` and never reaches it.
//!
//! Everything goes through `tower::ServiceExt::oneshot` on `build_router(state)`,
//! as the other HTTP test files do: no socket is bound, so the tests are fast and
//! cannot be affected by a busy port. Requests are built by hand, which is also
//! why they carry an explicit `Host` header: `rmcp` validates it against its
//! DNS-rebinding allowlist (`localhost`, `127.0.0.1`, `::1` by default).
//!
//! The file is gated on the `mcp` feature: without it `build_router` has no MCP
//! branch at all and the crate does not even compile `rmcp`, so the default test
//! run is unchanged.

#![cfg(feature = "mcp")]

use std::time::{Duration, Instant};

use apimail::{AppState, Config, build_router};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const API_KEY: &str = "test-secret-key";

/// How long a test waits for a frame of the SSE reply before failing, so a
/// wedged transport fails the test instead of hanging the suite.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// The protocol version the test client **asks for**: `rmcp 3.5.1` pins
/// `ProtocolVersion::LATEST` to `V_2026_07_28`.
const REQUESTED_VERSION: &str = "2026-07-28";

/// The protocol version the server **answers with**: `V_2026_07_28` is also
/// `ProtocolVersion::NO_INITIALIZE` (SEP-2567 moved the lifecycle into
/// per-request `_meta`), so there is no handshake left to negotiate at that
/// revision and the server falls back to `LATEST_WITH_INITIALIZE`, which is
/// `V_2025_11_25`.
const NEGOTIATED_VERSION: &str = "2025-11-25";

/// The header `rmcp` answers an `initialize` with, which the client must echo on
/// every later request of the same session.
const SESSION_HEADER: &str = "mcp-session-id";

/// Builds a configuration from a lookup table instead of the environment.
///
/// The credentials are deliberately fake and the hosts unreachable: nothing here
/// may touch the network, and `AppState::from_config` only builds its services
/// lazily. `mcp` carries the `APIMAIL_MCP_*` variables of each test, so the only
/// difference between two tests is what they enable.
fn config(mcp: &[(&str, &str)]) -> Config {
    Config::from_lookup(|key| match key {
        "APIMAIL_API_KEY" => Some(API_KEY.to_string()),
        "APIMAIL_IMAP_HOST" => Some("imap.invalid".to_string()),
        "APIMAIL_IMAP_USER" => Some("imap-user".to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some("imap-pass".to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.invalid".to_string()),
        "APIMAIL_SMTP_USER" => Some("smtp-user".to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-pass".to_string()),
        _ => mcp
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| (*value).to_string()),
    })
    .expect("valid test config")
}

/// Builds the router with the MCP HTTP transport configured as `mcp` asks.
fn app(mcp: &[(&str, &str)]) -> Router {
    let state = AppState::from_config(&config(mcp)).expect("valid application state");
    build_router(state)
}

/// Sends a `POST` to the MCP endpoint and returns the response.
///
/// The three headers the transport requires are always set — `Host` (the
/// DNS-rebinding allowlist), `Accept` (both MIME types, as the MCP spec demands
/// of a client) and `Content-Type` — plus the `Authorization` and session
/// headers when the test supplies them.
async fn post_mcp(
    app: &Router,
    path: &str,
    authorization: Option<&str>,
    session: Option<&str>,
    body: &Value,
) -> Response<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "localhost")
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(value) = authorization {
        builder = builder.header(header::AUTHORIZATION, value);
    }
    if let Some(value) = session {
        builder = builder.header(SESSION_HEADER, value);
    }
    app.clone()
        .oneshot(
            builder
                .body(Body::from(body.to_string()))
                .expect("valid request"),
        )
        .await
        .expect("the router should respond")
}

/// The `initialize` request of the cycle.
fn initialize_request(id: i64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": REQUESTED_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "apimail-tests", "version": "0.1.0" },
        },
    })
}

/// The `tools/call` request for `whoami`, which takes no arguments.
fn call_whoami_request(id: i64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": "whoami", "arguments": {} },
    })
}

/// Reads the whole body, for the responses that are a small JSON envelope.
async fn body_bytes(response: Response<Body>) -> axum::body::Bytes {
    response
        .into_body()
        .collect()
        .await
        .expect("the body should be readable")
        .to_bytes()
}

/// Reads the whole body and parses it as JSON.
async fn body_json(response: Response<Body>) -> Value {
    serde_json::from_slice(&body_bytes(response).await).expect("valid JSON body")
}

/// Reads an SSE body until it carries the JSON-RPC reply to `id`.
///
/// Frames are read one at a time instead of collecting the body, because a
/// Streamable HTTP response is `text/event-stream` and may stay open (keep-alive
/// pings, reconnection priming): collecting it would hang. The payload of each
/// `data:` line is parsed, which also asserts nothing but JSON-RPC travels on
/// that stream; the priming event's empty payload is skipped.
async fn read_sse_reply(mut body: Body, id: i64) -> Value {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    let mut buffer = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for the reply to {id}; seen so far: {buffer:?}"
        );
        // `BodyExt::frame` yields `Option<Result<Frame<Bytes>, Error>>`: the
        // `Option` is the end of the stream, the `Result` the frame itself.
        let next = tokio::time::timeout(remaining, body.frame())
            .await
            .unwrap_or_else(|_| panic!("timed out reading the SSE body; seen so far: {buffer:?}"));
        let Some(next) = next else {
            panic!("the SSE stream ended before answering {id}; seen so far: {buffer:?}");
        };
        let frame = next.expect("the SSE frame should be readable");
        if let Ok(data) = frame.into_data() {
            buffer.push_str(&String::from_utf8_lossy(&data));
        }
        for line in buffer.lines() {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                // The reconnection priming event carries no message.
                continue;
            }
            let value: Value = serde_json::from_str(data)
                .unwrap_or_else(|error| panic!("the SSE stream carried non-JSON data ({error})"));
            assert_eq!(
                value["jsonrpc"], "2.0",
                "the SSE stream carried a non-JSON-RPC message: {value}"
            );
            if value["id"] == json!(id) {
                return value;
            }
        }
    }
}

/// Asserts `reply` answers `id` successfully and returns its `result`.
fn result_of(reply: &Value, id: i64) -> &Value {
    assert_eq!(
        reply["id"],
        json!(id),
        "the reply answers a different id: {reply}"
    );
    assert!(
        reply.get("error").is_none(),
        "expected a successful reply to {id}, got: {reply}"
    );
    reply
        .get("result")
        .unwrap_or_else(|| panic!("the reply to {id} carries no result: {reply}"))
}

/// **The security check §5 asks for.** A request to the MCP endpoint without a
/// credential is rejected by the *existing* middleware, not by the MCP service:
/// the status is `401` — never `404` — with the shared JSON envelope and the
/// `Bearer` challenge of every other protected route.
///
/// `404` would mean the endpoint is not mounted; `401` can only be produced by
/// `require_api_key`, so it also proves that a `nest_service` is one of the
/// routes `route_layer` covers.
#[tokio::test]
async fn mcp_endpoint_without_credential_is_rejected_by_the_shared_middleware() {
    let response = post_mcp(
        &app(&[("APIMAIL_MCP_HTTP", "true")]),
        "/mcp",
        None,
        None,
        &initialize_request(1),
    )
    .await;

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "the MCP endpoint must be behind `require_api_key`; a 404 here would mean \
         `route_layer` does not cover the nested service, a 200 that it does not \
         authenticate it"
    );
    assert_eq!(
        response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer"),
        "the MCP 401 must carry the same challenge as the HTTP API"
    );
    assert_eq!(
        body_json(response).await,
        json!({ "error": "unauthorized", "message": "missing or invalid API key" }),
        "the MCP 401 must be the shared error envelope"
    );
}

/// A wrong credential is rejected on the MCP endpoint exactly as it is on the
/// HTTP API: a token that is not the configured key, a non-`Bearer` scheme and a
/// `Bearer` with no token all get `401`.
#[tokio::test]
async fn mcp_endpoint_with_a_wrong_credential_is_rejected() {
    let app = app(&[("APIMAIL_MCP_HTTP", "true")]);

    for value in ["Bearer not-the-key", "Basic test-secret-key", "Bearer"] {
        let response = post_mcp(&app, "/mcp", Some(value), None, &initialize_request(1)).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "`{value}` should not authenticate the MCP endpoint"
        );
        assert_eq!(
            body_json(response).await["error"],
            json!("unauthorized"),
            "`{value}` must be answered with the shared envelope"
        );
    }
}

/// With the configured key the MCP **cycle** runs over HTTP on the default path:
/// `initialize` opens a session (`Mcp-Session-Id`, SSE body) and `tools/call`
/// runs `whoami` inside it — the same handler and the same [`AppState`] the stdio
/// transport serves.
#[tokio::test]
async fn mcp_cycle_runs_over_http_with_the_api_key() {
    let app = app(&[("APIMAIL_MCP_HTTP", "true")]);
    let authorization = format!("Bearer {API_KEY}");

    // 1. initialize. The reply names the server from the shared state and the
    //    session id comes back in a header.
    let response = post_mcp(
        &app,
        "/mcp",
        Some(&authorization),
        None,
        &initialize_request(1),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
        "a Streamable HTTP reply is an event stream"
    );
    let session = response
        .headers()
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .expect("initialize must open a session")
        .to_string();

    let initialize = read_sse_reply(response.into_body(), 1).await;
    let result = result_of(&initialize, 1);
    assert_eq!(
        result["protocolVersion"], NEGOTIATED_VERSION,
        "the negotiated version is the newest one with an initialize handshake: {initialize}"
    );
    assert_eq!(result["serverInfo"]["name"], "apimail");
    assert_eq!(
        result["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "serverInfo should come from AppState: {initialize}"
    );

    // 2. tools/call, in the session the handshake opened.
    let response = post_mcp(
        &app,
        "/mcp",
        Some(&authorization),
        Some(&session),
        &call_whoami_request(2),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let call = read_sse_reply(response.into_body(), 2).await;
    let result = result_of(&call, 2);
    assert_eq!(
        result["structuredContent"]["authenticated"],
        json!(true),
        "whoami did not report an authenticated caller: {result}"
    );
    assert_ne!(
        result["isError"],
        json!(true),
        "whoami reported a tool error: {result}"
    );
}

/// `APIMAIL_MCP_PATH` moves the transport: the configured path serves the
/// handshake and the default one stops existing on that router.
#[tokio::test]
async fn mcp_endpoint_honours_the_configured_path() {
    let app = app(&[
        ("APIMAIL_MCP_HTTP", "true"),
        ("APIMAIL_MCP_PATH", "/mcp-alt"),
    ]);
    let authorization = format!("Bearer {API_KEY}");

    let response = post_mcp(
        &app,
        "/mcp-alt",
        Some(&authorization),
        None,
        &initialize_request(1),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the configured path must serve the MCP transport"
    );
    assert!(
        response.headers().get(SESSION_HEADER).is_some(),
        "the transport must answer the handshake at the configured path"
    );
    let initialize = read_sse_reply(response.into_body(), 1).await;
    assert_eq!(result_of(&initialize, 1)["serverInfo"]["name"], "apimail");

    let response = post_mcp(&app, "/mcp", None, None, &initialize_request(2)).await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "the default path must stay unmounted when another one is configured"
    );
}

/// Without `APIMAIL_MCP_HTTP` the state carries no MCP path, so the router mounts
/// nothing: the MCP path is a plain `404` — the very code path a build **without**
/// the `mcp` feature takes, since such a configuration rejects every
/// `APIMAIL_MCP_*` variable outright — while the rest of the API is untouched.
#[tokio::test]
async fn mcp_endpoint_is_absent_when_the_transport_is_disabled() {
    let router = app(&[]);

    let response = post_mcp(&router, "/mcp", None, None, &initialize_request(1)).await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "no MCP route may exist when the transport is not configured"
    );

    // The public route and the protected routes are where they always were.
    let health = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("the router should respond");
    assert_eq!(health.status(), StatusCode::OK);

    let whoami = router
        .oneshot(
            Request::builder()
                .uri("/api/whoami")
                .header(header::AUTHORIZATION, format!("Bearer {API_KEY}"))
                .body(Body::empty())
                .expect("valid request"),
        )
        .await
        .expect("the router should respond");
    assert_eq!(whoami.status(), StatusCode::OK);
}
