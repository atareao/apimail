# Design

## Context

- Hoy `src/idle.rs` entrega en línea: `deliver()` (aprox. L814) hace hasta 3 intentos con esperas
  de 1 s y 2 s **dentro** del bucle del supervisor; si se agotan, marca
  `last_error = "webhook_failed"` y **el mensaje se pierde** (el bucle avanza `last_uid`).
- La cola es implícita (la pila del bucle) y vive en memoria: un reinicio pierde lo pendiente.
- En el arranque, `run_supervisor` (aprox. L674) fija `last_uid = UIDNEXT - 1` y salta el
  backlog; solo re-deriva el punto si cambia `UIDVALIDITY`.
- `APIMAIL_MAX_MESSAGE_BYTES` es 25 MiB por defecto, así que un ítem de cola puede pesar MBs.
- `openspec/specs/imap-idle/spec.md` exige literalmente «*nor write mail content to disk*»
  (requisito *Bounded, side-effect-free handling of untrusted content*, escenario *Nothing is
  written to disk*): persistir exige **modificar** ese requisito.
- El payload ya incluye `mailbox`, `uid` y `uid_validity`, así que la clave de deduplicación de
  at-least-once **ya existe** y el contrato de notificación no cambia.
- Dependencias actuales minimalistas (`default-features = false`, rustls en vez de native-tls).
  `tokio` no tiene activada la feature `fs`.

## Goals / Non-Goals

**Goals**
- Que una caída del webhook no pierda ninguna notificación (at-least-once).
- Que un reinicio no pierda lo pendiente **ni** el correo llegado durante la caída.
- Que la garantía sea **observable** y sus límites explícitos y acotados.
- Que todo esto **no** imponga dependencias nuevas ni escrituras a disco por defecto.

**Non-Goals**
- Exactamente-una-vez (imposible sin coordinación con el receptor; se deduplica con la clave
  `(mailbox, uid_validity, uid)`).
- Base de datos: descartado añadir `rusqlite`/`bundled` porque impone compilar SQLite en C a
  **todo consumidor del crate** (incluso sin usar IDLE) y su API es bloqueante igualmente.
- Reducir el payload (eso es `attachment-content`).
- Ordenación entre reinicios más allá del orden de encolado.

## Decisions

1. **Cola = log JSONL append-only + compactación atómica** (`src/queue.rs`).
   - `enqueue` = una línea nueva + `fsync` (O(1)). `ack`/`drop` = una línea diminuta (O(1)).
   - Compactación **solo** cuando el fichero supera `max(64 KiB, 2 × bytes vivos)`; reemplazo
     atómico (temporal + `rename`). Rara por construcción.
   - Crash a mitad de línea: en la recarga se descarta la última línea incompleta (JSON inválido).
   - Descartado el fichero JSON único con reemplazo atómico **en cada operación**: es O(bytes de
     la cola) por mensaje (con 64 MiB de tope y ráfaga, cientos de MB/min de escritura).
   - Escrituras con `std::fs` dentro de `tokio::task::spawn_blocking` (no se activa la feature
     `fs` de tokio).
2. **Protocolo de durabilidad**: el watermark de IMAP **solo avanza después** de que el encolado
   sea durable (`fsync`). Así un crash nunca pierde un mensaje ya reconocido al servidor.
3. **Persistencia opt-in** con `APIMAIL_QUEUE_PATH`. Sin la variable, la implementación en
   memoria conserva la garantía «nada a disco» y el comportamiento actual. Con ella:
   validación al arranque (crear/abrir el fichero, permisos `0600` en Unix) y **fallo cerrado**
   si no es usable (coherente con `APIMAIL_API_KEY`/`APIMAIL_WEBHOOK_URL`). Descartado el
   «siempre persistente con ruta por defecto»: no hay ruta portable (contenedores sin `HOME`) y
   el *fallback* silencioso a memoria degradaría la garantía sin avisar.
4. **`persistente ⇒ reanuda`**: el watermark vive en el fichero de cola, así que la reanudación
   viene dada por la persistencia y **no hace falta un interruptor nuevo**. Sin persistencia no
   hay watermark ⇒ reset a `UIDNEXT - 1` (comportamiento por defecto intacto y el escenario
   actual de «*Messages present before start are not notified*» sigue cumpliéndose). Con
   persistencia se modifica el requisito para decir que continúa desde el watermark. Si cambia
   el buzón o `UIDVALIDITY`, se resetea (los UIDs dejan de ser válidos).
5. **Reintentos**: errores reintentables (`WebhookError::is_retryable`) se reintentan
   **indefinidamente** con backoff exponencial acotado (reutilizando `Backoff`) sin bloquear la
   ingesta. Errores no reintentables (`4xx`) se reintentan un número acotado de veces y luego se
   descartan contados como `failed`, para que un mensaje "venenoso" no bloquee la FIFO.
6. **Desbordamiento**: al superar `APIMAIL_QUEUE_MAX_ITEMS` o `APIMAIL_QUEUE_MAX_BYTES` se
   descarta **el más antiguo** y se incrementa `dropped`. Topes por defecto 1000 ítems y 64 MiB
   (unas 400 notificaciones de 50 KB caben de sobra).
7. **Observabilidad aditiva**: ruta nueva `GET /api/idle/queue` (protegida) con `persistent`,
   `pending`, `delivered`, `dropped`, `failed` y `oldest_pending_secs`. No se toca el contrato ya
   publicado de `GET /api/idle/status`.
8. **Sin clave nueva en el payload**: la deduplicación usa `(mailbox, uid_validity, uid)`, que ya
   está en el payload; así el contrato de notificación permanece intacto.

## Risks / Trade-offs

- *Riesgo*: reanudar tras una parada larga provoca una ráfaga (bajar todo el backlog desde el
  watermark). *Mitigación*: la cola está acotada y `dropped` lo hace visible; es el precio
  consciente del at-least-once. Un cambio de `UIDVALIDITY` resetea por seguridad.
- *Riesgo*: escribir contenido de correo a disco. *Mitigación*: solo si el operador lo configura
  explícitamente, con permisos `0600`, en un solo fichero, y validado al arranque.
- *Riesgo*: corrupción parcial del log. *Mitigación*: el log tolera una última línea incompleta;
  la compactación escribe un temporal y hace `rename` (atómico).
- *Trade-off*: hay que mantener ~180 líneas de cola propia en vez de delegar en SQLite. Se acepta
  a cambio de no imponer una dependencia con C a los consumidores del crate.
- *Trade-off*: `fsync` por mensaje añade ~1–10 ms por correo. Irrelevante a ritmo de correo y es
  lo que compra la durabilidad.
- *Trade-off*: el pico de memoria es aproximadamente **2× el payload mayor** (el clon que retiene
  `peek_oldest` durante una entrega) **más** el tope de `APIMAIL_QUEUE_MAX_BYTES` de la cola viva.
  El log se serializa desde una referencia (sin clonar el mensaje), así que persistir no duplica
  el payload; es la cota aceptada para no repartir por referencia.
