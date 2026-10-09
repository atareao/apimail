# Tasks

## 1. Dependencias y configuración

- [x] 1.1 Añadir a `Cargo.toml`: `async-imap` (`default-features = false, features = ["runtime-tokio"]`), `tokio-rustls` (`default-features = false, features = ["ring","tls12","logging"]`), `webpki-roots`, la feature `time` de `tokio` y `rust-version = "1.88"`. Verificar con `cargo check`.
- [x] 1.2 RED: en `src/config.rs`, tests de `APIMAIL_IMAP_TIMEOUT_SECS` — ausente → 30 s; valor válido se usa; `0` y no numérico → error que nombra la variable. Ejecutar `cargo test` y confirmar el fallo.
- [x] 1.3 GREEN: `Config.imap_timeout: Duration`, la constante de defecto y la variante de error; encadenar la validación. Verificar `cargo test`.
- [x] 1.4 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Capa de conexión (dominio, TDD)

- [x] 2.1 RED: en `src/imap.rs`, tests unitarios — `tls_strategy(TlsMode)` mapea los tres modos; `ConnectionManager` con un `ImapConnector` falso (guionizado): conecta en el primer uso y **reutiliza** (un solo `connect`); una sesión muerta (`noop` falla) se descarta y reconecta; si todos los intentos fallan → error tras un número **acotado** de intentos (usa backoff `base = 0` para que sea instantáneo); `timeout` inválido ya cubierto en config. Ejecutar `cargo test` y confirmar el fallo.
- [x] 2.2 GREEN: implementar `ImapStream` (`Plain`/`Tls`, `AsyncRead`/`AsyncWrite`/`Debug` por delegación), `ImapError`, los traits `ImapSession`/`ImapConnector` (futuros boxeados), `TokioImapConnector` (TCP + TLS implícito / STARTTLS / sin TLS + *greeting* + `login`, con `tokio::time::timeout`) y `ConnectionManager` (`tokio::sync::Mutex`, perezoso, verificación por `NOOP`, backoff inyectable). Verificar `cargo test`. Nota de cobertura: los tres modos TLS reales (`implicit`/`starttls`/`none`) y la aplicación efectiva del timeout se verifican en manual/integración; los tests unitarios solo cubren el mapeo `tls_strategy` y la validación de `APIMAIL_IMAP_TIMEOUT_SECS`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Endpoint `GET /api/imap/status` (TDD)

- [x] 3.1 RED: crear `tests/imap_status.rs` con un `ImapConnector` falso inyectado en `AppState` — conexión OK → `200 application/json` con `connected:true`, `host`, `port` y `tls`; conector que falla → `503` con `connected:false` y mensaje; sin API key → `401`. Ejecutar `cargo test` y confirmar el fallo. Nota de cobertura: al usar un conector falso, la verificación del `tls` reportado es la del mapeo de configuración; el handshake TLS real de los tres modos y la aplicación efectiva del timeout se verifican en manual/integración.
- [x] 3.2 GREEN: añadir a `AppState` el `Arc<dyn ImapConnector>` (y la política/gestor), el handler `imap_status` y su montaje en el sub-router protegido; `AppStateError` envolviendo los errores del emisor SMTP y de la conexión; constructor para inyectar el conector en tests. `AppState::from_config` construye el `TokioImapConnector` real **sin abrir red**. Verificar `cargo test`.
- [x] 3.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios; documentar el contrato del endpoint en el comentario de módulo de `src/http.rs`.

## 4. Integración con el arranque

- [x] 4.1 Ajustar `src/main.rs` al nuevo tipo de error de `AppState::from_config` (fallo → `ExitCode::FAILURE` con mensaje).
- [x] 4.2 Ajustar los tests existentes (`tests/health.rs`, `tests/auth.rs`, `tests/account.rs`, `tests/send.rs`) al nuevo constructor sin debilitar aserciones.
- [x] 4.3 Añadir a `tests/startup.rs` un caso de `APIMAIL_IMAP_TIMEOUT_SECS` inválido → salida ≠ 0 y stderr menciona la variable; y un caso de **servidor IMAP inalcanzable**: el servicio **arranca** igualmente (no se abre red en el arranque).

## 5. Verificación de integración

- [x] 5.1 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 5.2 Confirmar que `just check-spec` pasa con este change activo.

## 6. Documentación y cierre

- [x] 6.1 Documentar `GET /api/imap/status` y `APIMAIL_IMAP_TIMEOUT_SECS` en `README.md`.
- [x] 6.2 `openspec validate imap-connection --strict` en verde y `openspec archive imap-connection` tras la aprobación e implementación.
