# Spec Delta

## Purpose

Ajustar `imap-idle` para que las notificaciones dejen de entregarse en línea y pasen por la cola
duradera de `webhook-delivery`, y para que la suscripción reanude su punto de arranque desde el
watermark guardado cuando la cola es persistente.

## MODIFIED Requirements

### Requirement: New-message notification

While subscribed, the service SHALL notify the configured webhook, through the delivery queue,
about each message whose UID is greater than its starting point, in ascending UID order,
containing the message metadata and, when the message is within the size limit and can be parsed,
its plain-text and HTML bodies and its attachment metadata. The starting point SHALL be the
highest UID present when the subscription first started, or, when a persistent queue holds a
watermark for the same mailbox and `UIDVALIDITY`, that stored watermark, so that messages which
arrived while the service was down are still notified.

#### Scenario: A new message is notified

- **WHEN** a message with a UID higher than the highest UID present at subscription start arrives
- **THEN** the service sends a `POST` to the configured webhook whose JSON body reports `mailbox`, `uid`, `uid_validity`, `flags`, `size`, `internal_date`, `envelope`, `parsed`, `text`, `html` and `attachments`

#### Scenario: Messages present before start are not notified

- **WHEN** the subscription starts with no stored watermark on a mailbox that already contains messages
- **THEN** none of those existing messages is notified

#### Scenario: A restart resumes from the stored watermark

- **WHEN** the subscription starts again with a persistent queue holding a watermark for the same mailbox and `UIDVALIDITY`
- **THEN** the messages that arrived while the service was down are notified, in ascending UID order

#### Scenario: A changed UIDVALIDITY resets the starting point

- **WHEN** the mailbox reports a `UIDVALIDITY` different from the stored watermark's
- **THEN** the starting point is re-derived from the new mailbox status and no message is notified from the stale UIDs

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

### Requirement: Bounded, side-effect-free handling of untrusted content

The service SHALL bound the size of each notified message with the configured `APIMAIL_MAX_MESSAGE_BYTES` limit, SHALL contact the webhook only over `http` or `https`, SHALL validate the webhook URL at startup, and SHALL never emit credentials or third-party error text in the payload, the logs, `last_error` or HTTP responses. Mail content SHALL never be written to disk, except in the durable queue explicitly configured through `APIMAIL_QUEUE_PATH`, which SHALL be the only file written, SHALL be created with owner-only permissions and SHALL contain neither credentials nor third-party error text.

#### Scenario: The webhook URL is validated at startup

- **WHEN** `APIMAIL_WEBHOOK_URL` is set to a value that is not an absolute `http`/`https` URL
- **THEN** the service fails to start with a configuration error naming the variable

#### Scenario: No secrets or third-party text are exposed

- **WHEN** a notification or a failure is emitted
- **THEN** the payload, the logs, `last_error` and the HTTP responses contain neither the IMAP password nor third-party error text

#### Scenario: last_error is a stable code

- **WHEN** the subscription records a failure
- **THEN** `last_error` is one of the documented stable codes (`"imap_unavailable"`, `"webhook_failed"` or `"queue_unavailable"`), never a raw error rendering

#### Scenario: Nothing is written to disk

- **WHEN** a message is notified and no queue path is configured
- **THEN** no mail content is written to the filesystem

#### Scenario: Only the configured queue file is written

- **WHEN** a persistent queue is configured
- **THEN** mail content is written only to that file, with owner-only permissions
