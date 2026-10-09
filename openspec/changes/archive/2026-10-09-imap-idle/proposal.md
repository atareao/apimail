# Proposal

## Why

Con `imap-messages`, `imap-flags` y `mime-parsing` la API sabe leer, mutar e interpretar el
correo, pero solo **a petición**: para enterarse de un correo nuevo hay que sondear. La
extensión `IDLE` (RFC 2177) permite que el **servidor** notifique los cambios del buzón en
cuanto ocurren. Esta capability cierra el MVP convirtiendo la API en **reactiva**: al llegar
un correo, se hace `POST` de su contenido y metadatos a un webhook configurable, que es
justo el caso de uso para el que se diseñó el roadmap (automatizaciones, bandejas triage,
integraciones).

## What Changes

- Nueva capability `imap-idle`.
- **Control de la suscripción** (rutas protegidas e idempotentes):
  `POST /api/idle/start` → `{"status":"running","mailbox":"INBOX"}`;
  `POST /api/idle/stop` → `{"status":"stopped"}`;
  `GET /api/idle/status` → `{"status","mailbox","last_error"}`.
  Sin webhook configurado, `start` responde `501 idle_not_configured`.
- **Conexión propia dedicada**: `Session::idle()` de `async-imap` **consume** la sesión, así
  que la suscripción abre su **propia** conexión TLS+LOGIN (reutilizando la rutina de
  conexión existente) y **nunca** bloquea la sesión compartida del resto de las rutas.
- **Detección de correo nuevo**: al arrancar se fija `last_uid = UIDNEXT - 1` (solo se
  notifica el correo que llega **después**); en cada despertar de `IDLE` se hace
  `UID FETCH <last_uid+1>:*` en formato `Full` y se notifican los mensajes en orden
  ascendente de `UID`.
- **Notificación al webhook**: `POST` JSON (con **reintentos acotados**: 1 intento + 2
  reintentos con backoff corto) con `mailbox`, `uid`, `uid_validity`, `flags`, `size`,
  `internal_date`, `envelope`, `parsed` y, cuando el mensaje cabe en el límite y se puede
  parsear, `text`, `html` y `attachments` (**solo metadatos**: `id`, `filename`,
  `content_type`, `size`, `inline`, `content_id`; sin bytes).
- **Resiliencia**: reemisión de `IDLE` al menos cada 29 min, reconexión con backoff ante
  caídas y continuación ante fallos del webhook (`last_error` con **código estable**, sin
  texto de terceros).
- **Configuración**: `APIMAIL_WEBHOOK_URL` (opcional; sin él, `start` → `501`),
  `APIMAIL_IDLE_MAILBOX` (por defecto `INBOX`) y `APIMAIL_WEBHOOK_TIMEOUT_SECS` (por defecto
  10). El parseo se acota con el `APIMAIL_MAX_MESSAGE_BYTES` ya existente.
- Errores JSON (`{"error","message"}`): `401 unauthorized` sin API key y `501
  idle_not_configured` cuando `start` se pide sin webhook. Los fallos de IMAP **no**
  devuelven código HTTP: se absorben en `last_error` (`imap_unavailable`) mientras la
  suscripción sigue viva y reintenta.

## Capabilities

### New Capabilities
- `imap-idle`: suscripción `IDLE` sobre una conexión dedicada que notifica al webhook el
  correo nuevo con sus metadatos y contenido parseado.

### Modified Capabilities
<!-- Ninguna: `imap-connection`, `imap-messages`, `imap-flags`, `mime-parsing` y `http-api`
conservan sus contratos; `imap-idle` añade rutas y reutiliza `fetch`/`mime` sin modificarlos. -->

## Impact

- `Cargo.toml`: nueva dependencia `reqwest` (solo rustls, sin `native-tls`) para el cliente
  HTTP del webhook.
- `src/idle.rs` (**nuevo**): traits `IdleConnector`/`IdleSession`/`WebhookSender` y sus impls
  reales (`TokioIdleConnector`/`TokioIdleSession`/`HttpWebhookSender`), `IdleSupervisor`
  (arranque/parada/estado + bucle con backoff) y el `WebhookPayload` tipado.
- `src/imap.rs`: extraer la rutina de conexión+LOGIN (hoy dentro de `TokioImapConnector`) a
  una función compartida para reutilizarla desde la conexión dedicada de `IDLE`.
- `src/config.rs`: `webhook_url`, `idle_mailbox`, `webhook_timeout` (+ variantes de
  `ConfigError`, incluida la validación de la URL).
- `src/http.rs`: `AppState.idle` (`Arc<IdleSupervisor>`) y los handlers/rutas
  `POST /api/idle/start`, `POST /api/idle/stop`, `GET /api/idle/status`; variante
  `IdleError`/`IdleErrorResponse` con `501 idle_not_configured`.
- `src/lib.rs`: re-exports de los tipos nuevos.
- `tests/idle.rs` (nuevo) y tests unitarios de `src/idle.rs`/`src/config.rs`.
- `README.md`: documentar los tres endpoints y las tres variables nuevas.
- **Fuera de alcance**: garantía *at-least-once* con cola persistente (se usa *at-most-once*
  con reintentos acotados); contenido de adjuntos en el payload; suscripción a varios
  buzones o varias cuentas a la vez; el correo crudo en el payload; `IDLE` sobre la sesión
  compartida.
