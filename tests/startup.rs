//! Integration tests for the binary entrypoint's startup behaviour.

use std::process::Command;

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
