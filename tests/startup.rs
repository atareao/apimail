//! Integration tests for the binary entrypoint's startup behaviour.

use std::process::Command;

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
