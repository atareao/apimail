# Tasks

## 1. Capa de dominio (TDD)

- [x] 1.1 RED: en `src/imap.rs`, tests unitarios de las funciones puras — `SystemFlag::parse`
  acepta las cinco banderas en cualquier caso y las canonicaliza, y rechaza cualquier otra;
  el parseo de `add`/`remove` rechaza un flag repetido entre ambos y exige al menos uno no
  vacío; la composición de la *query* `STORE` rinde `+FLAGS.SILENT (\Seen \Flagged)` /
  `-FLAGS.SILENT (\Deleted)` solo con el allowlist e items fijos (y jamás texto libre); el *uid set*
  rinde un `u32`; el mapeo de capacidades detecta `MOVE` y `UIDPLUS`. Ejecutar `cargo test`
  y confirmar el fallo.
- [x] 1.2 GREEN: añadir `SystemFlag` (allowlist + parseo/canonicalización), `Capabilities`,
  la variante `ImapError::CapabilityNotSupported`, los helpers puros de composición segura
  del `STORE`, los métodos `store`/`copy`/`move_message`/`uid_expunge`/`capabilities` en
  `ImapSession` (implementados en `SessionHandle` con `uid_store`/`uid_copy`/`uid_mv`/
  `uid_expunge`/`capabilities`) y `update_flags`/`copy_message`/`move_message`/
  `delete_message` en `ConnectionManager` (composición `select`→comando, y
  `MOVE`→fallback `UIDPLUS`→`501`). Ampliar los fakes existentes. Verificar `cargo test`.
- [x] 1.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Endpoints HTTP (TDD)

- [x] 2.1 RED: crear `tests/flags.rs` con una sesión falsa inyectada en `AppState` (que
  registra los comandos, banderas y capacidades recibidos) — `PATCH .../flags` OK → `200`
  con `mailbox/uid/flags`; flag fuera del allowlist, `add` y `remove` contradictorios o body
  vacío → `400 invalid_request`; `POST .../move` OK con `MOVE` → `200` `"moved"`; sin `MOVE`
  pero con `UIDPLUS` → emulación (`COPY`+`STORE`+`UID EXPUNGE`); sin ninguna → `501
  capability_not_supported`; `POST .../copy` OK → `200` `"copied"`; `DELETE ...` OK con
  `UIDPLUS` → `200` `"deleted"`; sin `UIDPLUS` → `501`; `to` ausente/vacío o con control
  chars → `400`; `uid` no numérico/0 o `mailbox` ausente/vacío/con control chars → `400`;
  buzón inexistente → `404 mailbox_not_found`; mensaje inexistente → `404
  message_not_found`; servidor caído → `503 imap_unavailable`; sin API key → `401`. Ejecutar
  `cargo test` y confirmar el fallo.
- [x] 2.2 GREEN: handlers `update_flags`/`move_message`/`copy_message`/`delete_message`,
  DTOs/cuerpos y respuestas, `MessageError::CapabilityNotSupported` → `501
  capability_not_supported`, rutas protegidas `PATCH /api/messages/{uid}/flags`, `POST
  /api/messages/{uid}/move`, `POST /api/messages/{uid}/copy` y `DELETE /api/messages/{uid}`,
  y documentación del contrato en el doc de módulo de `src/http.rs`. Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Integración y verificación

- [x] 3.1 Ajustar los tests existentes afectados por el trait ampliado (`src/imap.rs`,
  `tests/messages.rs`, `tests/imap_status.rs`, `tests/mailboxes.rs`) sin debilitar
  aserciones.
- [x] 3.2 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 3.3 Confirmar que `just check-spec` pasa con este change activo.

## 4. Documentación y cierre

- [x] 4.1 Documentar los cuatro endpoints nuevos en `README.md`.
- [x] 4.2 `openspec validate imap-flags --strict` en verde y `openspec archive imap-flags`
  tras la aprobación e implementación.
