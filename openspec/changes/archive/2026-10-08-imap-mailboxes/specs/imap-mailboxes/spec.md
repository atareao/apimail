# Spec Delta

## Purpose
Expone el árbol de buzones de la cuenta y permite seleccionar uno, dejándolo como buzón
activo de la sesión IMAP reutilizada por `imap-connection`, para las operaciones de
lectura y banderas posteriores.

## ADDED Requirements

### Requirement: Mailbox listing
The service SHALL expose `GET /api/mailboxes` as a protected route that lists the account's mailboxes, reporting for each one its name, its hierarchy delimiter and its attributes, without exposing any credential.

#### Scenario: Mailboxes are listed
- **WHEN** `GET /api/mailboxes` is requested with a valid API key and the server is reachable
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body is `{"mailboxes":[{"name":"...","delimiter":"/","attributes":["\\HasNoChildren"]}]}`, where `delimiter` is `null` when the server reports no hierarchy

#### Scenario: Unreachable server
- **WHEN** `GET /api/mailboxes` is requested with a valid API key and the server cannot be reached
- **THEN** the response status is `503`
- **AND** the body is `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated listing
- **WHEN** `GET /api/mailboxes` is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Mailbox selection
The service SHALL expose `POST /api/mailboxes/select` as a protected route that selects the named mailbox on the current IMAP session and reports its status, leaving it active for subsequent operations.

#### Scenario: Mailbox is selected
- **WHEN** `POST /api/mailboxes/select` is requested with `{"mailbox":"INBOX"}` and the mailbox exists
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports the mailbox name together with `exists`, `recent`, `unseen`, `uid_validity`, `uid_next` (nullable where the server omits the value) and the mailbox `flags`

#### Scenario: Unknown mailbox
- **WHEN** the named mailbox does not exist (the server answers with a `NO` response)
- **THEN** the response status is `404`
- **AND** the body is `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Invalid request
- **WHEN** the request body is malformed JSON or omits a non-empty `mailbox`
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`

#### Scenario: Unreachable server on selection
- **WHEN** the server cannot be reached
- **THEN** the response status is `503`
- **AND** the body reports `"error":"imap_unavailable"`

#### Scenario: Unauthenticated selection
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Session reuse for mailbox operations
The service SHALL run mailbox operations on the existing lazily-established IMAP session, reconnecting only when the cached session is found dead, so listing and selecting do not open a new connection while a live session exists.

#### Scenario: Operations reuse a live session
- **WHEN** a listing and a selection run in sequence against a live session
- **THEN** only one connection is established

#### Scenario: Dead session is replaced
- **WHEN** the cached session is found dead on next use
- **THEN** the service discards it, reconnects within the bounded attempts and then runs the operation

### Requirement: Mailbox response hygiene
The service SHALL never emit the IMAP password or third-party error text in logs, debug output or HTTP responses.

#### Scenario: Stable error messages
- **WHEN** a mailbox operation fails with a connection or protocol error
- **THEN** the HTTP body carries the stable `imap_unavailable` code and a fixed message

#### Scenario: Redacted state
- **WHEN** the application state or the connection manager is formatted with `Debug`
- **THEN** the output contains none of the configured secrets
