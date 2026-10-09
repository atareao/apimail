# Design

## Context

`imap-connection` provee una **sesión compartida perezosa** (`ConnectionManager` con un
`tokio::sync::Mutex<Option<Box<dyn ImapSession>>>`) sobre la que corren todas las rutas.
Pero `async-imap 0.12` expone la extensión `IDLE` así:

```rust
Session::idle(self) -> Handle<T>;                       // CONSUME la sesión (toma ownership)
Handle::init(&mut self) -> Result<()>;                  // envía IDLE, espera "+"
Handle::wait_with_timeout(dur) -> (impl Future<Output = Result<IdleResponse>>, StopSource);
Handle::done(self) -> Result<Session<T>>;               // envía DONE, devuelve la sesión
enum IdleResponse { NewData(ResponseData), Timeout, ManualInterrupt }
```

Dos consecuencias de diseño:

1. **`idle()` consume la sesión**, así que `IDLE` no puede compartir la sesión del
   `ConnectionManager` sin bloquear **todo** el resto de la API mientras dure la espera
   (potencialmente 29 min). La suscripción necesita, por tanto, una **conexión propia**.
2. `IdleResponse::NewData` lleva un `ResponseData` que en `async-imap` es **`pub(crate)`**
   (no accesible desde fuera), luego **no se puede leer el `EXISTS`** del evento: solo se
   sabe que *algo* cambió. Por eso, tras cada despertar, la suscripción **vuelve a
   consultar** el buzón (`UID FETCH <last+1>:*`) en lugar de fiarse del contenido del evento.

`Config` ya tiene el patrón de límites/tiempos numéricos (`max_attachment_bytes`,
`imap_timeout`, `max_message_bytes`) y `mime::parse_message` reutilizable.

## Goals / Non-Goals

**Goals:**
- Suscripción `IDLE` sobre una **conexión dedicada** que no interfiera con el resto de rutas.
- Arranque/parada/estado controlables e **idempotentes** desde la API.
- Notificar por `POST` al webhook cada correo nuevo (solo los posteriores al arranque) con
  metadatos + contenido parseado (texto/HTML/metadatos de adjuntos).
- Resiliencia: reemisión de `IDLE`, reconexión con backoff, fallo de webhook que no tumba la
  suscripción.
- Errores/estado estables y sin secretos ni texto de terceros; todo testeable sin red.

**Non-Goals:**
- *At-least-once* con cola persistente (se asume *at-most-once* con reintentos acotados).
- Contenido de adjuntos en el payload (solo metadatos).
- Suscripción a varios buzones/cuentas, ni `IDLE` sobre la sesión compartida.
- Correo crudo en el payload; escritura a disco.

## Decisions

- **Modelo de control: `start`/`stop`/`status` idempotentes** — control en caliente sin
  reiniciar el servicio. `start` con la suscripción ya activa devuelve `200` y **no** abre
  una segunda conexión; `stop` es idempotente y, de forma acotada, para la suscripción y
  cierra su conexión dedicada (el `DONE` se envía en el camino normal de timeout de `IDLE`).
- **Detección de correo nuevo por `UID`, no por el evento** — por la limitación de
  `ResponseData`, cada despertar (o timeout) re-consulta `UID FETCH <last_uid+1>:*` en
  formato `Full` (que ya trae `UID FLAGS RFC822.SIZE INTERNALDATE ENVELOPE BODY.PEEK[]`). Es
  robusto ante eventos perdidos y ante reconexiones.
- **Solo correo posterior al arranque** — `last_uid = UIDNEXT.saturating_sub(1)` obtenido del
  `SELECT` inicial (barato). Si el servidor no informa `UIDNEXT`, se parte de `0` (puede
  notificar el buzón existente); se documenta.
- **Payload tipado y puro** — `WebhookPayload::from_message(mailbox, uid_validity, &Message,
  Option<&ParsedMessage>)`, construido solo con datos del servidor y del parseo MIME; los
  adjuntos se listan **sin** su contenido. `parsed=false` (texto/HTML nulos, adjuntos vacíos)
  cuando el mensaje supera `APIMAIL_MAX_MESSAGE_BYTES` o no se puede parsear; los metadatos
  se envían igualmente.
- **Conexión dedicada** — traits `IdleConnector`/`IdleSession` (análogos a
  `ImapConnector`/`ImapSession`) con impl real `TokioIdleConnector`/`TokioIdleSession` y
  fakes en tests. La rutina de conexión+LOGIN hoy embebida en `TokioImapConnector` se
  extrae a una función compartida y la reutilizan ambas. `IdleSession` expone solo lo que el
  supervisor necesita: `select`, `wait_for_change(timeout)` (hace `init`+`wait`+`DONE`
  internamente y devuelve `Changed`/`TimedOut`) y `fetch_from(from_uid, format)`.
- **`wait_for_change` encapsula `init`/`wait`/`DONE`** — al terminar cada espera la sesión
  vuelve a estar libre para el `UID FETCH`; el bucle reemite `IDLE` con un timeout acotado
  (29 min por defecto, inyectable en tests), cumpliendo la recomendación de RFC 2177.
- **Entrega: *at-most-once* con reintentos acotados** — 1 intento + 2 reintentos con backoff
  corto; solo se reintenta lo reintentable (error de transporte o `5xx`/`429`). Si se agota,
  se registra `last_error = "webhook_failed"` (código estable) y la suscripción continúa. No
  hay cola persistente.
- **Cliente HTTP del webhook: `reqwest`** con `default-features = false, features =
  ["rustls-tls-webpki-roots", "json"]` (coherente con el rustls/`webpki-roots` del resto del
  proyecto; sin `native-tls`). `HttpWebhookSender` construye su `reqwest::Client` de forma
  perezosa (una sola vez) para que su creación sea infalible y no complique `AppState`.
- **Configuración** — `APIMAIL_WEBHOOK_URL` (opcional; **validada al arranque**: esquema
  `http`/`https` y host no vacío → si no, `ConfigError`), `APIMAIL_IDLE_MAILBOX` (por defecto
  `INBOX`), `APIMAIL_WEBHOOK_TIMEOUT_SECS` (por defecto 10). El parseo se acota con
  `APIMAIL_MAX_MESSAGE_BYTES`.
- **`AppState`** gana `idle: Arc<IdleSupervisor>`. `with_services` (3 args) se mantiene y
  construye las piezas reales de forma **infalible**; se añade `with_idle(...)` (con
  connector+webhook inyectables) para los tests de `imap-idle`. Los call sites existentes no
  cambian.
- **`last_error` como código estable** — `null`, `"imap_unavailable"` o `"webhook_failed"`;
  nunca `Display` de un error de terceros. Los logs tampoco filtran credenciales ni texto de
  terceros.

## Risks / Trade-offs

- [Eventos IDLE ilegibles] → no se puede saber *qué* cambió (tipo privado); se mitiga
  re-consultando `UID FETCH <last+1>:*`, robusto ante eventos perdidos.
- [Ventana entre `DONE` y el siguiente `IDLE`] → durante ese hueco podría perderse un
  `EXISTS`; la re-consulta por `UID` lo recupera en el siguiente ciclo.
- [Sin `UIDNEXT`] → se parte de `0` y podría notificarse el buzón existente; documentado, y
  la mayoría de servidores sí lo anuncian.
- [*At-most-once*] → un `POST` puede perderse tras agotar reintentos con el webhook caído; se
  compensa con `last_error` visible y se deja *at-least-once* como trabajo futuro.
- [Tarea de fondo en tests] → fakes guionizados (`IdleSession` que devuelve eventos y
  mensajes prefijados, `WebhookSender` que registra payloads) y parada explícita por el
  `stop`; sin red y sin depender de tiempos reales.
- [`reqwest` como nueva dependencia] → se limita a rustls/`webpki-roots` para no arrastrar
  `native-tls`/OpenSSL.

## Migration Plan

No hay despliegue en producción. `README.md` documentará los tres endpoints y las tres
variables nuevas. La suscripción no arranca sola: hay que llamar a `POST /api/idle/start`.
