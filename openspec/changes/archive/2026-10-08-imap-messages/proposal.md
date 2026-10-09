# Proposal

## Why

`imap-mailboxes` permite listar y seleccionar buzones, pero todavía **no se puede leer
nada de ellos**: no hay forma de saber qué mensajes contiene un buzón ni de descargar un
mensaje. Sin `SEARCH`/`FETCH`, la API es incapaz de exponer el correo entrante. Es la base
de `imap-flags` (marcar/mover), `mime-parsing` (parsear el MIME descargado) e `imap-idle`
(que reutiliza el fetch para construir el payload del webhook).

## What Changes

- Nueva capability `imap-messages`.
- **Listar y buscar mensajes**: `GET /api/messages` (protegido). Query params:
  - `mailbox` (obligatorio): buzón a consultar; la petición lo selecciona y lee sin fijar
    `\Seen` (los cuerpos se piden con `BODY.PEEK`).
  - Filtros: `from`, `to`, `subject`, `text`, `since`, `before` (`YYYY-MM-DD`), `seen`,
    `flagged`, `unseen` (booleanos), traducidos a claves `SEARCH` de IMAP.
  - Paginación: `limit` (por defecto 50, máximo 200) y `offset` (por defecto 0).
  - Orden: por `UID` descendente (más reciente primero).
- **Descargar un mensaje**: `GET /api/messages/{uid}` (protegido), con `mailbox`
  (obligatorio) y `format` = `summary` (por defecto) | `headers` | `full`. Devuelve
  metadatos + envelope y, para `headers`/`full`, el bloque crudo en **base64** (el MIME no
  es UTF-8 y JSON exige UTF-8). El parseo a texto/HTML/adjuntos queda para `mime-parsing`.
- `ImapSession` se amplía con `search()`/`fetch()`; `ConnectionManager` gana
  `list_messages()`/`fetch_message()`, que componen `SELECT` + `SEARCH` + `FETCH` sobre la
  sesión perezosa existente.
- **Seguridad**: `async-imap 0.12` construye `SEARCH`/`FETCH` con **strings crudos sin
  validar** (interpola en la línea del comando). Aquí los valores del usuario se validan y
  se rechazan `CR`/`LF`/`NUL`, y los literales se **citan/escapan**, para impedir inyección
  de comandos IMAP en la sesión autenticada.
- Errores JSON: `400 invalid_request`, `404 mailbox_not_found`, `404 message_not_found`,
  `503 imap_unavailable`, `401` sin API key.

## Capabilities

### New Capabilities
- `imap-messages`: listado/búsqueda de mensajes (SEARCH) y descarga de metadatos, cabeceras
  o mensaje crudo (FETCH), sobre la sesión IMAP existente.

### Modified Capabilities
<!-- Ninguna: `imap-connection` e `imap-mailboxes` conservan sus contratos HTTP; solo se
amplía la superficie interna del trait `ImapSession`, que no forma parte del contrato. -->

## Impact

- `src/imap.rs`: tipos `SearchCriteria`, `SearchDate`, `FetchFormat`, `Message`,
  `MessageEnvelope`, `Address`, `MessagePage`; variante `ImapError::MessageNotFound`;
  helpers puros de citado/escape y de construcción de claves `SEARCH` y consultas `FETCH`;
  métodos `search`/`fetch` en `ImapSession` e implementación en `SessionHandle`; métodos
  `list_messages`/`fetch_message` en `ConnectionManager`.
- `src/http.rs`: handlers `GET /api/messages` y `GET /api/messages/{uid}`, parsing y
  validación de query params, DTOs/respuestas, `MessageError` (`400/404/404/503`), rutas
  protegidas y documentación del contrato en el doc de módulo.
- `tests/`: nuevo `tests/messages.rs`; ajuste de las sesiones falsas existentes
  (`src/imap.rs`, `tests/imap_status.rs`, `tests/mailboxes.rs`) al trait ampliado.
- `Cargo.toml`/`Cargo.lock`: nueva dependencia `serde_urlencoded` (parsear la query string
  controlando el envelope JSON de `400`).
- `README.md`: documentar los dos endpoints nuevos.
- **Fuera de alcance**: modificar banderas, mover/copiar/borrar (`imap-flags`); parseo MIME
  a texto/HTML/adjuntos y descarga de secciones MIME concretas (`mime-parsing`); `SORT`/
  `THREAD`; IMAP modified UTF-7; `IDLE`/webhook.
