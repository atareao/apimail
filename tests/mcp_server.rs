//! Integration tests for the MCP server served over standard input and output.
//!
//! The file holds the two tests that cover the two entrypoints of the feature,
//! and nothing else:
//!
//! * the **in-process** cycle ([`tool_cycle_runs_in_process_over_a_pipe`]): the
//!   server is handed one end of a `tokio::io::duplex` pipe and driven through
//!   the real `initialize` → `tools/list` → `tools/call` cycle from the other.
//!   No socket, no child process, no network — this is what proves the handler
//!   and the tool router are wired correctly.
//! * the **subprocess** cycle ([`binary_keeps_stdout_protocol_only`]): the
//!   compiled binary is started with `APIMAIL_MCP_STDIO=true` and driven over
//!   its own standard input/output. It is the only way to assert the *stream
//!   discipline* the transport depends on — standard output carries nothing but
//!   JSON-RPC while every diagnostic goes to standard error — because the
//!   process's own `stdout` cannot be captured from inside the process.
//!
//! Both tests use fake credentials pointing at an unreachable host, so neither
//! opens a connection: `whoami` is deliberately network-free.
//!
//! The file is gated on the `mcp` feature. Without it the crate never compiles
//! `rmcp`, so neither test could even name what it exercises; the default build
//! therefore does not see this file at all.

#![cfg(feature = "mcp")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use apimail::{ApimailMcp, AppState, Config};
use rmcp::ServiceExt;
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader as AsyncBufReader, Lines,
};

/// How long a test waits for a reply before failing, so a wedged transport
/// fails the test instead of hanging the suite.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

/// The protocol version the test client **asks for**: `rmcp 3.5.1` pins
/// `ProtocolVersion::LATEST` to `V_2026_07_28`.
const REQUESTED_VERSION: &str = "2026-07-28";

/// The protocol version the server **answers with**: `V_2026_07_28` is also
/// `ProtocolVersion::NO_INITIALIZE` (SEP-2567 moved the lifecycle into
/// per-request `_meta`), so there is no handshake left to negotiate at that
/// revision and the server falls back to `LATEST_WITH_INITIALIZE`, which is
/// `V_2025_11_25`. Asserting this is asserting what the SDK really does, not
/// what the request asks for.
const NEGOTIATED_VERSION: &str = "2025-11-25";

/// Builds a configuration from a lookup table instead of the environment.
///
/// The credentials are deliberately fake and the hosts unreachable: nothing in
/// either test may touch the network, and `AppState::from_config` only builds
/// its services lazily.
fn test_config() -> Config {
    Config::from_lookup(|key| match key {
        "APIMAIL_API_KEY" => Some("test-secret-key".to_string()),
        "APIMAIL_IMAP_HOST" => Some("imap.invalid".to_string()),
        "APIMAIL_IMAP_USER" => Some("imap-user".to_string()),
        "APIMAIL_IMAP_PASSWORD" => Some("imap-pass".to_string()),
        "APIMAIL_SMTP_HOST" => Some("smtp.invalid".to_string()),
        "APIMAIL_SMTP_USER" => Some("smtp-user".to_string()),
        "APIMAIL_SMTP_PASSWORD" => Some("smtp-pass".to_string()),
        _ => None,
    })
    .expect("valid test config")
}

/// The `initialize` request, asking for the SDK's newest revision on purpose so
/// the reply has to name the newest revision that still has a handshake.
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

/// The `tools/list` request.
fn list_tools_request(id: i64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {} })
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

/// Parses a line received from the server, asserting it is a JSON-RPC message.
///
/// A line that is not JSON, or is JSON without `"jsonrpc": "2.0"`, means
/// something other than the protocol reached the stream.
fn parse_json_rpc(line: &str) -> Value {
    let value: Value = serde_json::from_str(line)
        .unwrap_or_else(|error| panic!("stream carried a non-JSON line ({error}): {line:?}"));
    assert_eq!(
        value["jsonrpc"], "2.0",
        "stream carried a non-JSON-RPC message: {line:?}"
    );
    value
}

/// Asserts `reply` answers `id` successfully and returns its `result` object.
fn result_of(reply: &Value, id: i64) -> &Value {
    assert_eq!(
        reply["id"],
        json!(id),
        "reply answers a different id: {reply}"
    );
    assert!(
        reply.get("error").is_none(),
        "expected a successful reply to {id}, got: {reply}"
    );
    reply
        .get("result")
        .unwrap_or_else(|| panic!("reply to {id} carries no result: {reply}"))
}

/// Asserts the `whoami` tool result reports an authenticated caller.
fn assert_whoami_result(result: &Value, origin: &str) {
    assert_eq!(
        result["structuredContent"]["authenticated"],
        json!(true),
        "whoami did not report an authenticated caller ({origin}): {result}"
    );
    assert_ne!(
        result["isError"],
        json!(true),
        "whoami reported a tool error ({origin}): {result}"
    );
    // `Json<T>` returns both the structured content and a text mirror of it, so
    // a client that only reads `content` sees the same fact.
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("whoami returned no text content ({origin}): {result}"));
    let mirrored: Value =
        serde_json::from_str(text).expect("the text mirror of whoami should be JSON");
    assert_eq!(mirrored["authenticated"], json!(true));
}

/// Writes one newline-delimited JSON-RPC message.
async fn send<W>(writer: &mut W, message: &Value)
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

/// Reads messages until the reply to `id` arrives, recording every message seen.
///
/// Notifications (which carry no `id`) are skipped but still parsed, so a stray
/// non-protocol line fails the test wherever it appears.
async fn await_reply<R>(lines: &mut Lines<R>, transcript: &mut Vec<Value>, id: i64) -> Value
where
    R: AsyncBufRead + Unpin,
{
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let received = tokio::time::timeout(remaining, lines.next_line()).await;
        let line = match received {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => panic!("the server closed the stream before answering {id}"),
            Ok(Err(error)) => panic!("failed to read from the server: {error}"),
            Err(_) => panic!("timed out waiting for the reply to {id}"),
        };
        if line.trim().is_empty() {
            continue;
        }
        let value = parse_json_rpc(&line);
        let is_reply = value["id"] == json!(id);
        transcript.push(value.clone());
        if is_reply {
            return value;
        }
    }
}

/// Drives the whole MCP cycle — `initialize`, `tools/list`, `tools/call` — in
/// process, over an in-memory pipe, against [`ApimailMcp`] itself.
///
/// This is the test that covers the handler: the tool router, the schema-bearing
/// `WhoamiOutput` DTO and the `get_info` override are all exercised through the
/// real transport rather than by calling the tool method directly.
#[tokio::test]
async fn tool_cycle_runs_in_process_over_a_pipe() {
    let state = AppState::from_config(&test_config()).expect("valid application state");

    // Two connected ends of an in-memory pipe: the server holds one, the client
    // below the other. `(read, write)` is exactly the pair `rmcp` accepts as a
    // transport, so a writer to one end is read at the other.
    let (server_end, client_end) = tokio::io::duplex(64 * 1024);
    let (server_read, server_write) = tokio::io::split(server_end);
    let server = tokio::spawn(async move {
        ApimailMcp::new(state)
            .serve((server_read, server_write))
            .await
    });

    let (client_read, mut client_write) = tokio::io::split(client_end);
    let mut lines = AsyncBufReader::new(client_read).lines();
    let mut transcript = Vec::new();

    // 1. initialize. The reply must agree on the newer handshake-bearing
    //    revision and name the server from the shared state.
    send(&mut client_write, &initialize_request(1)).await;
    let initialize = await_reply(&mut lines, &mut transcript, 1).await;
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
    assert!(
        result["capabilities"]["tools"].is_object(),
        "the server should advertise the tools capability: {initialize}"
    );

    // The client acknowledges the handshake, as a real client does.
    send(
        &mut client_write,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;

    // 2. tools/list. Exactly one tool is advertised, `whoami`, with a schema.
    send(&mut client_write, &list_tools_request(2)).await;
    let listing = await_reply(&mut lines, &mut transcript, 2).await;
    let result = result_of(&listing, 2);
    let tools = result["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list returned no array: {listing}"));
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["whoami"],
        "the MCP surface is `whoami` and nothing else: {listing}"
    );
    assert!(
        tools[0]["inputSchema"].is_object(),
        "a tool must publish an input schema: {listing}"
    );
    assert!(
        tools[0]["description"]
            .as_str()
            .is_some_and(|description| !description.is_empty()),
        "a tool must publish a description: {listing}"
    );

    // 3. tools/call. The only fact `whoami` asserts is authentication.
    send(&mut client_write, &call_whoami_request(3)).await;
    let call = await_reply(&mut lines, &mut transcript, 3).await;
    assert_whoami_result(result_of(&call, 3), "in process");

    // Every message on the stream was protocol, including the notification that
    // the skip logic in `await_reply` had to step over.
    assert!(
        transcript
            .iter()
            .all(|message| message["jsonrpc"] == json!("2.0")),
        "the stream carried a non-protocol message: {transcript:?}"
    );

    // 4. Closing the client side ends the session: the server must shut down on
    //    EOF instead of spinning.
    drop(client_write);
    drop(lines);
    let running = server
        .await
        .expect("the server task should not panic")
        .expect("the handshake should succeed");
    let reason = tokio::time::timeout(REPLY_TIMEOUT, running.waiting())
        .await
        .expect("the server should stop once the peer disconnects")
        .expect("the server should stop cleanly");
    assert!(
        matches!(reason, rmcp::service::QuitReason::Closed),
        "the session should end because the peer closed it: {reason:?}"
    );
}

/// Owns a spawned MCP child so a failing assertion cannot leak the process.
struct ChildGuard {
    /// The spawned binary.
    child: Child,
    /// The thread draining its standard output, joined once the child is dead.
    reader: Option<thread::JoinHandle<()>>,
}

impl ChildGuard {
    /// Kills the child, joins the reader thread and returns its standard error.
    fn finish(&mut self) -> String {
        let _ = self.child.kill();
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        stderr
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Reads messages from an already-collected transcript until the reply to `id`.
fn await_subprocess_reply(transcript: &[Value], id: i64) -> Value {
    transcript
        .iter()
        .find(|message| message["id"] == json!(id))
        .cloned()
        .unwrap_or_else(|| panic!("the binary never answered request {id}: {transcript:?}"))
}

/// Runs the compiled binary in stdio mode and asserts the two streams stay in
/// their lanes: standard output is protocol-only, standard error carries the
/// logs.
///
/// The binary is pointed at a port this test holds open, so a stray
/// `TcpListener::bind` would abort the startup and no reply would ever arrive —
/// which also documents that stdio mode binds nothing.
#[test]
fn binary_keeps_stdout_protocol_only() {
    // Hold a loopback port for the whole test: stdio mode must not want it.
    let occupied = TcpListener::bind("127.0.0.1:0").expect("a free loopback port");
    let port = occupied.local_addr().expect("bound address").port();

    let mut child = Command::new(env!("CARGO_BIN_EXE_apimail"))
        .env("APIMAIL_MCP_STDIO", "true")
        .env("APIMAIL_API_KEY", "test-key")
        .env("APIMAIL_IMAP_HOST", "imap.invalid")
        .env("APIMAIL_IMAP_USER", "imap-user")
        .env("APIMAIL_IMAP_PASSWORD", "imap-pass")
        .env("APIMAIL_SMTP_HOST", "smtp.invalid")
        .env("APIMAIL_SMTP_USER", "smtp-user")
        .env("APIMAIL_SMTP_PASSWORD", "smtp-pass")
        .env("APIMAIL_HOST", "127.0.0.1")
        .env("APIMAIL_PORT", port.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn the apimail binary");

    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let (lines_tx, lines_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if lines_tx.send(line).is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
    let mut guard = ChildGuard {
        child,
        reader: Some(reader),
    };

    // Drive the cycle over the process's real streams.
    let mut transcript = Vec::new();
    for request in [initialize_request(1), call_whoami_request(3)] {
        writeln!(stdin, "{request}").expect("the request should be written to the child");
        stdin.flush().expect("the request should reach the child");

        let id = request["id"].clone();
        let line = collect_until(&lines_rx, &mut transcript, &id);
        let value = parse_json_rpc(&line);
        transcript.push(value);
    }

    let stderr = guard.finish();

    // Every single line the process wrote to its standard output is JSON-RPC.
    for message in &transcript {
        assert_eq!(
            message["jsonrpc"], "2.0",
            "the binary wrote something other than the protocol to stdout: {message}"
        );
    }

    let initialize = await_subprocess_reply(&transcript, 1);
    let result = result_of(&initialize, 1);
    assert_eq!(
        result["protocolVersion"], NEGOTIATED_VERSION,
        "the binary negotiated a different version: {initialize}"
    );
    assert_eq!(result["serverInfo"]["name"], "apimail");

    let call = await_subprocess_reply(&transcript, 3);
    assert_whoami_result(result_of(&call, 3), "subprocess");

    // The logs are on standard error, and they are not protocol.
    assert!(
        stderr.contains("MCP over stdio"),
        "stderr should carry the startup log of the stdio transport, got: {stderr:?}"
    );
    assert!(
        !stderr.contains("\"jsonrpc\""),
        "stderr must not carry protocol frames, got: {stderr:?}"
    );
    // And nothing but the protocol went to standard output: not one line of it
    // is a log record.
    assert!(
        transcript
            .iter()
            .all(|message| message.get("level").is_none()),
        "a log record reached stdout: {transcript:?}"
    );
}

/// Blocks until a stdout line answering `id` arrives, asserting every line seen
/// on the way is protocol.
fn collect_until(lines: &Receiver<String>, transcript: &mut Vec<Value>, id: &Value) -> String {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for the reply to {id} from the binary; seen so far: {transcript:?}"
        );
        let line = lines
            .recv_timeout(remaining)
            .unwrap_or_else(|error| panic!("failed to read stdout of the binary: {error}"));
        if line.trim().is_empty() {
            continue;
        }
        let value = parse_json_rpc(&line);
        if value["id"] == *id {
            return line;
        }
        transcript.push(value);
    }
}
