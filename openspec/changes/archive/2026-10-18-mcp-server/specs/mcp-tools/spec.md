# Spec Delta

## Purpose

Definir la superficie de 17 tools MCP que expone las capacidades de correo ya existentes,
delegando en la misma capa de servicio que la API HTTP para que una operación y su tool nunca
divergan, con esquemas derivados de los mismos tipos y errores de dominio legibles y sin secretos.

## ADDED Requirements

### Requirement: The tool surface mirrors the existing HTTP capabilities

The service SHALL expose exactly one MCP tool per HTTP operation except the deliberately excluded
administrative operations (`GET /api/health`, `POST /api/idle/start`, `POST /api/idle/stop`), so the
surface is twelve read tools and five write tools that delegate to the same service layer the HTTP
handlers use, with no duplicated business logic. `select_mailbox` counts among the **read** tools
because what it returns is mailbox status and it mutates no message, but it has a **state effect on
the shared IMAP session** (it changes the currently selected mailbox), and that effect SHALL be
documented on the tool. Only the five write tools mutate mail.

#### Scenario: Read tools available
- **WHEN** an MCP client lists the available tools
- **THEN** it sees `whoami`, `get_account`, `imap_status`, `list_mailboxes`, `select_mailbox`, `list_messages`, `get_message`, `get_message_body`, `list_attachments`, `get_attachment`, `idle_status` and `idle_queue`

#### Scenario: Write tools available
- **WHEN** an MCP client lists the available tools
- **THEN** it sees `send_message`, `update_flags`, `move_message`, `copy_message` and `delete_message`

#### Scenario: Administrative operations are absent
- **WHEN** an MCP client lists the available tools
- **THEN** no tool exposes `GET /api/health`, `POST /api/idle/start` or `POST /api/idle/stop`

#### Scenario: A tool and its HTTP counterpart agree
- **WHEN** a tool and the equivalent HTTP route are exercised with the same input
- **THEN** they return the same domain result, because both call the same service function

### Requirement: Tool schemas are derived from the API types

The service SHALL derive each tool's `inputSchema` and, where applicable, `outputSchema` from the
existing API types with `schemars::JsonSchema` (JSON Schema 2020-12), so optional parameters stay
optional and the pagination bounds match the HTTP API (default limit `50`, maximum `200`).

#### Scenario: Every tool carries a schema
- **WHEN** an MCP client inspects any of the seventeen tools
- **THEN** the tool declares an `inputSchema` derived from the API types

#### Scenario: Optional parameters stay optional
- **WHEN** a tool whose HTTP counterpart has optional query parameters is inspected
- **THEN** those parameters are optional in the tool schema, not required

#### Scenario: Result limits match the HTTP bounds
- **WHEN** the `list_messages` tool receives a `limit`
- **THEN** a value below `1` or above `200` is rejected, a missing value defaults to `50`, exactly as the HTTP API does

### Requirement: Domain errors become readable tool errors

The service SHALL map every domain failure to an MCP tool error that carries the same stable `error`
code as the HTTP API plus a human-readable message, and SHALL never include credentials or
third-party error text.

#### Scenario: Unknown mailbox
- **WHEN** a tool selects or lists messages of a mailbox that does not exist on the server
- **THEN** the tool error reports the stable code `mailbox_not_found`, the same code the HTTP API uses for that condition

#### Scenario: IMAP unavailable
- **WHEN** the IMAP session cannot be established while a tool runs
- **THEN** the tool error reports the stable code `imap_unavailable`

#### Scenario: Capability not supported
- **WHEN** the server does not support a requested capability, such as the flag operation without `UIDPLUS`
- **THEN** the tool error reports the stable code `capability_not_supported`

#### Scenario: Message not found
- **WHEN** a tool receives a `uid` that does not exist in the target mailbox
- **THEN** the tool error reports the stable code `message_not_found`, the same code the HTTP API uses for that condition

#### Scenario: Read size limit
- **WHEN** a tool must fetch or parse a message, or return an attachment, whose size exceeds the configured limit (`APIMAIL_MAX_MESSAGE_BYTES`, 25 MiB by default)
- **THEN** the tool error reports the stable code `message_too_large` instead of returning an oversized payload

#### Scenario: Send size limit
- **WHEN** `send_message` receives a payload whose decoded attachments or raw body exceed the configured limit
- **THEN** the tool error reports the stable code `payload_too_large`

#### Scenario: Invalid input
- **WHEN** a tool receives a blank mailbox, a non-numeric or zero `uid`, or a `send_message` with malformed recipients or attachments
- **THEN** the tool error reports the stable code `invalid_request`

#### Scenario: SMTP failure
- **WHEN** `send_message` fails because the SMTP server rejects or cannot accept the message
- **THEN** the tool error reports the stable code `smtp_error`

#### Scenario: No secret leak
- **WHEN** any tool error is returned
- **THEN** it contains neither the API key, nor the IMAP/SMTP password, nor raw third-party error text

### Requirement: Bounded, side-effect-free tool payloads

The service SHALL keep tool payloads bounded and free of side effects: message and attachment
metadata SHALL never embed the attachment bytes, only the explicit attachment tool MAY return bytes
(base64-encoded and within the same limits as the HTTP API), and no file SHALL be written to disk
except the durable queue file already configured through `APIMAIL_QUEUE_PATH`.

#### Scenario: Attachment metadata carries no bytes
- **WHEN** a tool reports the attachments of a message
- **THEN** each attachment is listed with its metadata and never its content

#### Scenario: Oversized attachment fails instead of streaming
- **WHEN** the attachment tool is asked for an attachment whose decoded size exceeds the configured limit
- **THEN** the tool returns an error instead of streaming or truncating the bytes

#### Scenario: Nothing is written to disk
- **WHEN** tools are exercised and no queue path is configured
- **THEN** no file is created or modified on the filesystem

### Requirement: Write tools state their destructive semantics

The service SHALL document the destructive semantics of the write tools: `delete_message` marks the
message `\Deleted` and runs `EXPUNGE` (permanently purging it rather than moving it to a Trash
folder) and therefore requires `UIDPLUS`; `move_message` requires an explicit destination mailbox;
`update_flags` changes only the flags explicitly named.

#### Scenario: Delete purges and does not fill Trash
- **WHEN** `delete_message` succeeds
- **THEN** the message is marked `\Deleted` and expunged from the mailbox, and no copy is placed in a Trash mailbox

#### Scenario: Move requires an explicit destination
- **WHEN** `move_message` is invoked without a destination mailbox
- **THEN** the tool error reports the stable code `invalid_request`

#### Scenario: Flags change only what was named
- **WHEN** `update_flags` is invoked with a set of flag changes
- **THEN** only the named flags are modified and the message's other flags are left untouched

### Requirement: Explicit mailbox parameters keep concurrent calls independent

Every tool that reads or mutates a message SHALL receive the mailbox explicitly, so `select_mailbox`
cannot silently retarget another tool, and the service SHALL serialize access to the shared IMAP
session (a single connection guarded by a mutex) so that concurrent calls neither corrupt the
session state nor mix up their responses.

#### Scenario: Concurrent calls serialize on the shared session
- **WHEN** several tools run concurrently against the single shared IMAP session
- **THEN** their accesses are serialized and each call receives its own correct response, with no interleaved or crossed results

#### Scenario: select_mailbox does not retarget other tools
- **WHEN** a tool is invoked with an explicit mailbox after `select_mailbox` selected a different one
- **THEN** that tool operates on the mailbox it was given, not on whatever `select_mailbox` left selected

#### Scenario: A failed call leaves the session usable
- **WHEN** a tool call fails against the shared session
- **THEN** the failure is contained and the session remains usable for subsequent calls
