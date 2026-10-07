# Tasks

## 1. Modelo de la cuenta y errores

- [x] 1.1 RED: en `src/config.rs`, escribir tests unitarios de `MailAccount::from_lookup` — config completa válida; TLS por defecto `implicit`; parseo de `starttls`/`none` (case-insensitive); `none` carga; valor TLS inválido → `AccountError::InvalidTls`; puertos por defecto por TLS (IMAP 993/143/143, SMTP 465/587/587); `PORT` explícito gana; `PORT` inválido o `0` → `AccountError::InvalidPort`; host/usuario/secreto ausente o vacío → `AccountError::MissingValue`; `Debug` redacta los secretos. Ejecutar `cargo test` y confirmar el fallo.
- [x] 1.2 GREEN: implementar `TlsMode`, `MailEndpoint`, `MailAccount` y `AccountError` (`thiserror`), con `Debug` redactado y el *warning* de `tracing` para `none`. Verificar `cargo test`.
- [x] 1.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios con la suite en verde.

## 2. Integración en `Config`

- [x] 2.1 RED: tests en `src/config.rs` — `Config::from_lookup` carga `account` correctamente; un `AccountError` se propaga como `ConfigError::Account`; el `Debug` de `Config` no filtra secretos de correo. Ejecutar `cargo test` y confirmar el fallo.
- [x] 2.2 GREEN: añadir `Config.account`, ampliar `ConfigError` con `Account(#[from] AccountError)` y encadenar la carga de la cuenta tras host → port → api key. Reexportar los tipos en `src/lib.rs`. Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Endpoint `GET /api/account` (TDD)

- [x] 3.1 RED: crear `tests/account.rs` construyendo el router con clave y cuenta conocidas — `GET /api/account` con `Bearer` válido → `200`, JSON con `imap`/`smtp` `host` y `port`; el cuerpo **no** contiene usuario ni secreto; sin cabecera → `401`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 3.2 GREEN: añadir la vista pública (`AccountView`/`EndpointView`) a `AppState` en `AppState::from_config`, el handler `account` y su montaje en el sub-router protegido de `build_router`. Verificar `cargo test`.
- [x] 3.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios; documentar el contrato del endpoint en el comentario de módulo de `src/http.rs`.

## 4. Integración con el arranque

- [x] 4.1 Actualizar `tests/health.rs` y `tests/auth.rs` para aportar una cuenta completa al construir el estado/router; confirmar verde.
- [x] 4.2 Ampliar `tests/startup.rs`: un arranque sin variables de cuenta falla con salida distinta de cero y mensaje que mencione la variable que falta; caso de puerto de correo inválido y de TLS inválido.
- [x] 4.3 Verificar manualmente con `curl` que, con la cuenta configurada, `GET /api/account` responde `200` con host/puerto y que el proceso falla al arrancar sin las credenciales.

## 5. Verificación de integración

- [x] 5.1 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 5.2 Confirmar que `just check-spec` pasa con este change activo.

## 6. Documentación y cierre

- [x] 6.1 Documentar las variables `APIMAIL_IMAP_*`/`APIMAIL_SMTP_*`, los puertos por defecto y el endpoint `GET /api/account` en `README.md`.
- [x] 6.2 `openspec validate mail-account --strict` en verde y `openspec archive mail-account` tras la aprobación e implementación.
