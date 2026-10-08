# Tasks

## 1. Capa de dominio (TDD)

- [x] 1.1 RED: en `src/imap.rs`, tests unitarios — `flag_label`/`attribute_label` mapean los valores conocidos; `ConnectionManager::list_mailboxes` con una sesión falsa devuelve los buzones guionizados y **reutiliza** la sesión; `select_mailbox` devuelve el estado guionizado; un `select` que responde `NO` produce `MailboxNotFound`; una sesión muerta se descarta y reconecta (mismo número acotado de `connect`). Ejecutar `cargo test` y confirmar el fallo.
- [x] 1.2 GREEN: ampliar `ImapSession` (`list_mailboxes`, `select`), añadir `MailboxInfo`/`MailboxStatus`, la variante `ImapError::MailboxNotFound`, el mapeo de flags/atributos, `ensure_session` y los métodos `list_mailboxes`/`select_mailbox` del `ConnectionManager`; implementarlos en `SessionHandle` sobre `async-imap`. Ajustar las sesiones falsas existentes al trait ampliado. Verificar `cargo test`.
- [x] 1.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Endpoints HTTP (TDD)

- [x] 2.1 RED: crear `tests/mailboxes.rs` con una sesión falsa inyectada en `AppState` — `GET /api/mailboxes` OK → `200` con `mailboxes`; `POST /api/mailboxes/select` OK → `200` con metadatos; buzón inexistente (`NO`) → `404 mailbox_not_found`; body malformado o `mailbox` vacío → `400 invalid_request`; servidor caído → `503 imap_unavailable`; sin API key → `401`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 2.2 GREEN: handlers `list_mailboxes` y `select_mailbox`, DTOs/respuestas, mapeo de errores a `400/404/503`, montaje en el sub-router protegido y documentación del contrato en el doc de módulo de `src/http.rs`. Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Integración y verificación

- [x] 3.1 Ajustar los tests existentes afectados por el trait ampliado sin debilitar aserciones.
- [x] 3.2 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 3.3 Confirmar que `just check-spec` pasa con este change activo.

## 4. Documentación y cierre

- [x] 4.1 Documentar `GET /api/mailboxes` y `POST /api/mailboxes/select` en `README.md`.
- [x] 4.2 `openspec validate imap-mailboxes --strict` en verde y `openspec archive imap-mailboxes` tras la aprobación e implementación.
