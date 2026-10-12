# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

### Requirement: Incomplete command responses
The service SHALL treat an IMAP command whose response stream ends before the command's tagged reply as a connection failure, and SHALL NOT surface it as a valid empty or default result.

#### Scenario: A truncated command is a connection failure
- **WHEN** a command's response stream ends without its tagged reply because the peer closed the connection
- **THEN** the operation fails with a connection error
- **AND** it never yields an empty result, a default mailbox, zero UIDs or an empty mailbox list
