# Tasks

## 1. Módulo de parseo (TDD)

- [x] 1.1 RED: en `src/mime.rs`, tests unitarios con mensajes RFC822 embebidos — un multipart
  con text+html+adjunto lista los adjuntos en orden con sus metadatos (`filename` con
  RFC 2047/2231, `content_type`, `size`, `inline`, `content_id`); `text`/`html` devuelven el
  texto y el HTML; un mensaje sin parte visible devuelve `text`/`html` nulos; un `id` fuera de
  rango devuelve `None`; una entrada no parseable devuelve `Err`. Ejecutar `cargo test` y
  confirmar el fallo.
- [x] 1.2 GREEN: añadir la dependencia `mail-parser` (`full_encoding`) y el módulo
  `src/mime.rs` con `ParsedMessage`/`ParsedAttachment`/`MimeError` y `parse_message`
  (solo `MessageParser`, conversión a tipos propios, sin I/O). Verificar `cargo test`.
- [x] 1.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Configuración y orquestación (TDD)

- [x] 2.1 RED: tests de `Config` para `APIMAIL_MAX_MESSAGE_BYTES` (ausente → 25 MiB; válido →
  el valor; `0`/no numérico → error que nombra la variable) y de
  `ConnectionManager::fetch_parsed` con una sesión falsa que sirve un RFC822 (devuelve
  `ParsedMessage`; mensaje inexistente → `MessageNotFound`; tamaño por encima del límite →
  `TooLarge`; contenido no parseable → `Unparsable`). Ejecutar `cargo test` y confirmar el
  fallo.
- [x] 2.2 GREEN: `Config.max_message_bytes` + variante de `ConfigError`;
  `ConnectionManager::fetch_parsed(mailbox, uid, max_bytes)` (`select` → `Summary` → guarda
  de tamaño → `fetch(Full)` → `mime::parse_message`) y `ParsedMessageError`; re-exportar los
  tipos nuevos en `src/lib.rs`. Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Endpoints HTTP (TDD)

- [x] 3.1 RED: crear `tests/mime.rs` con una sesión falsa inyectada en `AppState` (sirve un
  RFC822 y registra el `mailbox`/`uid` pedidos) — `GET .../{uid}/body` OK → `200` con
  `mailbox/uid/text/html`; `GET .../{uid}/attachments` OK → `200` con la lista y sus
  metadatos; `GET .../{uid}/attachments/{id}` OK → `200` con `content_base64` decodificable;
  mensaje sin adjuntos → array vacío; `id` fuera de rango → `404 attachment_not_found`;
  tamaño excedido → `413 message_too_large`; MIME no parseable → `422 message_not_parsable`;
  `uid`/`id` no numérico o `0`, `mailbox` ausente/vacío/con control chars → `400` sin comando;
  buzón inexistente → `404 mailbox_not_found`; mensaje inexistente → `404 message_not_found`;
  servidor caído → `503 imap_unavailable`; sin API key → `401`. Ejecutar `cargo test` y
  confirmar el fallo.
- [x] 3.2 GREEN: handlers `get_message_body`/`list_attachments`/`get_attachment`, DTOs y
  respuestas, `MessageError::{AttachmentNotFound,TooLarge,Unparsable}` con sus códigos (`404
  attachment_not_found`, `413 message_too_large`, `422 message_not_parsable`), mapeo desde
  `ParsedMessageError`, rutas protegidas `GET /api/messages/{uid}/body`,
  `GET /api/messages/{uid}/attachments` y `GET /api/messages/{uid}/attachments/{id}`, y
  documentación del contrato en el doc de módulo de `src/http.rs`. Verificar `cargo test`.
- [x] 3.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 4. Integración y verificación

- [x] 4.1 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 4.2 Confirmar que `just check-spec` pasa con este change activo.

## 5. Documentación y cierre

- [x] 5.1 Documentar los tres endpoints y `APIMAIL_MAX_MESSAGE_BYTES` en `README.md`.
- [x] 5.2 `openspec validate mime-parsing --strict` en verde y `openspec archive
  mime-parsing` tras la aprobación e implementación.
