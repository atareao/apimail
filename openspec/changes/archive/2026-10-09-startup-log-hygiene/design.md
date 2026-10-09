# Design

## Context

- `ImapError` (`src/imap.rs:87`) y `SmtpError` (`src/smtp.rs:119`) ya exponen
  `public_message() -> &'static str` y `kind() -> &'static str`. `src/http.rs` los usa para
  construir las respuestas de error y `src/imap.rs` usa `kind()` en sus logs.
- `AppStateError` (`src/http.rs:345`) es `#[error(transparent)]` hacia `SmtpError`/`ImapError`.
  `ConfigError` es `#[error(transparent)]` hacia `AccountError` para la cuenta de correo.
- `src/main.rs:24` y `src/main.rs:44` registran el `Display` de esos errores.
- `tests/startup.rs` ya afirma que `stderr` **menciona la variable** afectada (aserciones en
  las líneas 76, 97, 134, 156, 177, 200 y 223): eso debe seguir cumpliéndose.

## Goals / Non-Goals

**Goals**
- Que ningún log de arranque pueda contener un valor de variable de entorno, una credencial o
  texto de terceros.
- Mantener la utilidad de diagnóstico: el mensaje público **nombra la variable** afectada
  (nuestro propio identificador, no un secreto) y `kind()` da una etiqueta estable.

**Non-Goals**
- No cambia el `Display` de los errores: sigue siendo el detalle completo para `Debug` y
  depuración en proceso.
- No cambia las respuestas HTTP: esas ya usan `public_message()`.
- No toca los logs de `bind`/`serve` (texto de `std::io::Error`, sin terceros ni secretos) ni
  el `eprintln!` de inicialización de `tracing`.
- No introduce un redactor genérico de logs.

## Decisions

1. **`public_message() -> String`** (y no `&'static str`) en `ConfigError` y `AccountError`,
   porque el mensaje debe incluir el **nombre** de la variable
   (`MissingValue{name}`/`InvalidPort{name}`/`InvalidTls{name}`), que es dinámico y forma parte
   de nuestra propia API de configuración. El **valor** nunca se incluye.
   `AppStateError::public_message() -> String` delega (`SmtpError`/`ImapError` devuelven
   `&'static str`; se convierten con `to_string()`), de modo que la frontera nueva tiene un
   tipo uniforme.
2. **`kind() -> &'static str`** con etiquetas por variante, siguiendo el patrón ya establecido
   en `SmtpError::kind()`/`ImapError::kind()`:
   - `AccountError`: `missing_value`, `invalid_port`, `invalid_tls`.
   - `ConfigError`: `invalid_port`, `missing_api_key`, `invalid_max_attachment_bytes`,
     `invalid_max_message_bytes`, `invalid_webhook_url`, `invalid_webhook_timeout`,
     `invalid_imap_timeout`, `account`.
   - `AppStateError`: `smtp`, `imap`.
3. **`AppStateError` delega** en `SmtpError::public_message()`/`ImapError::public_message()`:
   el `#[error(transparent)]` se conserva para `Display`/`Debug`, pero el log usa el mensaje
   público de la familia interna.
4. **`main.rs`** pasa a
   `tracing::error!(kind = error.kind(), "failed to start apimail: {}", error.public_message())`
   y análogo para los servicios. `kind` como campo estructurado permite filtrar; el mensaje
   conserva el nombre de la variable.
5. **Se conservan las aserciones de `tests/startup.rs`** sobre el nombre de variable (el
   mensaje público lo incluye) y se añade una aserción sobre la ausencia del valor.

## Risks / Trade-offs

- *Riesgo*: perder detalle de diagnóstico en los logs. *Mitigación*: se conservan el nombre de
  la variable y `kind()`; el detalle completo sigue disponible vía `Display`/`Debug`.
- *Riesgo*: confundir «mensaje público» con «mensaje para el cliente HTTP». *Mitigación*:
  documentar que `public_message()` es seguro para logs y clientes.
- *Trade-off*: `String` asigna en la ruta de error. Irrelevante: ocurre una vez, al arrancar.

## Migration Plan

- Aditivo: superficie nueva + dos líneas de log. Sin cambios de contrato HTTP ni de
  configuración, sin migración de datos. Reversible.
- Rollback: revertir la rama `fix/startup-log-hygiene`.
