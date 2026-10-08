# Tasks

## 1. Capa de dominio (TDD)

- [x] 1.1 RED: en `src/imap.rs`, tests unitarios de las funciones puras — `quote_search_string` escapa `\` y `"` y descarta `CR`/`LF`/`NUL`; `SearchCriteria::imap_key` rinde `ALL` sin filtros y las claves `FROM/TO/SUBJECT/BODY/SINCE/BEFORE/SEEN/UNSEEN/FLAGGED/UNFLAGGED` con citado y fecha `DD-Mon-YYYY`; `SearchDate::parse` acepta `YYYY-MM-DD` válido y rechaza formatos/fechas inválidas; `FetchFormat::query` rinde las tres constantes con `BODY.PEEK`; el mapeo `Fetch`→`Message` (uid/seq/flags/size/internal_date/envelope/address) y `MessageNotFound`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 1.2 GREEN: añadir los tipos (`SearchCriteria`, `SearchDate`, `FetchFormat`, `Message`, `MessageEnvelope`, `Address`, `MessagePage`), la variante `ImapError::MessageNotFound`, los helpers puros, los métodos `search`/`fetch` en `ImapSession` (implementados en `SessionHandle` con `uid_search`/`uid_fetch`) y `list_messages`/`fetch_message` en `ConnectionManager` (composición `select`→`search`→ventana→`fetch`, sin `UID FETCH` con set vacío). Ampliar los fakes existentes. Verificar `cargo test`.
- [x] 1.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Endpoints HTTP (TDD)

- [x] 2.1 RED: crear `tests/messages.rs` con una sesión falsa inyectada en `AppState` (que registra los criterios y el formato recibidos) — `GET /api/messages` OK → `200` con `total/limit/offset/messages`; paginación y orden; filtros traducidos; `GET /api/messages/{uid}` en `summary`/`headers`/`full` → `200` con `headers_base64`/`raw_base64`; mailbox inexistente → `404 mailbox_not_found`; uid inexistente → `404 message_not_found`; fecha/bool/limit/offset/format inválidos o `mailbox` vacío → `400 invalid_request`; servidor caído → `503 imap_unavailable`; sin API key → `401`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 2.2 GREEN: handlers `list_messages` y `fetch_message`, parseo de query con `RawQuery`+`serde_urlencoded`, DTOs/respuestas, `MessageError` → `400/404/404/503`, rutas protegidas `GET /api/messages` y `GET /api/messages/{uid}`, y documentación del contrato en el doc de módulo de `src/http.rs`. Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Integración y verificación

- [x] 3.1 Ajustar los tests existentes afectados por el trait ampliado (`src/imap.rs`, `tests/imap_status.rs`, `tests/mailboxes.rs`) sin debilitar aserciones.
- [x] 3.2 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 3.3 Confirmar que `just check-spec` pasa con este change activo.

## 4. Documentación y cierre

- [x] 4.1 Documentar `GET /api/messages` y `GET /api/messages/{uid}` en `README.md`.
- [x] 4.2 `openspec validate imap-messages --strict` en verde y `openspec archive imap-messages` tras la aprobación e implementación.
