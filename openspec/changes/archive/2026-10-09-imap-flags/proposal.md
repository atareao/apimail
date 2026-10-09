# Proposal

## Why

`imap-messages` permite **leer** mensajes (listar/buscar y descargar), pero la API sigue
siendo estrictamente de solo lectura: no hay forma de marcar un mensaje como leído, marcarlo
con una bandera, archivarlo, moverlo, copiarlo ni borrarlo. Sin `STORE`/`COPY`/`MOVE`/
`EXPUNGE`, el CRUD sobre la cuenta de correo está incompleto. Es la pieza que falta entre la
lectura (`imap-messages`) y la automatización futura (`imap-idle`/webhook), y habilita la
gestión habitual del buzón.

## What Changes

- Nueva capability `imap-flags`.
- **Actualizar banderas**: `PATCH /api/messages/{uid}/flags?mailbox=INBOX` (protegido).
  Body JSON `{"add": ["\\Seen", "\\Flagged"], "remove": ["\\Deleted"]}` (ambos campos
  opcionales; al menos uno presente y no vacío; un mismo flag no puede estar en `add` y
  `remove`). Banderas con **allowlist estricta**: `\Seen`, `\Answered`, `\Flagged`,
  `\Draft`, `\Deleted` (case-insensitive, canonicalizadas). Respuesta `200` con las
  banderas resultantes del mensaje.
- **Mover**: `POST /api/messages/{uid}/move?mailbox=INBOX` con body `{"to":"Archive"}`.
  Si el servidor anuncia `MOVE` (RFC 6851) → `UID MOVE`; si no anuncia `MOVE` pero sí
  `UIDPLUS` (RFC 4315) → se emula con `UID COPY` + `UID STORE +FLAGS.SILENT (\Deleted)` +
  `UID EXPUNGE`; si no hay ninguna → `501 capability_not_supported`.
- **Copiar**: `POST /api/messages/{uid}/copy?mailbox=INBOX` con body `{"to":"Archive"}`.
  Usa `UID COPY` (base IMAP4rev1, sin capability extra).
- **Borrar**: `DELETE /api/messages/{uid}?mailbox=INBOX`. Marca `\Deleted` con `UID STORE`
  y purga **solo ese** mensaje con `UID EXPUNGE <uid>` (requiere `UIDPLUS`; si no se
  anuncia → `501`; nunca se usa `EXPUNGE` global).
- `ImapSession` se amplía con `store()`/`copy()`/`move_message()`/`uid_expunge()`/`capabilities()`;
  `ConnectionManager` gana `update_flags()`/`copy_message()`/`move_message()`/
  `delete_message()`, que componen `SELECT` + el comando correspondiente sobre la sesión
  perezosa existente.
- **Seguridad**: `async-imap 0.12` **interpola strings crudos** en `UID STORE <set> <query>`
  y `UID EXPUNGE <set>`. El *uid set* es siempre numérico (`u32`) y el `query` de `STORE` se
  compone **exclusivamente** del allowlist de banderas + items fijos (`+FLAGS`/`-FLAGS`),
  nunca de texto del usuario. El buzón destino de `copy`/`move` pasa por `validate_str` en
  la crate, pero además se rechazan `CR`/`LF`/`NUL` en el borde HTTP.
- Errores JSON (`{"error","message"}`): `400 invalid_request`, `401`, `404
  mailbox_not_found`, `404 message_not_found`, `501 capability_not_supported` (nuevo), `503
  imap_unavailable`.

## Capabilities

### New Capabilities
- `imap-flags`: actualización de banderas (`STORE`), copia (`COPY`), movimiento
  (`MOVE`, con emulación vía `UIDPLUS`) y borrado (`UID EXPUNGE`) de un mensaje, sobre la
  sesión IMAP existente.

### Modified Capabilities
<!-- Ninguna: `imap-messages`, `imap-connection` e `imap-mailboxes` conservan sus contratos
HTTP; solo se amplía la superficie interna del trait `ImapSession`, que no forma parte del
contrato. -->

## Impact

- `src/imap.rs`: tipo `SystemFlag` (allowlist + parseo/canonicalización); tipo
  `Capabilities` (MOVE/UIDPLUS); variante `ImapError::CapabilityNotSupported`; composición
  pura y segura de la consulta `STORE` y del *uid set*; métodos `store`/`copy`/`move`/
  `uid_expunge`/`capabilities` en `ImapSession` e implementación en `SessionHandle`;
  métodos `update_flags`/`copy_message`/`move_message`/`delete_message` en
  `ConnectionManager`.
- `src/http.rs`: handlers `PATCH /api/messages/{uid}/flags`, `POST /api/messages/{uid}/move`,
  `POST /api/messages/{uid}/copy` y `DELETE /api/messages/{uid}`; DTOs/cuerpos y respuestas;
  variante nueva `MessageError::CapabilityNotSupported` → `501 capability_not_supported`;
  rutas protegidas y documentación del contrato en el doc de módulo.
- `tests/`: nuevo `tests/flags.rs`; ajuste de las sesiones falsas existentes (`src/imap.rs`,
  `tests/messages.rs`, `tests/imap_status.rs`, `tests/mailboxes.rs`) al trait ampliado.
- `README.md`: documentar los cuatro endpoints nuevos.
- **Fuera de alcance**: `EXPUNGE` global; operaciones en lote / multi-uid (un solo `{uid}`
  por petición); parseo MIME a texto/HTML/adjuntos (`mime-parsing`); `IDLE`/webhook
  (`imap-idle`); `SORT`/`THREAD`; IMAP modified UTF-7.
