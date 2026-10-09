# Tasks

## 1. Conexión dedicada y abstracción IDLE (TDD)

- [x] 1.1 RED: en `src/idle.rs`, tests unitarios de la construcción pura del payload
  (`WebhookPayload::from_message`): metadatos completos, `parsed=true` con texto/HTML y
  adjuntos **sin** contenido, y `parsed=false` (texto/HTML nulos, adjuntos vacíos) cuando no
  hay `ParsedMessage`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 1.2 GREEN: extraer en `src/imap.rs` la rutina conexión+LOGIN compartida y añadir
  `src/idle.rs` con `IdleEvent`, los traits `IdleConnector`/`IdleSession`/`WebhookSender`
  (estilo `SendFuture`), `WebhookError`, el `WebhookPayload` tipado y su construcción pura.
  Verificar `cargo test`.
- [x] 1.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Configuración y supervisor (TDD)

- [x] 2.1 RED: tests de `Config` para `APIMAIL_WEBHOOK_URL` (ausente → `None`; válida
  `http`/`https` → `Some`; inválida → error que nombra la variable),
  `APIMAIL_IDLE_MAILBOX` (por defecto `INBOX`) y `APIMAIL_WEBHOOK_TIMEOUT_SECS` (defecto 10;
  `0`/no numérico → error). Y tests del `IdleSupervisor` con fakes — `start` arranca y es
  idempotente; `stop` para y es idempotente; `status` refleja `running`/`last_error`; un
  evento `Changed` con un mensaje nuevo hace que el webhook reciba el payload; solo se
  notifican UIDs > `last_uid`; un mensaje no parseable/sobredimensionado se notifica con
  `parsed=false`; un fallo del webhook deja `last_error="webhook_failed"` sin detener la
  suscripción. Ejecutar `cargo test` y confirmar el fallo.
- [x] 2.2 GREEN: `Config.webhook_url`/`idle_mailbox`/`webhook_timeout` (+ `ConfigError`);
  `IdleSettings`, `IdleSupervisor::{new,start,stop,status}`, el bucle con reconexión/backoff,
  la reemisión de `IDLE` y los reintentos acotados; `TokioIdleConnector`/`TokioIdleSession`
  sobre `Session::idle()`/`init`/`wait_with_timeout`/`done` y `uid_fetch`; re-exportar los
  tipos nuevos en `src/lib.rs`. Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Endpoints HTTP (TDD)

- [x] 3.1 RED: crear `tests/idle.rs` (con `IdleConnector`/`WebhookSender` falsos inyectados
  vía `AppState`) — `POST /api/idle/start` con webhook configurado → `200`
  `{"status":"running","mailbox":"INBOX"}`; `start` repetido → `200` idempotente; `POST
  /api/idle/stop` → `200 {"status":"stopped"}`; `GET /api/idle/status` → `200` con
  `status`/`mailbox`/`last_error`; sin webhook → `501 idle_not_configured`; sin API key →
  `401`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 3.2 GREEN: `AppState.idle` + `with_idle(...)`; handlers `idle_start`/`idle_stop`/
  `idle_status`; DTOs/respuestas; rutas protegidas; doc del contrato en el doc de módulo.
  Verificar `cargo test`.
- [x] 3.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 4. Integración y verificación

- [x] 4.1 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 4.2 Confirmar que `just check-spec` pasa con este change activo.

## 5. Documentación y cierre

- [x] 5.1 Documentar los tres endpoints y `APIMAIL_WEBHOOK_URL`/`APIMAIL_IDLE_MAILBOX`/
  `APIMAIL_WEBHOOK_TIMEOUT_SECS` en `README.md`.
- [x] 5.2 `openspec validate imap-idle --strict` en verde y `openspec archive imap-idle`
  tras la aprobación e implementación.
