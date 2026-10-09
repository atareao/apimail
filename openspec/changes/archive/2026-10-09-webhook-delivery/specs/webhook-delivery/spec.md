# Spec Delta

## Purpose

Definir una cola de entrega duradera para las notificaciones del webhook: que una caída del
webhook o un reinicio del servicio no pierdan ninguna notificación, que los límites de la cola
sean explícitos y acotados, y que el estado de la cola sea observable.

## ADDED Requirements

### Requirement: Durable delivery queue

The service SHALL decouple ingestion from delivery: every new message SHALL be enqueued and a
delivery worker SHALL drain the queue in FIFO order, retrying retryable failures with capped
exponential backoff until they are delivered, so a webhook outage never loses a notification.
Delivery SHALL be at-least-once, so the same notification may be delivered more than once and the
receiving end can de-duplicate it on the `mailbox`, `uid_validity` and `uid` values of the
payload.

#### Scenario: A notification is retried until it is delivered

- **WHEN** the webhook is temporarily unavailable
- **THEN** the notification stays queued and is retried with capped backoff until it is delivered
- **AND** the subscription keeps watching the mailbox while that happens

#### Scenario: A webhook outage does not block ingestion

- **WHEN** the webhook stays unavailable while new messages keep arriving
- **THEN** every new message is enqueued and the subscription never blocks waiting on the webhook

#### Scenario: A poison notification is dropped after bounded attempts

- **WHEN** a notification fails with a non-retryable status
- **THEN** it is attempted a bounded number of times, then dropped and counted, so it never blocks
  the queue

#### Scenario: Duplicates are de-duplicable

- **WHEN** the same message is delivered more than once, for example after a crash between the
  `POST` and its acknowledgement
- **THEN** both payloads carry the same `mailbox`, `uid_validity` and `uid`, so the receiver can
  de-duplicate them

#### Scenario: A failed enqueue is not acknowledged

- **WHEN** the queue cannot be written
- **THEN** the notification is not acknowledged (the mailbox watermark does not advance),
  `last_error` reports a stable code and the subscription keeps running

### Requirement: Persistent queue

When `APIMAIL_QUEUE_PATH` is set, the service SHALL persist the pending notifications and the
mailbox watermark in that file, appended durably and reloaded at startup, and SHALL create it with
owner-only permissions; when it is not set, the queue SHALL live in memory only, so no mail
content reaches the filesystem. The queue file SHALL contain neither credentials nor third-party
error text. A queue path that cannot be created, opened or written SHALL fail startup with a
configuration error naming the variable.

#### Scenario: Undelivered notifications survive a restart

- **WHEN** the service is stopped with notifications still pending in a persistent queue and then
  started again
- **THEN** those notifications are still pending and are delivered

#### Scenario: A persistent queue is reloaded with its watermark

- **WHEN** the service starts with a persistent queue holding a watermark for the same mailbox and
  `UIDVALIDITY`
- **THEN** the subscription resumes from that watermark instead of skipping the backlog

#### Scenario: In-memory mode writes nothing

- **WHEN** no queue path is configured
- **THEN** no queue file is created and no mail content is written to the filesystem

#### Scenario: The queue file is owner-only

- **WHEN** a persistent queue is used
- **THEN** its file is created with owner-only permissions (`0600` on Unix)

#### Scenario: An unusable queue path fails closed

- **WHEN** `APIMAIL_QUEUE_PATH` is set to a path that cannot be created, opened or written
- **THEN** the service fails to start with a configuration error naming the variable

#### Scenario: The queue never stores secrets

- **WHEN** the queue file is written
- **THEN** it contains no IMAP password and no third-party error text

### Requirement: Bounded queue

The service SHALL bound the queue by item count (`APIMAIL_QUEUE_MAX_ITEMS`, default 1000) and by
live bytes (`APIMAIL_QUEUE_MAX_BYTES`, default 64 MiB), SHALL validate both values at startup, and
SHALL drop the oldest pending notification, counting it, when enqueuing would exceed a limit.

#### Scenario: Overflow drops the oldest notification

- **WHEN** enqueuing a notification would exceed `APIMAIL_QUEUE_MAX_ITEMS` or
  `APIMAIL_QUEUE_MAX_BYTES`
- **THEN** the oldest pending notification is dropped and the dropped counter is incremented

#### Scenario: Queue limits are validated at startup

- **WHEN** a queue limit variable is present but not a positive integer
- **THEN** the service fails to start with a configuration error naming the variable

### Requirement: Queue observability

The service SHALL expose a protected `GET /api/idle/queue` route that reports whether the queue is
persistent, the pending, delivered, dropped and failed counters and the age of the oldest pending
notification.

#### Scenario: The queue state is reported

- **WHEN** `GET /api/idle/queue` is requested with a valid API key
- **THEN** the response status is `200`
- **AND** the body reports `persistent`, `pending`, `delivered`, `dropped`, `failed` and
  `oldest_pending_secs`

#### Scenario: In-memory mode is visible

- **WHEN** the queue is in memory because no path is configured
- **THEN** the reported `persistent` is `false`

#### Scenario: Unauthenticated queue inspection

- **WHEN** `GET /api/idle/queue` is requested without a valid API key
- **THEN** the response status is `401`
