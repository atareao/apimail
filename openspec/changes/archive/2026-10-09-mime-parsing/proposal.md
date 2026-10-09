# Proposal

## Why

`imap-messages` permite descargar el mensaje crudo (`format=full`, en base64) e `imap-flags`
modifica su estado, pero la API no sabe **interpretar** el MIME: quien consume tendría que
parsear el RFC822 por su cuenta para obtener el texto, el HTML o los adjuntos. Un correo sin
MIME parseado no es directamente utilizable (ni para mostrarlo, ni para integrarlo, ni como
payload del futuro webhook de `imap-idle`). Es la pieza que separa «descargar correo» de
«usar correo».

## What Changes

- Nueva capability `mime-parsing`.
- **Cuerpo parseado**: `GET /api/messages/{uid}/body?mailbox=INBOX` (protegido) →
  `{"mailbox","uid","text","html"}` (`text`/`html` son `null` si no hay parte visible;
  cuando falta una alternativa, el parser la deriva de la otra según RFC 8621 §4.1.4). El
  HTML se devuelve **sin sanear**.
- **Listar adjuntos**: `GET /api/messages/{uid}/attachments?mailbox=INBOX` →
  `{"mailbox","uid","attachments":[{"id","filename","content_type","size","inline","content_id"}]}`.
  `id` es posicional (índice en la lista de adjuntos parseados) y estable para un mismo
  mensaje.
- **Descargar un adjunto**: `GET /api/messages/{uid}/attachments/{id}?mailbox=INBOX` →
  `{"mailbox","uid","id","filename","content_type","size","content_base64"}` (el contenido
  descodificado, en base64 dentro del JSON, igual que `raw_base64`/`headers_base64` de
  `imap-messages`).
- **Parseo con `mail-parser`** en un módulo puro `src/mime.rs` (sin I/O), alimentado con el
  `fetch(Full)` que ya existe.
- **Límite configurable** `APIMAIL_MAX_MESSAGE_BYTES` (por defecto 25 MiB) para acotar el
  parseo; superarlo → `413 message_too_large`.
- Nuevos códigos de error: `404 attachment_not_found`, `413 message_too_large`, `422
  message_not_parsable` (además de los ya existentes `400`, `401`, `404 mailbox_not_found`,
  `404 message_not_found`, `503 imap_unavailable`).
- **No se modifica `imap-messages`**: se añaden endpoints nuevos, no se toca el parámetro
  `format` de `GET /api/messages/{uid}`.

## Capabilities

### New Capabilities
- `mime-parsing`: interpretación del mensaje MIME crudo — texto plano, HTML y adjuntos
  (metadatos y contenido) — sobre la sesión IMAP perezosa existente.

### Modified Capabilities
<!-- Ninguna: `imap-messages`, `imap-flags` e `imap-connection` conservan sus contratos
HTTP y de trait; `mime-parsing` solo reutiliza el `fetch(Full)` ya existente. -->

## Impact

- `Cargo.toml`: nueva dependencia `mail-parser = { version = "0.11", features = ["full_encoding"] }`.
- `src/mime.rs` (**nuevo**): tipos `ParsedMessage`/`ParsedAttachment` y `parse_message` puro
  (RFC5322/MIME/2047/2231) con conversión a tipos propios, sin I/O.
- `src/imap.rs`: `ConnectionManager::fetch_parsed(mailbox, uid, max_bytes)` (una toma del
  lock: `select` → `fetch(Summary)` para existencia y `RFC822.SIZE` → guarda de tamaño →
  `fetch(Full)` → `mime::parse_message`) y el enum `ParsedMessageError`
  (`Imap`/`TooLarge`/`Unparsable`). **Sin cambios en el trait `ImapSession`.**
- `src/config.rs`: `Config.max_message_bytes` + `APIMAIL_MAX_MESSAGE_BYTES` y su variante de
  `ConfigError`.
- `src/http.rs`: handlers/DTOs/respuestas y rutas `GET /api/messages/{uid}/body`,
  `GET /api/messages/{uid}/attachments`, `GET /api/messages/{uid}/attachments/{id}`;
  variantes nuevas de `MessageError` (`AttachmentNotFound`→404, `TooLarge`→413,
  `Unparsable`→422).
- `src/lib.rs`: re-exports de los tipos nuevos.
- `tests/mime.rs` (nuevo) y tests de `src/mime.rs`/`src/config.rs`/`src/imap.rs`.
- `README.md`: documentar los tres endpoints y la variable de entorno nueva.
- **Fuera de alcance**: escribir adjuntos a disco; descarga en streaming/chunked; resolución
  o reescritura de recursos `cid:` inline en el HTML; sanear el HTML; añadir `format=parsed`
  a `GET /api/messages/{uid}` (se expone un endpoint `/body` aparte); construcción de MIME
  (`mail-builder`); `IDLE`/webhook (`imap-idle`).
