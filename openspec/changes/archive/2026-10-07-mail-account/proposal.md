# Proposal

## Why

Tras `api-auth` la API está protegida, pero todavía **no sabe a qué cuenta de
correo conectarse**: no existe ninguna noción de servidor IMAP/SMTP ni de
credenciales. Todo el roadmap (IMAP, MIME, SMTP, IDLE) necesita, como base común,
una cuenta configurada y validada. Sin ella la siguiente capability
(`imap-connection`) no tendría a qué conectarse, y un error de configuración
(p. ej. un puerto inválido o una variable ausente) se descubriría tarde y de
forma confusa.

## What Changes

- Nueva capability `mail-account`: la cuenta de correo (endpoint **IMAP** y
  endpoint **SMTP**) se define por **variables de entorno** (host, puerto, modo
  TLS, usuario y secreto).
- **Fail-closed**: el servicio **no arranca** si falta o está vacío cualquier
  valor obligatorio —host, usuario o secreto— de IMAP o SMTP.
- **Modo TLS** configurable por endpoint: `implicit` (por defecto), `starttls` o
  `none` (permitido con *warning* en logs, útil para pruebas locales).
- **Puertos por defecto derivados del modo TLS**: IMAP `993`/`143`/`143`
  (implicit/starttls/none) y SMTP `465`/`587`/`587`.
- Validación al arranque de puertos (`1..=65535`) y valores no vacíos, con
  errores descriptivos.
- Los secretos **nunca** aparecen en logs ni en la salida `Debug` (redactados).
- Nuevo endpoint protegido `GET /api/account` que expone **solo host y puerto**
  de IMAP y SMTP (sin usuario, sin secreto).

Cambio de API interna (no publicado como contrato estable): `Config` gana el
campo `account` y `AppState` incorpora una vista pública de la cuenta (host y
puerto) para el endpoint.

## Capabilities

### New Capabilities
- `mail-account`: configuración y validación de la cuenta IMAP/SMTP por entorno,
  protección de secretos y endpoint `GET /api/account`.

### Modified Capabilities
- `http-api`: el requisito *Server startup and configuration* pasa a exigir
  también una cuenta de correo válida (IMAP y SMTP) para arrancar.

## Impact

- `src/config.rs`: tipos `TlsMode`, `MailEndpoint`, `MailAccount`, `AccountError`;
  `Config.account`; nuevas variantes en `ConfigError`; `Debug` redactando secretos.
- `src/http.rs`: `AppState` con la vista pública de la cuenta y handler
  `GET /api/account` montado en el sub-router protegido.
- `src/lib.rs`: reexportar los tipos públicos de la cuenta.
- Tests: unitarios en `config.rs`; nuevo `tests/account.rs`; `tests/startup.rs`,
  `tests/health.rs` y `tests/auth.rs` actualizados para aportar una cuenta completa.
- Habilita `imap-connection` y, después, `smtp-send`.
- **Fuera de alcance**: conexión de red, verificación de credenciales contra el
  servidor, múltiples cuentas, OAuth, rotación de secretos y TLS de terminación.
