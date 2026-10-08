# Proposal

## Why

`imap-connection` ya establece una sesión IMAP autenticada y expone su estado, pero la
API **no puede descubrir ni elegir buzones**: sin `LIST` no sabe qué carpetas existen y
sin `SELECT` no hay un buzón activo sobre el que leer. Es la base de `imap-messages`
(listar/buscar/fetch) e `imap-flags` (marcar/mover), que operan sobre un buzón
seleccionado.

## What Changes

- Nueva capability `imap-mailboxes`.
- **Listar buzones**: `GET /api/mailboxes` (protegido) → nombre, delimitador de
  jerarquía y atributos de cada buzón.
- **Seleccionar buzón**: `POST /api/mailboxes/select` (protegido) con
  `{"mailbox":"INBOX"}` → metadatos del buzón (`EXISTS`, `RECENT`, `UNSEEN`,
  `UIDVALIDITY`, `UIDNEXT`, `FLAGS`) y deja ese buzón como activo en la sesión.
- Los traits `ImapSession`/`ImapConnector` se amplían: `ImapSession` gana
  `list_mailboxes()` y `select()`; el `ConnectionManager` gana `list_mailboxes()` y
  `select_mailbox()`, reutilizando la garantía de sesión viva (NOOP + reconexión).
- Errores con el mismo modelo JSON: `404` buzón inexistente, `503` servidor/sesión no
  disponible, `400` petición inválida, `401` sin API key.
- Todo inyectable: los tests usan sesiones falsas, sin red.

## Capabilities

### New Capabilities
- `imap-mailboxes`: listado y selección de buzones sobre la sesión IMAP existente.

### Modified Capabilities
<!-- Ninguna: `imap-connection` conserva sus requisitos; solo se amplía la superficie interna de su trait, que no forma parte de su contrato. -->

## Impact

- `src/imap.rs`: tipos `MailboxInfo`/`MailboxStatus`; métodos nuevos en `ImapSession`
  (`list_mailboxes`, `select`) y en `ConnectionManager` (`list_mailboxes`,
  `select_mailbox`); refactor de `status()` para compartir la garantía de sesión
  (`ensure_session`).
- `src/http.rs`: handlers `GET /api/mailboxes` y `POST /api/mailboxes/select`, DTOs y
  respuestas, rutas protegidas y documentación del contrato en el módulo.
- `tests/`: nuevo `tests/mailboxes.rs`; ajuste de las sesiones falsas existentes en
  `src/imap.rs` y `tests/imap_status.rs` al trait ampliado.
- `README.md`: documentar ambos endpoints.
- `Cargo.toml`/`Cargo.lock`: nueva dependencia `futures-util` (consume el
  `Stream` devuelto por `Session::list` con `TryStreamExt::try_collect`).
- **Fuera de alcance**: crear/renombrar/borrar buzones, `STATUS` de conteos,
  `SUBSCRIBE`/`LSUB`, árbol anidado (lista plana + delimitador), IMAP modified UTF-7,
  `imap-messages`/`imap-flags`/`IDLE`.
