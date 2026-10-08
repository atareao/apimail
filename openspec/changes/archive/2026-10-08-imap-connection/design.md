# Design

## Context

Ver `proposal.md` — Why. `mail-account` ya modela el endpoint IMAP
(`MailEndpoint { host, port, tls, username, password }`, con
`TlsMode::{Implicit, StartTls, Plain}`) y `AppState::from_config(&Config) ->
Result<_, SmtpError>` construye el estado en `src/http.rs`. Existe el patrón de
inyección de servicios (trait `MailSender` + emisor falso) para testear sin red,
que se reutiliza aquí.

`async-imap 0.12` con la feature **`runtime-tokio`** exige que el stream
implemente `tokio::io::{AsyncRead, AsyncWrite} + Unpin + Debug + Send` (NO
`futures-io`), y **no** expone un helper `connect`: hay que hacer
`Client::new(stream)` → leer el *greeting* → `login()`.

## Goals / Non-Goals

**Goals:**
- Establecer sesión IMAP (TCP + TLS + login) con los tres modos TLS.
- Sesión perezosa y persistente, con reconexión y backoff.
- Timeout configurable.
- Estado observable por `GET /api/imap/status`.
- Testear todo **sin red**.

**Non-Goals:**
- Operaciones de buzón (listar/seleccionar/`FETCH`), `IDLE`, MIME.
- SASL/OAuth, certificados personalizados/pinning, pool de conexiones, varias cuentas.
- Verificar la conexión en el arranque (decisión explícita: no abrir red al arrancar).

## Decisions

- **`async-imap` con `runtime-tokio`** (`default-features = false`) y **rustls**
  vía `tokio-rustls` (`default-features = false, features = ["ring", "tls12",
  "logging"]`) + `webpki-roots`. Se evita `native-tls` (coherente con el resto) y
  se fija el proveedor criptográfico a **ring** explícitamente
  (`ClientConfig::builder_with_provider(rustls::crypto::ring::default_provider())`)
  para no depender del "proveedor por defecto del proceso".
- **Stream propio `ImapStream`** con variantes `Plain(TcpStream)` y
  `Tls(Box<TlsStream<TcpStream>>)`, implementando `AsyncRead`/`AsyncWrite`/`Debug`
  por delegación (`Pin::new` sobre los internos, que son `Unpin`). Se evita añadir
  `tokio-util` solo por `Either`.
  *Alternativa descartada:* `tokio_util::either::Either` (una dependencia extra);
  `Box<dyn AsyncRead + AsyncWrite>` (no es *object safe*).
- **`Client::new` + greeting + `login`** manualmente; STARTTLS como
  `run_command_and_check_ok("STARTTLS")` → `into_inner()` → handshake TLS →
  `Client::new(tls)`.
- **Gestión de sesión perezosa** en `ConnectionManager`: `tokio::sync::Mutex<Option<Box<dyn ImapSession>>>`.
  `status()` toma el lock, y:
  1. si no hay sesión, conecta;
  2. si la hay, la verifica con `NOOP`; si muere, la descarta y reconecta;
  3. aplica reintentos con **backoff exponencial** acotado.
  *Alternativa descartada:* conectar en el arranque (acopla la disponibilidad de
  la API a la del correo) o por petición (sin persistencia, contra el PLAN).
- **Backoff inyectable**: la política (`attempts`, `base`, `factor`, `max`) es un
  valor del constructor con defaults de producción; los tests usan `base = 0`
  para que la reconexión sea instantánea y determinista. Así se prueba el número
  de intentos sin dormir.
- **Timeout configurable** `APIMAIL_IMAP_TIMEOUT_SECS` (default 30, *fail-closed*)
  en `Config.imap_timeout: Duration`, aplicado con `tokio::time::timeout` a TCP
  connect, handshake TLS y `login`. La feature `time` de `tokio` se añade.
- **Abstracción inyectable**: `trait ImapSession { fn noop(&mut self) -> SendFuture<Result<(), ImapError>>; }`
  y `trait ImapConnector { fn connect(&self) -> SendFuture<Result<Box<dyn ImapSession>, ImapError>>; }`
  con futuros boxeados (mismo patrón que `MailSender`, sin `async-trait`).
  `AppState` guarda `Arc<dyn ImapConnector>` + la política; el binario construye
  `TokioImapConnector`.
- **`AppState::from_config` → `Result<_, AppStateError>`** (`AppStateError`
  envuelve el error del emisor SMTP y el de la conexión IMAP). No abre red:
  construir el `ClientConfig` de rustls no hace I/O.
- **Endpoint `GET /api/imap/status`** en el sub-router protegido:
  `200 {"connected":true,"host":..,"port":..,"tls":".."}` o
  `503 {"connected":false,"error":"imap_unavailable","message":".."}`. No expone
  credenciales.
- **Redacción**: el `Debug` de `AppState` no imprime el gestor de conexión
  (que contiene credenciales) ni el emisor SMTP.

## Risks / Trade-offs

- [No hay servidor IMAP en los tests] → Todo el camino se cubre con un conector
  falso guionizado (falla N veces, sesión muerta, etc.); la verificación contra un
  servidor real queda como prueba manual documentada. La selección TLS se aísla en
  una función pura testeable (como en `smtp-send`).
- [`async-imap 0.12` exige Rust 1.88] → Se fija `rust-version = "1.88"` en
  `Cargo.toml`; la toolchain de CI/desarrollo ya es superior.
- [Una única sesión tras un `Mutex` serializa las operaciones] → Es el modelo del
  PLAN ("una tarea por conexión") y suficiente para listar/leer; un pool llegaría
  si hiciera falta concurrencia.
- [STARTTLS cambia el tipo del stream a mitad de conexión] → Resuelto con
  `ImapStream` (se reconstruye el `Client` tras el upgrade).
- [Un `503` puede tardar hasta `timeout * intentos`] → Acotado por el número de
  intentos y el timeout; documentado.

## Migration Plan

No hay despliegue en producción. Quien quiera ajustar el timeout fija
`APIMAIL_IMAP_TIMEOUT_SECS`; por defecto 30 s. El `README` documentará el endpoint
y la variable.
