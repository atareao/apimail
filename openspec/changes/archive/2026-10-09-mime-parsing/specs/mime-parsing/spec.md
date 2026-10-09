# Spec Delta

## Purpose
Interpretar el mensaje MIME crudo de un buzón de la cuenta — su texto plano, su HTML y sus
adjuntos (metadatos y contenido) — reutilizando la sesión IMAP perezosa de `imap-connection`
y sin escribir ningún dato del servidor a disco.

## ADDED Requirements

### Requirement: Parsed message body
The service SHALL expose `GET /api/messages/{uid}/body` as a protected route that selects the named mailbox, fetches the single message identified by `uid`, parses its MIME content and returns its plain-text and HTML bodies as JSON.

#### Scenario: Parsed bodies are returned
- **WHEN** `GET /api/messages/42/body?mailbox=INBOX` is requested with a valid API key, the message exists and can be parsed
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports `mailbox`, `uid`, `text` and `html`

#### Scenario: A missing alternative is filled in
- **WHEN** the message carries only one of a text or an HTML part
- **THEN** the missing alternative is derived from the available one following RFC 8621 §4.1.4, and both `text` and `html` are reported

#### Scenario: A message without a displayable body
- **WHEN** the message has no text or HTML part (for example only attachments)
- **THEN** the response status is `200`
- **AND** `text` and `html` are `null`

#### Scenario: HTML is returned verbatim
- **WHEN** the message carries an HTML part
- **THEN** `html` carries the decoded HTML unchanged and rendering it safely is the client's responsibility

#### Scenario: Unknown message while parsing the body
- **WHEN** the requested `uid` does not exist in the named mailbox
- **THEN** the response status is `404` with `{"error":"message_not_found","message":"..."}`

#### Scenario: Unknown mailbox while parsing the body
- **WHEN** the named mailbox does not exist (the server answers `NO` on selection)
- **THEN** the response status is `404` with `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Invalid body request
- **WHEN** `uid` is not a valid positive numeric identifier, or `mailbox` is missing, blank or contains control characters
- **THEN** the response status is `400` with `{"error":"invalid_request","message":"..."}`
- **AND** no IMAP command is sent

#### Scenario: Message too large to parse
- **WHEN** the message size exceeds the configured `APIMAIL_MAX_MESSAGE_BYTES` limit
- **THEN** the response status is `413` with `{"error":"message_too_large","message":"..."}`

#### Scenario: Message cannot be parsed
- **WHEN** the message exists but its MIME content cannot be parsed
- **THEN** the response status is `422` with `{"error":"message_not_parsable","message":"..."}`

#### Scenario: Unreachable server while parsing the body
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503` with `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated body request
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Attachment listing
The service SHALL expose `GET /api/messages/{uid}/attachments` as a protected route that selects the named mailbox, parses the message identified by `uid` and returns the metadata of each attachment, each identified by a stable positional `id`, without their content.

#### Scenario: Attachments are listed
- **WHEN** `GET /api/messages/42/attachments?mailbox=INBOX` is requested with a valid API key and the message exists
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports `mailbox`, `uid` and an `attachments` array where each element reports `id`, `filename`, `content_type`, `size`, `inline` and `content_id`

#### Scenario: Positional identifiers are stable
- **WHEN** the same message is listed twice
- **THEN** each attachment keeps the same `id`, assigned from its position in the parsed attachment list

#### Scenario: A message without attachments
- **WHEN** the message has no attachment
- **THEN** the response status is `200` and `attachments` is an empty array

#### Scenario: Unknown message while listing attachments
- **WHEN** the requested `uid` does not exist in the named mailbox
- **THEN** the response status is `404` with `{"error":"message_not_found","message":"..."}`

#### Scenario: Unknown mailbox while listing attachments
- **WHEN** the named mailbox does not exist (the server answers `NO` on selection)
- **THEN** the response status is `404` with `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Invalid attachment list request
- **WHEN** `uid` is not a valid positive numeric identifier, or `mailbox` is missing, blank or contains control characters
- **THEN** the response status is `400` with `{"error":"invalid_request","message":"..."}`
- **AND** no IMAP command is sent

#### Scenario: Oversized or unparsable message while listing attachments
- **WHEN** the message exceeds the configured size limit or cannot be parsed
- **THEN** the response status is `413` with `message_too_large`, or `422` with `message_not_parsable`, respectively

#### Scenario: Unreachable server while listing attachments
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503` with `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated attachment listing
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Attachment retrieval
The service SHALL expose `GET /api/messages/{uid}/attachments/{id}` as a protected route that selects the named mailbox, parses the message identified by `uid` and returns the decoded content of the attachment identified by the positional `id`, base64-encoded inside the JSON body.

#### Scenario: An attachment is returned
- **WHEN** `GET /api/messages/42/attachments/0?mailbox=INBOX` is requested with a valid API key, the message exists and the attachment exists
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports `mailbox`, `uid`, `id`, `filename`, `content_type`, `size` and a `content_base64` field whose decoded bytes are the attachment's decoded content

#### Scenario: Unknown attachment
- **WHEN** the requested `id` does not match any attachment of the message
- **THEN** the response status is `404` with `{"error":"attachment_not_found","message":"..."}`

#### Scenario: Invalid attachment request
- **WHEN** `uid` is not a positive numeric identifier (`> 0`), or `id` is not a non-negative numeric identifier (`>= 0`, the 0-based positional index), or `mailbox` is missing, blank or contains control characters
- **THEN** the response status is `400` with `{"error":"invalid_request","message":"..."}`
- **AND** no IMAP command is sent

#### Scenario: Unknown message while retrieving an attachment
- **WHEN** the requested `uid` does not exist in the named mailbox
- **THEN** the response status is `404` with `{"error":"message_not_found","message":"..."}`

#### Scenario: Unknown mailbox while retrieving an attachment
- **WHEN** the named mailbox does not exist (the server answers `NO` on selection)
- **THEN** the response status is `404` with `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Oversized or unparsable message while retrieving an attachment
- **WHEN** the message exceeds the configured size limit or cannot be parsed
- **THEN** the response status is `413` with `message_too_large`, or `422` with `message_not_parsable`, respectively

#### Scenario: Unreachable server while retrieving an attachment
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503` with `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated attachment retrieval
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Bounded, side-effect-free parsing of untrusted content
The service SHALL bound the size of the message it parses with the configurable `APIMAIL_MAX_MESSAGE_BYTES` limit, SHALL derive the fetched UID from the numeric identifier validated at the HTTP edge so no MIME content can inject an IMAP command, SHALL return attachment content only inside the JSON body and never as an HTTP header, and SHALL never write server-supplied MIME data to the filesystem or to logs.

#### Scenario: Oversized message is rejected before parsing
- **WHEN** the reported or fetched message size exceeds `APIMAIL_MAX_MESSAGE_BYTES`
- **THEN** the response status is `413` with `{"error":"message_too_large","message":"..."}` and the MIME content is not parsed

#### Scenario: Only a numeric UID reaches the IMAP command
- **WHEN** a message is fetched for parsing
- **THEN** the only IMAP command is a `UID FETCH` over the numeric `uid` parsed at the HTTP edge, and `mailbox` is control-character-checked before the crate's mailbox validation

#### Scenario: No data reaches response headers
- **WHEN** an attachment with a hostile `filename` or `content_type` is retrieved
- **THEN** both values appear only as JSON fields and are never echoed into an HTTP header

#### Scenario: Nothing is written to disk
- **WHEN** a message is parsed
- **THEN** no server-supplied content is written to the filesystem

#### Scenario: The size limit is configured safely
- **WHEN** `APIMAIL_MAX_MESSAGE_BYTES` is absent
- **THEN** the documented default (`26214400`, 25 MiB) is used, and when it is present but not a positive integer the service fails to start with a configuration error naming the variable

### Requirement: Session reuse and response hygiene
The service SHALL run message parsing on the existing lazily-established session, reconnecting only when the cached session is found dead, and SHALL never emit the IMAP password or third-party error text in logs, debug output or HTTP responses.

#### Scenario: Operations reuse a live session
- **WHEN** several parsing operations run in sequence against a live session
- **THEN** only one connection is established

#### Scenario: Dead session is replaced
- **WHEN** the cached session is found dead on next use
- **THEN** the service discards it, reconnects within the bounded attempts and then runs the operation

#### Scenario: Stable error messages
- **WHEN** a parsing operation fails with a connection or protocol error
- **THEN** the HTTP body carries the stable `imap_unavailable` code and a fixed message

#### Scenario: Redacted state
- **WHEN** the application state or the connection manager is formatted with `Debug`
- **THEN** the output contains none of the configured secrets
