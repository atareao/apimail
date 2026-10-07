//! Integration tests for the binary entrypoint's startup behaviour.

use std::process::Command;

/// An invalid `APIMAIL_PORT` must make the process exit non-zero with a
/// descriptive message, never falling back to the default port.
#[test]
fn invalid_port_exits_non_zero() {
    let output = Command::new(env!("CARGO_BIN_EXE_apimail"))
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
