# Spec Delta

## Purpose

Envía correo saliente a través del SMTP configurado. Expone `POST /api/messages`
(protegido), acepta un mensaje en JSON con adjuntos en base64, respeta el modo TLS
de `mail-account`, acota el tamaño de los adjuntos y traduce los fallos del
servidor SMTP a respuestas HTTP claras.

## ADDED Requirements

### Requirement: Send endpoint
The service SHALL expose `POST /api/messages` as a protected route that accepts an `application/json` body describing an outgoing message and, on success, responds with HTTP `200` and a JSON body reporting the message as sent.

#### Scenario: Successful send
- **WHEN** `POST /api/messages` is requested with a valid API key and a well-formed message
- **THEN** the response status is `200`
- **AND** the `Content-Type` is `application/json`
- **AND** the body reports `"status":"sent"`

#### Scenario: Unauthenticated send
- **WHEN** `POST /api/messages` is requested without a valid API key
- **THEN** the response status is `401`

### Requirement: Message fields
The service SHALL accept a message with `to` (one or more addresses, required) plus the optional `cc`, `bcc`, `subject`, `text`, `html`, `from` and `attachments` fields, and SHALL default the sender to the configured SMTP user when `from` is absent.

#### Scenario: Minimal message
- **WHEN** a message provides only `to` and `subject`
- **THEN** the send succeeds and the sender is the configured SMTP user

#### Scenario: Explicit sender
- **WHEN** a message provides a `from` address
- **THEN** that address is used as the sender instead of the SMTP user

#### Scenario: Missing recipients
- **WHEN** a message provides no `to` address
- **THEN** the response status is `400`

#### Scenario: Invalid address
- **WHEN** a message contains an address that is not a valid mailbox
- **THEN** the response status is `400`

#### Scenario: Malformed JSON body
- **WHEN** the request body is not valid JSON for the message schema
- **THEN** the response status is `400`

### Requirement: Message body and attachments
The service SHALL support a plain-text body, an HTML body or both, and SHALL accept attachments provided as base64-encoded content with a filename and an optional content type.

#### Scenario: Text and HTML alternative
- **WHEN** a message provides both `text` and `html`
- **THEN** the send succeeds

#### Scenario: Attachment decoded from base64
- **WHEN** a message provides an attachment whose `data_base64` is valid base64
- **THEN** the send succeeds with the decoded bytes attached under the given filename

#### Scenario: Invalid base64 attachment
- **WHEN** an attachment `data_base64` is not valid base64
- **THEN** the response status is `400`

### Requirement: Attachment size limit
The service SHALL read a maximum total attachment size in bytes from the `APIMAIL_MAX_ATTACHMENT_BYTES` environment variable, defaulting to 10 MiB, SHALL refuse to start when the value is present but not a positive integer, and SHALL reject a message whose total decoded attachment size exceeds the limit.

#### Scenario: Default limit
- **WHEN** `APIMAIL_MAX_ATTACHMENT_BYTES` is unset
- **THEN** the limit is 10 MiB

#### Scenario: Configured limit
- **WHEN** `APIMAIL_MAX_ATTACHMENT_BYTES` is set to a positive integer
- **THEN** that value is used as the limit

#### Scenario: Invalid limit
- **WHEN** `APIMAIL_MAX_ATTACHMENT_BYTES` is set to `0` or a non-numeric value
- **THEN** startup fails with a non-zero exit and a descriptive error

#### Scenario: Attachment limit exceeded
- **WHEN** the total decoded attachment size is greater than the configured limit
- **THEN** the response status is `413`

### Requirement: SMTP delivery and TLS
The service SHALL deliver the message through the SMTP endpoint configured in `mail-account`, honouring its TLS mode (`implicit`, `starttls` or `none`), and SHALL answer with HTTP `502` when the SMTP server rejects or fails the delivery.

#### Scenario: Delivery failure
- **WHEN** the SMTP server fails or rejects the message
- **THEN** the response status is `502`
- **AND** the body contains an error message

### Requirement: SMTP secret protection
The service SHALL never emit the SMTP password in logs, debug output or HTTP responses.

#### Scenario: Redacted state
- **WHEN** the application state is formatted with `Debug`
- **THEN** the output contains none of the configured secrets
