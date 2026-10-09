# Proposal

## Why

`imap-idle` entrega las notificaciones **at-most-once**: `deliver()` reintenta 3 veces con
esperas de 1 s y 2 s **bloqueando** el bucle del supervisor y, si el webhook sigue caído,
**descarta el mensaje** (avanza `last_uid` igualmente). Además la cola vive solo en memoria, así
que un reinicio del proceso (deploy, OOM, reboot) pierde lo pendiente y, al volver, fija el
punto de arranque en `UIDNEXT-1` y **salta** el correo llegado durante la caída.

Es decir: la promesa del roadmap («al llegar un correo, se notifica al webhook») se rompe en los
dos escenarios que más importan en producción — webhook caído y servicio reiniciado.

## What Changes

- Nueva capability `webhook-delivery`.
- **Ingesta y entrega desacopladas**: cada mensaje nuevo se **encola**; un worker de entrega drena
  la cola en orden FIFO y **reintenta con backoff exponencial acotado hasta entregar**, sin
  bloquear la suscripción. La entrega pasa a ser **at-least-once** (puede haber duplicados; el
  receptor deduplica con `(mailbox, uid_validity, uid)`, que el payload **ya** incluye: el
  contrato de notificación no cambia).
- **Cola persistente opt-in** (`APIMAIL_QUEUE_PATH`): sin la variable, la cola vive en memoria y
  **no se escribe nada a disco** (comportamiento por defecto intacto); con ella, las
  notificaciones pendientes y el watermark del buzón se guardan en ese fichero con permisos
  `0600`, escritura atómica y recarga al arrancar.
- **Reanudación**: con cola persistente, la suscripción **continúa desde el watermark guardado**
  (mismo buzón y `UIDVALIDITY`), de modo que el correo llegado con el servicio caído también se
  notifica. Sin persistencia no hay watermark y el arranque sigue saltando el backlog como hoy.
  Un cambio de `UIDVALIDITY` o de buzón resetea el punto.
- **Cola acotada y observable**: `APIMAIL_QUEUE_MAX_ITEMS` (def. 1000) y
  `APIMAIL_QUEUE_MAX_BYTES` (def. 64 MiB); al desbordar se descarta **el más antiguo** y se
  cuenta. Nuevo endpoint protegido `GET /api/idle/queue` con `persistent`, `pending`,
  `delivered`, `dropped`, `failed` y `oldest_pending_secs`.
- **Fallo no reintentable** (respuesta `4xx`): intentos acotados y descarte contado, para que un
  mensaje "venenoso" no bloquee la FIFO.
- **Sin dependencias nuevas**: la cola es un log JSONL append-only con `fsync` en el encolado y
  compactación atómica cuando crece; el I/O va en `spawn_blocking` con `std::fs`.

No hay **BREAKING**: los contratos de las rutas existentes y del payload no cambian.

## Capabilities

### New Capabilities
- `webhook-delivery`: cola de entrega duradera, persistente, acotada y observable para las
  notificaciones del webhook.

### Modified Capabilities
- `imap-idle`:
  - *New-message notification*: el punto de arranque pasa a ser el **watermark guardado** cuando
    hay cola persistente (para no perder el correo llegado durante una caída); sin persistencia
    se mantiene el comportamiento actual.
  - *Bounded, side-effect-free handling of untrusted content*: «no escribir correo a disco» pasa
    a «nada a disco **salvo** que se configure `APIMAIL_QUEUE_PATH`, y entonces con permisos solo
    para el propietario».

## Impact

- `src/queue.rs` (**nuevo**): modelo de la cola, log JSONL con `fsync` + compactación atómica,
  `WebhookQueue` (impl en memoria y persistente) y el worker de entrega.
- `src/idle.rs`: el supervisor encola en vez de entregar en línea; se mantiene el protocolo
  *encolar (durable) y después avanzar el watermark*; `deliver()` se sustituye por el worker.
- `src/config.rs`: `queue_path`, `queue_max_items`, `queue_max_bytes` (+ `ConfigError`).
- `src/http.rs`: `GET /api/idle/queue` y su DTO.
- `src/lib.rs`: re-exports.
- `tests/queue.rs` (nuevo) y ampliación de `tests/idle.rs`; tests unitarios en `src/queue.rs` y
  `src/config.rs`.
- `README.md`: la cola, sus tres variables, el endpoint nuevo y la semántica at-least-once.
- `plans/PLAN-001.md`: marcar el ítem #3.

## Fuera de alcance

Contenido de adjuntos en el payload (`attachment-content`), suscripción a varios buzones o
cuentas (`idle-scope`), almacén en base de datos (la cola es un log JSONL sin dependencias),
entrega exactamente-una-vez y ordenación global entre reinicios más allá del orden de encolado.
