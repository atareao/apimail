# Spec Delta

## MODIFIED Requirements

### Requirement: Mailbox listing
The service SHALL expose `GET /api/mailboxes` as a protected route that lists the account's mailboxes, reporting for each one its name, its hierarchy delimiter and its attributes, without exposing any credential. A listing run over a connection whose peer has closed it SHALL fail as a connection error and SHALL never be reported as `200` with an empty list.

#### Scenario: Mailboxes are listed
- **WHEN** `GET /api/mailboxes` is requested with a valid API key and the server is reachable
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body is `{"mailboxes":[{"name":"...","delimiter":"/","attributes":["\\HasNoChildren"]}]}`, where `delimiter` is `null` when the server reports no hierarchy

#### Scenario: Listing mailboxes over a connection closed by the peer
- **WHEN** the cached session's peer has already closed the connection and `GET /api/mailboxes` is requested with a valid API key
- **THEN** the response status is `503`
- **AND** the body is `{"error":"imap_unavailable","message":"..."}`
- **AND** the response is never `200` with `{"mailboxes":[]}`

#### Scenario: Unreachable server
- **WHEN** `GET /api/mailboxes` is requested with a valid API key and the server cannot be reached
- **THEN** the response status is `503`
- **AND** the body is `{"error":"imap_unavailable","message":"..."}`

#### Scenario: Unauthenticated listing
- **WHEN** `GET /api/mailboxes` is requested without a valid API key
- **THEN** the response status is `401`
