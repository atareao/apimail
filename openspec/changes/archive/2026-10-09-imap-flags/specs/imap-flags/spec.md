# Spec Delta

## Purpose
Actualizar las banderas, mover, copiar y borrar un único mensaje de un buzón de la cuenta,
componiendo `UID STORE`, `UID COPY`, `UID MOVE` y `UID EXPUNGE` sobre la sesión IMAP perezosa
de `imap-connection`, con una allowlist estricta de banderas y sin permitir inyección de
comandos IMAP.

## ADDED Requirements

### Requirement: Message flag update
The service SHALL expose `PATCH /api/messages/{uid}/flags` as a protected route that selects the named mailbox and applies the requested flag additions and removals to the single message identified by `uid` with `UID STORE`, returning the message's resulting flags.

#### Scenario: Flags are added
- **WHEN** `PATCH /api/messages/42/flags?mailbox=INBOX` is requested with a valid API key, the message exists and the body is `{"add":["\\Seen","\\Flagged"]}`
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports `mailbox`, `uid` and the resulting `flags` array
- **AND** a `UID STORE` carrying `+FLAGS.SILENT (\Seen \Flagged)` is sent

#### Scenario: Flags are removed
- **WHEN** the same route is requested with the body `{"remove":["\\Deleted"]}` and the message exists
- **THEN** the response status is `200`
- **AND** a `UID STORE` carrying `-FLAGS.SILENT (\Deleted)` is sent
- **AND** the body reports the resulting `flags`

#### Scenario: Flags are canonicalised case-insensitively
- **WHEN** the body carries a flag in a different case (for example `\seen` or `\flagged`)
- **THEN** it is accepted and canonicalised to its standard form (`\Seen`, `\Flagged`) before the command is built

#### Scenario: Additions and removals are combined
- **WHEN** the body carries both `add` and `remove` with disjoint flags
- **THEN** a single `UID STORE` applying the additions and removals is sent and the resulting flags are returned

#### Scenario: Flag outside the allowlist
- **WHEN** the body carries a flag other than `\Seen`, `\Answered`, `\Flagged`, `\Draft` or `\Deleted`
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`
- **AND** no IMAP command is sent

#### Scenario: Empty or contradictory flag request
- **WHEN** neither `add` nor `remove` is present or both are empty, or the same flag appears in both `add` and `remove`
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`
- **AND** no IMAP command is sent

#### Scenario: Unknown message while updating flags
- **WHEN** the requested `uid` does not exist in the named mailbox
- **THEN** the response status is `404`
- **AND** the body is `{"error":"message_not_found","message":"..."}`

#### Scenario: Unknown mailbox while updating flags
- **WHEN** the named mailbox does not exist (the server answers `NO` on selection)
- **THEN** the response status is `404`
- **AND** the body is `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Invalid flag update request
- **WHEN** `uid` is not a valid positive numeric identifier, `mailbox` is missing, blank or contains control characters, or the body is not valid JSON
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`

#### Scenario: Unreachable server while updating flags
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503`
- **AND** the body is `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated flag update
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Message move
The service SHALL expose `POST /api/messages/{uid}/move` as a protected route that selects the named mailbox and moves the single message identified by `uid` to the destination mailbox, using `UID MOVE` when the server announces `MOVE`, emulating it with `UID COPY` plus `UID STORE +FLAGS.SILENT (\Deleted)` plus `UID EXPUNGE` when it announces `UIDPLUS` instead, and reporting `capability_not_supported` when it announces neither.

#### Scenario: Message is moved with the MOVE extension
- **WHEN** `POST /api/messages/42/move?mailbox=INBOX` is requested with a valid API key, the body is `{"to":"Archive"}`, the message exists and the server announces `MOVE`
- **THEN** the response status is `200`
- **AND** the body is `{"mailbox":"INBOX","uid":42,"to":"Archive","status":"moved"}`
- **AND** a `UID MOVE` to `Archive` is sent

#### Scenario: Move is emulated without MOVE but with UIDPLUS
- **WHEN** the server does not announce `MOVE` but announces `UIDPLUS`
- **THEN** `UID COPY` to `Archive`, then `UID STORE +FLAGS.SILENT (\Deleted)`, then `UID EXPUNGE` are sent in that order
- **AND** the response status is `200` with `"status":"moved"`

#### Scenario: Neither MOVE nor UIDPLUS is supported
- **WHEN** the server announces neither `MOVE` nor `UIDPLUS`
- **THEN** the response status is `501`
- **AND** the body is `{"error":"capability_not_supported","message":"..."}`
- **AND** no move, copy, store or expunge command is sent

#### Scenario: Invalid move request
- **WHEN** `uid` is not a valid positive numeric identifier, `mailbox` is missing, blank or contains control characters, `to` is missing, blank or contains `CR`, `LF` or `NUL`, or the body is not valid JSON
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`
- **AND** no IMAP command is sent

#### Scenario: Unknown message while moving
- **WHEN** the requested `uid` does not exist in the named mailbox
- **THEN** the response status is `404` with `{"error":"message_not_found","message":"..."}`

#### Scenario: Unknown mailbox while moving
- **WHEN** the named mailbox does not exist (the server answers `NO` on selection)
- **THEN** the response status is `404` with `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Unreachable server while moving
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503` with `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated move
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Message copy
The service SHALL expose `POST /api/messages/{uid}/copy` as a protected route that selects the named mailbox and copies the single message identified by `uid` to the destination mailbox with `UID COPY`, requiring no additional capability.

#### Scenario: Message is copied
- **WHEN** `POST /api/messages/42/copy?mailbox=INBOX` is requested with a valid API key, the body is `{"to":"Archive"}` and the message exists
- **THEN** the response status is `200`
- **AND** the body is `{"mailbox":"INBOX","uid":42,"to":"Archive","status":"copied"}`
- **AND** a `UID COPY` to `Archive` is sent

#### Scenario: Invalid copy request
- **WHEN** `uid` is not a valid positive numeric identifier, `mailbox` is missing, blank or contains control characters, `to` is missing, blank or contains `CR`, `LF` or `NUL`, or the body is not valid JSON
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`
- **AND** no IMAP command is sent

#### Scenario: Unknown message while copying
- **WHEN** the requested `uid` does not exist in the named mailbox
- **THEN** the response status is `404` with `{"error":"message_not_found","message":"..."}`

#### Scenario: Unknown mailbox while copying
- **WHEN** the named mailbox does not exist (the server answers `NO` on selection)
- **THEN** the response status is `404` with `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Unreachable server while copying
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503` with `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated copy
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Message deletion
The service SHALL expose `DELETE /api/messages/{uid}` as a protected route that selects the named mailbox, marks the single message identified by `uid` as `\Deleted` with `UID STORE` and purges only that message with `UID EXPUNGE`, requiring the server to announce `UIDPLUS` and never falling back to a global `EXPUNGE`.

#### Scenario: Message is deleted
- **WHEN** `DELETE /api/messages/42?mailbox=INBOX` is requested with a valid API key, the message exists and the server announces `UIDPLUS`
- **THEN** the response status is `200`
- **AND** the body is `{"mailbox":"INBOX","uid":42,"status":"deleted"}`
- **AND** a `UID STORE` marking `\Deleted` followed by `UID EXPUNGE` for that `uid` are sent

#### Scenario: Deletion requires UIDPLUS
- **WHEN** the server does not announce `UIDPLUS`
- **THEN** the response status is `501`
- **AND** the body is `{"error":"capability_not_supported","message":"..."}`
- **AND** no `EXPUNGE` command (global or targeted) is sent

#### Scenario: Invalid delete request
- **WHEN** `uid` is not a valid positive numeric identifier, `mailbox` is missing, blank or contains control characters
- **THEN** the response status is `400`
- **AND** the body is `{"error":"invalid_request","message":"..."}`
- **AND** no IMAP command is sent

#### Scenario: Unknown message while deleting
- **WHEN** the requested `uid` does not exist in the named mailbox
- **THEN** the response status is `404` with `{"error":"message_not_found","message":"..."}`

#### Scenario: Unknown mailbox while deleting
- **WHEN** the named mailbox does not exist (the server answers `NO` on selection)
- **THEN** the response status is `404` with `{"error":"mailbox_not_found","message":"..."}`

#### Scenario: Unreachable server while deleting
- **WHEN** the server cannot be reached or the session fails with a connection error
- **THEN** the response status is `503` with `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated deletion
- **WHEN** the route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: IMAP command injection safety
The service SHALL constrain every literal interpolated into a `UID STORE` or `UID EXPUNGE` command to the flag allowlist and fixed items, derive the UID set from a numeric `u32`, and reject `CR`, `LF` and `NUL` in any user-supplied value, so no request can inject additional IMAP commands into the authenticated session.

#### Scenario: Control characters are rejected
- **WHEN** a flag, mailbox or destination value contains `CR`, `LF` or `NUL`
- **THEN** the response status is `400` with `{"error":"invalid_request","message":"..."}` and no IMAP command is sent

#### Scenario: The STORE query is allowlisted
- **WHEN** flags are supplied
- **THEN** the `UID STORE` query contains only the fixed items (`+FLAGS`/`-FLAGS`, with `.SILENT`) and flags from the allowlist, and never free-form user text

#### Scenario: The UID is numeric
- **WHEN** a `uid` is supplied
- **THEN** it is parsed as a positive `u32` and rendered as a numeric UID set, never interpolated raw

#### Scenario: The destination mailbox is validated
- **WHEN** `to` is supplied for a copy or a move
- **THEN** it is checked at the HTTP edge for `CR`/`LF`/`NUL` and then passed through the crate's mailbox validation, never interpolated raw

### Requirement: Session reuse and response hygiene
The service SHALL run flag operations on the existing lazily-established session, reconnecting only when the cached session is found dead, and SHALL never emit the IMAP password or third-party error text in logs, debug output or HTTP responses.

#### Scenario: Operations reuse a live session
- **WHEN** several flag operations run in sequence against a live session
- **THEN** only one connection is established

#### Scenario: Dead session is replaced
- **WHEN** the cached session is found dead on next use
- **THEN** the service discards it, reconnects within the bounded attempts and then runs the operation

#### Scenario: Stable error messages
- **WHEN** a flag operation fails with a connection or protocol error
- **THEN** the HTTP body carries the stable `imap_unavailable` code and a fixed message

#### Scenario: Redacted state
- **WHEN** the application state or the connection manager is formatted with `Debug`
- **THEN** the output contains none of the configured secrets
