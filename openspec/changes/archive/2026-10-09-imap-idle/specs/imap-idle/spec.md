# Spec Delta

## Purpose
Suscribirse al buzón con la extensión `IDLE` sobre una conexión IMAP dedicada y notificar a un
webhook configurable cada correo nuevo, con sus metadatos y su contenido MIME parseado
(texto, HTML y metadatos de adjuntos), sin interferir con el resto de rutas de la API.

## ADDED Requirements

### Requirement: IDLE subscription control
The service SHALL expose protected routes to start, stop and inspect the IDLE subscription, which watches a single configured mailbox on a dedicated IMAP connection and notifies a configured webhook about newly arrived messages.

#### Scenario: Subscription is started
- **WHEN** `POST /api/idle/start` is requested with a valid API key and a webhook URL is configured
- **THEN** the response status is `200`
- **AND** the body reports `status` = `"running"` and the watched `mailbox`
- **AND** a dedicated IMAP connection is established for the subscription

#### Scenario: Start is idempotent
- **WHEN** `POST /api/idle/start` is requested while the subscription is already running
- **THEN** the response status is `200` with the same body and no second connection is opened

#### Scenario: Subscription is stopped
- **WHEN** `POST /api/idle/stop` is requested with a valid API key
- **THEN** the response status is `200` with `status` = `"stopped"`
- **AND** the subscription stops and its dedicated connection is closed within a bounded time (a `DONE` is sent on the normal IDLE timeout path)

#### Scenario: Stop is idempotent
- **WHEN** `POST /api/idle/stop` is requested while the subscription is not running
- **THEN** the response status is `200` with `status` = `"stopped"`

#### Scenario: Status is reported
- **WHEN** `GET /api/idle/status` is requested with a valid API key
- **THEN** the response status is `200`
- **AND** the body reports `status` (`"running"` or `"stopped"`), the watched `mailbox` and `last_error` (`null` or a stable code)

#### Scenario: Subscription cannot start without a webhook
- **WHEN** `POST /api/idle/start` is requested and no webhook URL is configured
- **THEN** the response status is `501`
- **AND** the body is `{"error":"idle_not_configured","message":"..."}`

#### Scenario: Unauthenticated IDLE control
- **WHEN** any IDLE route is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: New-message notification
While subscribed, the service SHALL POST a JSON payload to the configured webhook for each message whose UID is greater than the highest UID present when the subscription started, in ascending UID order, containing the message metadata and, when the message is within the size limit and can be parsed, its plain-text and HTML bodies and its attachment metadata.

#### Scenario: A new message is notified
- **WHEN** a message with a UID higher than the highest UID present at subscription start arrives
- **THEN** the service sends a `POST` to the configured webhook whose JSON body reports `mailbox`, `uid`, `uid_validity`, `flags`, `size`, `internal_date`, `envelope`, `parsed`, `text`, `html` and `attachments`

#### Scenario: Messages present before start are not notified
- **WHEN** the subscription starts on a mailbox that already contains messages
- **THEN** none of those existing messages is notified

#### Scenario: Several new messages are notified in order
- **WHEN** several messages with UIDs higher than the starting point arrive
- **THEN** one `POST` is sent per message in ascending UID order

#### Scenario: Attachments are reported without content
- **WHEN** a notified message carries attachments
- **THEN** the payload lists each attachment with `id`, `filename`, `content_type`, `size`, `inline` and `content_id` and never carries their bytes

#### Scenario: Oversized message is notified with metadata only
- **WHEN** a new message exceeds the configured `APIMAIL_MAX_MESSAGE_BYTES` limit
- **THEN** the payload reports `parsed` = `false` with `text` and `html` `null` and an empty `attachments` array, and still reports the message metadata

#### Scenario: Unparsable message is notified with metadata only
- **WHEN** a new message cannot be parsed
- **THEN** the payload reports `parsed` = `false` with `text` and `html` `null` and an empty `attachments` array

### Requirement: Dedicated connection and resilience
The subscription SHALL use its own IMAP connection, separate from the session used by the other routes, so it never blocks them; SHALL re-issue IDLE at least every 29 minutes; SHALL reconnect with backoff when the connection fails; and SHALL keep running until stopped.

#### Scenario: The subscription does not block the other routes
- **WHEN** the subscription is running and another route is requested
- **THEN** the other route runs on the shared session and is not blocked by the idling connection

#### Scenario: IDLE is re-issued periodically
- **WHEN** the bounded IDLE wait elapses without a change
- **THEN** the subscription ends IDLE (`DONE`) and issues it again

#### Scenario: A dropped connection is replaced
- **WHEN** the dedicated connection fails
- **THEN** the subscription reconnects with backoff and resumes watching the mailbox

#### Scenario: A webhook failure does not stop the subscription
- **WHEN** the webhook cannot be reached and the bounded retries are exhausted
- **THEN** `last_error` is set to a stable code, no third-party error text is exposed and the subscription keeps running

### Requirement: Bounded, side-effect-free handling of untrusted content
The service SHALL bound the size of each notified message with the configured `APIMAIL_MAX_MESSAGE_BYTES` limit, SHALL contact the webhook only over `http` or `https`, SHALL validate the webhook URL at startup, and SHALL never emit credentials or third-party error text in the payload, the logs, `last_error` or HTTP responses, nor write mail content to disk.

#### Scenario: The webhook URL is validated at startup
- **WHEN** `APIMAIL_WEBHOOK_URL` is set to a value that is not an absolute `http`/`https` URL
- **THEN** the service fails to start with a configuration error naming the variable

#### Scenario: No secrets or third-party text are exposed
- **WHEN** a notification or a failure is emitted
- **THEN** the payload, the logs, `last_error` and the HTTP responses contain neither the IMAP password nor third-party error text

#### Scenario: last_error is a stable code
- **WHEN** the subscription records a failure
- **THEN** `last_error` is one of the documented stable codes (`"imap_unavailable"` or `"webhook_failed"`), never a raw error rendering

#### Scenario: Nothing is written to disk
- **WHEN** a message is notified
- **THEN** no mail content is written to the filesystem
