# Spec Delta

## MODIFIED Requirements

### Requirement: Message listing and search
The service SHALL expose `GET /api/messages` as a protected route that selects the named mailbox and returns the messages matching the given search criteria, paginated and ordered by descending `UID`, without setting the `\Seen` flag. A listing run over a connection whose peer has closed it SHALL fail as a connection error and SHALL never be reported as a valid empty page.

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

#### Scenario: An empty mailbox still returns an empty page
- **WHEN** the mailbox exists and the search genuinely matches no message
- **THEN** the response status is `200` with `total` = `0` and an empty `messages` array

#### Scenario: Listing over a connection closed by the peer
- **WHEN** the cached session's peer has already closed the connection and `GET /api/messages?mailbox=INBOX&unseen=true&limit=50` is requested
- **THEN** the response status is `503`
- **AND** the body is `{"error":"imap_unavailable","message":"..."}`
- **AND** the response is never a `200` page reporting `total` = `0`

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

### Requirement: Read-only-ish fetching and session reuse
The service SHALL run message operations on the existing lazily-established session, request bodies with `BODY.PEEK` so fetching never sets `\Seen`, and reconnect only when the cached session is found dead.

#### Scenario: Fetching never sets the seen flag
- **WHEN** a message is retrieved with any `format`
- **THEN** the fetch requests body data with `BODY.PEEK` and no `\Seen` flag is set on the server

#### Scenario: Operations reuse a live session
- **WHEN** several message operations run in sequence against a live session
- **THEN** only one connection is established

#### Scenario: Dead session is replaced
- **WHEN** the cached session is found dead on next use, because its liveness probe observed the peer's already-closed connection
- **THEN** the service discards it, reconnects within the bounded attempts and then runs the operation
- **AND** the operation never runs against the dead session
