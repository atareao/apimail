# imap-messages Specification

## Purpose
Expone la lectura de mensajes de un buzón de la cuenta: listar y buscar por criterios
estándar de IMAP (SEARCH) y descargar los metadatos, las cabeceras o el mensaje crudo
completo (FETCH), reutilizando la sesión IMAP perezosa de `imap-connection` y sin fijar el
flag `\Seen` sobre los mensajes leídos.

## Requirements

### Requirement: Message listing and search
The service SHALL expose `GET /api/messages` as a protected route that selects the named mailbox and returns the messages matching the given search criteria, paginated and ordered by descending `UID`, without setting the `\Seen` flag.

#### Scenario: Messages are listed with default pagination
- **WHEN** `GET /api/messages?mailbox=INBOX` is requested with a valid API key and the server is reachable
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports `mailbox`, `total`, `limit` (the default `50`), `offset` (the default `0`) and a `messages` array where each element reports `uid`, `seq`, `flags`, `size`, `internal_date` and an `envelope` with `from`, `to`, `cc`, `subject`, `date` and `message_id`

#### Scenario: Search filters are applied
- **WHEN** the request carries `from`, `to`, `subject`, `text`, `since`, `before`, `seen`, `flagged` or `unseen`
- **THEN** each provided filter is translated into the corresponding IMAP `SEARCH` key and the result contains only the matching messages

#### Scenario: Pagination window is honoured
- **WHEN** `limit` and `offset` are provided
- **THEN** `total` reports the full number of matching messages and `messages` contains only the window starting at `offset` with at most `limit` elements

#### Scenario: Newest messages come first
- **WHEN** several messages match
- **THEN** they are ordered by descending `UID`

#### Scenario: Unknown mailbox
- **WHEN** the named mailbox does not exist (the server answers `NO` on selection)
- **THEN** the response status is `404`
- **AND** the body is `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Invalid listing request
- **WHEN** `mailbox` is missing or blank, a date is not `YYYY-MM-DD`, a boolean is not `true`/`false`, `limit`/`offset` are not valid integers in range, or `seen` and `unseen` are supplied with contradictory values
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`

#### Scenario: Unreachable server while listing
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503`
- **AND** the body is `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated listing
- **WHEN** `GET /api/messages` is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Message retrieval
The service SHALL expose `GET /api/messages/{uid}` as a protected route that selects the named mailbox and returns a single message, reporting its metadata for `format=summary`, its raw header block for `format=headers` and its raw full RFC822 source for `format=full`, without setting the `\Seen` flag.

#### Scenario: Summary is returned
- **WHEN** `GET /api/messages/42?mailbox=INBOX` is requested without `format` and the message exists
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports `mailbox`, `uid`, `seq`, `flags`, `size`, `internal_date`, `envelope` and `format` = `"summary"`

#### Scenario: Headers are returned base64-encoded
- **WHEN** `format=headers` is requested and the message exists
- **THEN** the response status is `200`
- **AND** the body carries `format` = `"headers"` and a `headers_base64` field whose decoded bytes are the message's raw header block

#### Scenario: Full message is returned base64-encoded
- **WHEN** `format=full` is requested and the message exists
- **THEN** the response status is `200`
- **AND** the body carries `format` = `"full"` and a `raw_base64` field whose decoded bytes are the full RFC822 message

#### Scenario: Unknown message
- **WHEN** the requested `uid` does not exist in the named mailbox
- **THEN** the response status is `404`
- **AND** the body is `{"error":"message_not_found","message":"..."}`

#### Scenario: Invalid retrieval request
- **WHEN** `uid` is not a valid numeric identifier, `mailbox` is missing or blank, or `format` is not one of `summary`, `headers` or `full`
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`

#### Scenario: Unreachable server while retrieving
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503`

#### Scenario: Unauthenticated retrieval
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: IMAP command injection safety
The service SHALL reject `CR`, `LF` and `NUL` characters in any user-supplied value and SHALL render every literal interpolated into an IMAP `SEARCH` or `FETCH` command as a quoted, escaped string, so no request can inject additional IMAP commands into the authenticated session.

#### Scenario: Control characters are rejected
- **WHEN** a search value contains `CR`, `LF` or `NUL`
- **THEN** the response status is `400` with `{"error":"invalid_request","message":"..."}` and no IMAP command is sent

#### Scenario: Literals are quoted and escaped
- **WHEN** a search value contains a double quote or a backslash
- **THEN** the value is rendered as an IMAP quoted string with `"` and `\` escaped and the search still runs on a single IMAP command line

#### Scenario: Dates and identifiers are not free-form
- **WHEN** `since`/`before` and the message `uid` are supplied
- **THEN** they are validated and rendered in canonical IMAP form (`DD-Mon-YYYY` and a numeric UID set), never interpolated raw

### Requirement: Read-only-ish fetching and session reuse
The service SHALL run message operations on the existing lazily-established session, request bodies with `BODY.PEEK` so fetching never sets `\Seen`, and reconnect only when the cached session is found dead.

#### Scenario: Fetching never sets the seen flag
- **WHEN** a message is retrieved with any `format`
- **THEN** the fetch requests body data with `BODY.PEEK` and no `\Seen` flag is set on the server

#### Scenario: Operations reuse a live session
- **WHEN** several message operations run in sequence against a live session
- **THEN** only one connection is established

#### Scenario: Dead session is replaced
- **WHEN** the cached session is found dead on next use
- **THEN** the service discards it, reconnects within the bounded attempts and then runs the operation

### Requirement: Response hygiene
The service SHALL never emit the IMAP password or third-party error text in logs, debug output or HTTP responses.

#### Scenario: Stable error messages
- **WHEN** a message operation fails with a connection or protocol error
- **THEN** the HTTP body carries the stable `imap_unavailable` code and a fixed message

#### Scenario: Redacted state
- **WHEN** the application state or the connection manager is formatted with `Debug`
- **THEN** the output contains none of the configured secrets
