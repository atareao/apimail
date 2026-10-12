# imap-connection Specification

## Purpose
Establece, reutiliza y reconecta la sesión IMAP contra el servidor configurado en
`mail-account`, usando TLS según el modo configurado y un timeout acotado. La
conexión es perezosa (no se abre en el arranque) y su estado es observable por
`GET /api/imap/status`.

## Requirements

### Requirement: IMAP session establishment
The service SHALL establish an authenticated IMAP session by opening a TCP connection to the configured host and port, negotiating TLS according to the configured mode and authenticating with the configured credentials.

#### Scenario: Implicit TLS
- **WHEN** the configured IMAP mode is `implicit`
- **THEN** the connection is encrypted from the first byte

#### Scenario: STARTTLS upgrade
- **WHEN** the configured IMAP mode is `starttls`
- **THEN** the connection starts in clear text and is upgraded with the `STARTTLS` command before authenticating

#### Scenario: No TLS
- **WHEN** the configured IMAP mode is `none`
- **THEN** the connection is established without TLS

### Requirement: Lazy persistent session
The service SHALL NOT open an IMAP connection at startup and SHALL establish the session on first use and reuse it for subsequent operations.

#### Scenario: Startup without a reachable server
- **WHEN** the service starts with an unreachable IMAP server
- **THEN** it still starts and serves `GET /api/health`

#### Scenario: Session reuse
- **WHEN** two operations run in sequence against a live session
- **THEN** only one connection is established

### Requirement: Reconnection with backoff
The service SHALL detect a dead session, discard it and reconnect, retrying with exponential backoff up to a bounded number of attempts before reporting failure. Detection SHALL be real: a session whose peer has closed the connection SHALL be observed as dead by the liveness probe and SHALL be discarded and replaced before any data command runs.

#### Scenario: Dead session is replaced
- **WHEN** the cached session is found dead on next use
- **THEN** the service discards it and establishes a new session

#### Scenario: A liveness probe that sees a truncated stream reports the session dead
- **WHEN** the liveness probe against the cached session observes the response stream ending without the command's tagged reply, because the peer closed the connection
- **THEN** the session is treated as dead, discarded and a new session is established before the data command runs

#### Scenario: Bounded retries
- **WHEN** every connection attempt fails
- **THEN** the service gives up after a bounded number of attempts and reports an error

### Requirement: Connection timeout configuration
The service SHALL read a timeout in seconds from the `APIMAIL_IMAP_TIMEOUT_SECS` environment variable, defaulting to 30, SHALL refuse to start when the value is present but not a positive integer, and SHALL apply the timeout to the TCP connection, the TLS handshake and the login.

#### Scenario: Default timeout
- **WHEN** `APIMAIL_IMAP_TIMEOUT_SECS` is unset
- **THEN** the timeout is 30 seconds

#### Scenario: Configured timeout
- **WHEN** `APIMAIL_IMAP_TIMEOUT_SECS` is set to a positive integer
- **THEN** that value is used

#### Scenario: Invalid timeout
- **WHEN** `APIMAIL_IMAP_TIMEOUT_SECS` is set to `0` or a non-numeric value
- **THEN** startup fails with a non-zero exit and a descriptive error

### Requirement: Connection status endpoint
The service SHALL expose `GET /api/imap/status` as a protected route that attempts to ensure a live connection and reports the outcome, without exposing any credential.

#### Scenario: Connected
- **WHEN** `GET /api/imap/status` is requested with a valid API key and the server is reachable
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports `"connected":true` along with the host, port and TLS mode

#### Scenario: Unreachable server
- **WHEN** `GET /api/imap/status` is requested with a valid API key and the server cannot be reached
- **THEN** the response status is `503`
- **AND** the body reports `"connected":false` with an error message

#### Scenario: Unauthenticated
- **WHEN** `GET /api/imap/status` is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: IMAP secret protection
The service SHALL never emit the IMAP password in logs, debug output or HTTP responses.

#### Scenario: Redacted state
- **WHEN** the application state is formatted with `Debug`
- **THEN** the output contains none of the configured secrets

### Requirement: Incomplete command responses
The service SHALL treat an IMAP command whose response stream ends before the command's tagged reply as a connection failure, and SHALL NOT surface it as a valid empty or default result.

#### Scenario: A truncated command is a connection failure
- **WHEN** a command's response stream ends without its tagged reply because the peer closed the connection
- **THEN** the operation fails with a connection error
- **AND** it never yields an empty result, a default mailbox, zero UIDs or an empty mailbox list
