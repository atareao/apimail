# Proposal

## Why

`mail-account` ya valida las credenciales IMAP y el envío SMTP funciona, pero la
API **no puede leer el buzón**: no existe ninguna conexión IMAP. Es la base de
todo el árbol de lectura (`imap-mailboxes`, `imap-messages`, `imap-flags`,
`mime-parsing` e `imap-idle`); sin ella ninguno de esos changes puede empezar.

## What Changes

- Nueva capability `imap-connection`: establecimiento de la sesión IMAP
  (TCP + TLS + `LOGIN`) con **`async-imap`** y **rustls**.
- **Sesión persistente perezosa**: se conecta en el primer uso, se reutiliza y se
  **reconecta con backoff exponencial acotado** si la sesión muere. **No se abre
  red en el arranque** (el servicio arranca aunque el correo esté caído).
- Soporta los tres modos TLS ya definidos en `mail-account`
  (`implicit`, `starttls`, `none`).
- Nueva variable **`APIMAIL_IMAP_TIMEOUT_SECS`** (por defecto **30**,
  *fail-closed* si es `0` o no numérica) aplicada a TCP connect, handshake TLS y
  `LOGIN`.
- Nuevo endpoint protegido **`GET /api/imap/status`**: intenta conectar y
  responde `200 {"connected":true,...}` o `503 {"connected":false,...}`.
- La capa de conexión es **inyectable** (trait) para poder testear sin red.
- Los secretos IMAP **nunca** aparecen en logs, `Debug` ni respuestas.

## Capabilities

### New Capabilities
- `imap-connection`: establecimiento, reutilización y reconexión de la sesión
  IMAP con TLS, timeout configurable y endpoint de estado.

### Modified Capabilities
<!-- Ninguna: mail-account y el resto mantienen sus requisitos. -->

## Impact

- `Cargo.toml`: `async-imap` (`runtime-tokio`), `tokio-rustls` (`ring`/`tls12`/`logging`),
  `webpki-roots`; feature `time` de `tokio`; y `rust-version = "1.88"` (lo exige
  `async-imap 0.12`).
- `src/config.rs`: `Config.imap_timeout` y su validación.
- `src/imap.rs` (nuevo): `ImapStream`, traits `ImapSession`/`ImapConnector`,
  `TokioImapConnector` y `ConnectionManager`.
- `src/http.rs`: `AppState` incorpora el gestor de conexión; handler
  `GET /api/imap/status`; nuevo tipo de error de construcción del estado.
- `src/lib.rs`: reexportar los tipos de la conexión.
- `src/main.rs`: tratar el nuevo error de arranque.
- Tests: `tests/imap_status.rs` (conector falso, sin red) y ajustes en los
  existentes; unitarios de la estrategia TLS, la reconexión y el timeout.
- Habilita `imap-mailboxes`, `imap-messages` y el resto del árbol IMAP.
- **Fuera de alcance**: operaciones de buzón (listar/seleccionar), `IDLE`, SASL/OAuth,
  certificados personalizados/pinning, pool de conexiones y varias cuentas.
