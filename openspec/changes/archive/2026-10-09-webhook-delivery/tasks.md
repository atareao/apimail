# Tasks

## 1. Cola (TDD)

- [x] 1.1 RED: en `src/queue.rs`, tests unitarios del modelo y del log JSONL: encolar/desencolar en
  orden FIFO; el fichero se relee al abrir y reconstruye la cola pendiente; una última línea
  incompleta se descarta; `ack` y `drop` se aplican al releer; conteos `pending`/`delivered`/
  `dropped`/`failed`; edad del más antiguo; compactación cuando el fichero supera el umbral (y que
  después del `rename` el contenido sigue siendo correcto); desbordamiento por ítems y por bytes
  descartando el más antiguo e incrementando `dropped`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 1.2 GREEN: `WebhookQueue` (trait + implementación en memoria y persistente) con el log JSONL
  append-only, `fsync` al encolar, compactación atómica y el I/O en `spawn_blocking`. Verificar
  `cargo test`.
- [x] 1.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Configuración (TDD)

- [x] 2.1 RED: tests de `Config` para `APIMAIL_QUEUE_PATH` (ausente/en blanco → `None`;
  definida → validada y usable, o error que nombra la variable),
  `APIMAIL_QUEUE_MAX_ITEMS` (defecto 1000; `0`/no numérico → error) y
  `APIMAIL_QUEUE_MAX_BYTES` (defecto 64 MiB; `0`/no numérico → error).
- [x] 2.2 GREEN: los tres campos en `Config` (+ `ConfigError`) con validación **fail-closed** al
  arranque. Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Supervisor y worker de entrega (TDD)

- [x] 3.1 RED: en `src/idle.rs`, tests con la cola y el webhook falsos: un mensaje nuevo se
  **encola** (no se entrega en línea) y la ingesta **no se bloquea** mientras el webhook falla;
  el worker reintenta un fallo reintentable hasta entregar; un `4xx` se descarta tras los intentos
  acotados y se cuenta como `failed`; el watermark **solo avanza tras encolar con éxito** (si el
  encolado falla, no avanza y la suscripción sigue viva con `last_error`); con cola persistente la
  suscripción **reanuda** desde el watermark guardado y notifica lo llegado durante la caída; sin
  persistencia el arranque sigue saltando el backlog.
- [x] 3.2 GREEN: sustituir `deliver()` por el worker de entrega, encolar desde el supervisor y
  persistir el watermark. Verificar `cargo test`.
- [x] 3.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 4. Endpoint de observabilidad (TDD)

- [x] 4.1 RED: en `tests/queue.rs`, `GET /api/idle/queue` con API key → `200` con `persistent`,
  `pending`, `delivered`, `dropped`, `failed` y `oldest_pending_secs`; sin API key → `401`.
- [x] 4.2 GREEN: ruta, handler y DTO en `src/http.rs` (contrato documentado en el doc de módulo).
  Verificar `cargo test`.
- [x] 4.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 5. Integración y verificación

- [x] 5.1 `just test`, `just clippy` y `just fmt` en verde.
- [x] 5.2 Integración real: cola persistente end-to-end — webhook caído → los mensajes quedan
  pendientes en el fichero; reinicio del supervisor → se reanuda el watermark y se entrega lo
  pendiente.
- [x] 5.3 Comprobar que el fichero de cola se crea con permisos `0600` y que no contiene el
  password de IMAP ni texto de terceros.

## 6. Documentación y cierre

- [x] 6.1 Documentar en `README.md` la cola, `APIMAIL_QUEUE_PATH`/`APIMAIL_QUEUE_MAX_ITEMS`/
  `APIMAIL_QUEUE_MAX_BYTES`, el endpoint `GET /api/idle/queue` y la semántica at-least-once
  (duplicados y clave de deduplicación).
- [x] 6.2 `openspec validate webhook-delivery --strict` en verde.
- [x] 6.3 Revisión con `rust-reviewer` (foco: protocolo de durabilidad, reanudación, sin fugas en
  el fichero de cola, y que el modo en memoria no escribe nada).
- [x] 6.4 `openspec archive webhook-delivery` y actualizar `plans/PLAN-001.md` (ítem #3).
