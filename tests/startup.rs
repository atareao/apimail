//! Integration tests for the binary entrypoint's startup behaviour.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Owns a spawned child process and guarantees it is killed on drop, so a
/// failing assertion cannot leak a stray server.
struct ChildGuard {
    /// The spawned process, exposed only through this guard.
    child: Child,
}

impl ChildGuard {
    /// Wraps a freshly spawned child.
    fn new(child: Child) -> Self {
        Self { child }
    }

    /// Kills the child and returns whatever it had written to stderr.
    ///
    /// Used to report the child's output when the health probe times out.
    fn kill_and_collect_stderr(&mut self) -> String {
        let _ = self.child.kill();
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let _ = self.child.wait();
        stderr
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Builds a command pre-loaded with a complete, valid configuration (API key and
/// mail account) so each test can remove or override exactly one variable.
fn configured_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_apimail"));
    command
        .env("APIMAIL_API_KEY", "test-key")
        .env("APIMAIL_IMAP_HOST", "imap.example.com")
        .env("APIMAIL_IMAP_USER", "imap-user")
        .env("APIMAIL_IMAP_PASSWORD", "imap-pass")
        .env("APIMAIL_SMTP_HOST", "smtp.example.com")
        .env("APIMAIL_SMTP_USER", "smtp-user")
        .env("APIMAIL_SMTP_PASSWORD", "smtp-pass");
    command
}

/// An invalid `APIMAIL_PORT` must make the process exit non-zero with a
/// descriptive message, never falling back to the default port. A valid API key
/// is supplied so the failure is isolated to the port.
#[test]
fn invalid_port_exits_non_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_apimail"))
        .env("APIMAIL_API_KEY", "test-key")
        .env("APIMAIL_PORT", "abc")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_PORT"),
        "stderr should mention APIMAIL_PORT, got: {stderr:?}"
    );
}

/// Without `APIMAIL_API_KEY` the service must refuse to start unprotected.
#[test]
fn missing_api_key_exits_non_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_apimail"))
        .env_remove("APIMAIL_API_KEY")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_API_KEY"),
        "stderr should mention APIMAIL_API_KEY, got: {stderr:?}"
    );
}

/// An empty `APIMAIL_API_KEY` is as invalid as a missing one.
#[test]
fn empty_api_key_exits_non_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_apimail"))
        .env("APIMAIL_API_KEY", "")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );
}

/// A missing required mail-account variable must make the service refuse to
/// start, mentioning the offending variable.
#[test]
fn missing_mail_account_variable_exits_non_zero() {
    let output = configured_command()
        .env_remove("APIMAIL_IMAP_HOST")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_IMAP_HOST"),
        "stderr should mention APIMAIL_IMAP_HOST, got: {stderr:?}"
    );
}

/// An invalid IMAP port must make the process exit non-zero with a descriptive
/// message.
#[test]
fn invalid_mail_port_exits_non_zero() {
    let output = configured_command()
        .env("APIMAIL_IMAP_PORT", "abc")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_IMAP_PORT"),
        "stderr should mention APIMAIL_IMAP_PORT, got: {stderr:?}"
    );
}

/// A non-numeric attachment limit must make the process exit non-zero with a
/// descriptive message naming the offending variable.
#[test]
fn invalid_max_attachment_bytes_exits_non_zero() {
    let output = configured_command()
        .env("APIMAIL_MAX_ATTACHMENT_BYTES", "abc")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_MAX_ATTACHMENT_BYTES"),
        "stderr should mention APIMAIL_MAX_ATTACHMENT_BYTES, got: {stderr:?}"
    );
}

/// An unsupported TLS mode must make the process exit non-zero with a
/// descriptive message.
#[test]
fn invalid_tls_exits_non_zero() {
    let output = configured_command()
        .env("APIMAIL_IMAP_TLS", "ssl")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_IMAP_TLS"),
        "stderr should mention APIMAIL_IMAP_TLS, got: {stderr:?}"
    );
}

/// A non-numeric IMAP timeout must make the process exit non-zero with a
/// descriptive message naming the offending variable.
#[test]
fn invalid_imap_timeout_exits_non_zero() {
    let output = configured_command()
        .env("APIMAIL_IMAP_TIMEOUT_SECS", "abc")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_IMAP_TIMEOUT_SECS"),
        "stderr should mention APIMAIL_IMAP_TIMEOUT_SECS, got: {stderr:?}"
    );
}

/// The service must start and serve `GET /api/health` even when the configured
/// IMAP server is unreachable: no network is opened at startup.
///
/// A free port is reserved, the binary is pointed at an unreachable IMAP
/// endpoint, and the test polls until the HTTP server answers `200 OK`.
#[test]
fn starts_and_serves_health_when_imap_is_unreachable() {
    // Reserve a free port and release it for the server to bind.
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a free loopback port");
        listener.local_addr().expect("bound local address").port()
    };

    let child = configured_command()
        .env("APIMAIL_HOST", "127.0.0.1")
        .env("APIMAIL_PORT", port.to_string())
        // Port 1 on the loopback is not an IMAP server; connecting fails fast.
        .env("APIMAIL_IMAP_HOST", "127.0.0.1")
        .env("APIMAIL_IMAP_PORT", "1")
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn apimail binary");
    let mut guard = ChildGuard::new(child);

    let request = "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_error = String::from("no probe attempted yet");

    loop {
        if Instant::now() >= deadline {
            let stderr = guard.kill_and_collect_stderr();
            panic!(
                "the service did not answer /api/health within 5s despite the IMAP \
                 server being unreachable; last probe error: {last_error}; stderr: {stderr:?}"
            );
        }

        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(mut stream) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut response = String::new();
                if stream.write_all(request.as_bytes()).is_ok()
                    && stream.read_to_string(&mut response).is_ok()
                    && response.contains("200 OK")
                {
                    // Success: `guard` drops here and kills the child.
                    return;
                }
                last_error = response;
            }
            Err(error) => last_error = error.to_string(),
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

/// An invalid value must never be echoed to stderr: it may carry a credential.
#[test]
fn invalid_value_is_not_echoed_in_stderr() {
    let output = configured_command()
        .env("APIMAIL_PORT", "not-a-port-secret-value")
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_PORT"),
        "stderr should mention the variable, got: {stderr:?}"
    );
    assert!(
        !stderr.contains("not-a-port-secret-value"),
        "stderr must not echo the raw value, got: {stderr:?}"
    );
}

/// A malformed webhook URL may embed userinfo; the credential must not reach stderr.
#[test]
fn invalid_webhook_url_credentials_are_not_echoed_in_stderr() {
    let output = configured_command()
        .env(
            "APIMAIL_WEBHOOK_URL",
            "ftp://user:s3cret-token@hooks.example.com/incoming",
        )
        .output()
        .expect("failed to spawn apimail binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit, got {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("APIMAIL_WEBHOOK_URL"),
        "stderr should mention the variable, got: {stderr:?}"
    );
    assert!(
        !stderr.contains("s3cret-token"),
        "stderr must not echo the credential, got: {stderr:?}"
    );
}
