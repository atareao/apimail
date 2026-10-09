# Proposal

## Why

El requisito de higiene de errores del proyecto es explícito: **los mensajes de fallo nunca
deben filtrar credenciales ni texto de terceros**. `ImapError` y `SmtpError` ya exponen
`public_message()`/`kind()` y la capa HTTP los usa. Pero el **entrypoint** (`src/main.rs`) es
el último sitio que todavía registra el `Display` crudo de errores de dominio: ante un fallo
de `AppState::from_config`, `AppStateError::Smtp` es `#[error(transparent)]` hacia
`SmtpError::Transport`, cuyo `Display` interpola el error crudo de `lettre` (que puede incluir
la respuesta del servidor SMTP). Es una fuga de texto de terceros a los logs, y la primera
release pública (`v0.2.0`) no debería salir con ella.

## What Changes

- Nueva capability `startup-logging`.
- `AccountError`, `ConfigError` y `AppStateError` exponen `public_message() -> String`
  (estable; **sin** valores de variables de entorno, credenciales ni texto de terceros; puede
  nombrar la variable afectada, que es nuestro propio identificador) y `kind() -> &'static str`
  (etiqueta estable para filtrar en logs).
- `AppStateError::public_message()` **delega** en `SmtpError`/`ImapError`, de modo que un error
  envuelto de `lettre`/`async-imap`/rustls nunca llega a un log a través de `Display`.
- `src/main.rs` registra los fallos de configuración y de inicialización de servicios con
  `kind` + `public_message()` en lugar de `Display`.
- Los tests de `tests/startup.rs` que comprueban que el nombre de la variable aparece en
  `stderr` **siguen pasando** (el mensaje público nombra la variable, nunca el valor); se añade
  una aserción sobre la **ausencia del valor**.

## Capabilities

### New Capabilities
- `startup-logging`: mensajes públicos estables para los fallos de arranque y logs del
  entrypoint libres de valores de configuración y de texto de terceros.

### Modified Capabilities
<!-- Ninguna: los contratos de `http-api`, `api-auth`, `mail-account`, etc. no cambian; solo
se añade superficie pública de diagnóstico y se cambian dos líneas de log del binario. -->

## Impact

- `src/config.rs`: `AccountError::public_message()/kind()` y
  `ConfigError::public_message()/kind()`.
- `src/http.rs`: `AppStateError::public_message()/kind()`.
- `src/main.rs`: las dos líneas de log (configuración y servicios) pasan a usar
  `kind` + `public_message()`.
- `tests/startup.rs`: un test nuevo que verifica que el **valor** inválido no aparece en
  `stderr` (y que el nombre de la variable sí).
- Tests unitarios en `src/config.rs` y `src/http.rs`.
- **Fuera de alcance**: los logs de `bind`/`axum::serve` (usan el texto estándar de
  `std::io::Error`, sin secretos ni texto de protocolo de terceros) y el `eprintln!` de
  inicialización de `tracing`.
