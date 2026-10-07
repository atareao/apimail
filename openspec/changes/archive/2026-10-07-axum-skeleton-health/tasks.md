# Tasks

## 1. Dependencias y estructura lib + bin

- [x] 1.1 Añadir a `Cargo.toml` las dependencias (`axum`, `tokio` con `rt-multi-thread`/`macros`/`net`, `serde`, `serde_json`, `tracing`, `tracing-subscriber`, `thiserror`) y las dev-dependencies de test (`tower` con `util`, `http-body-util`); verificar con `cargo check`.
- [x] 1.2 Crear `src/lib.rs` declarando los módulos (`config`, rutas/http) y verificar que `cargo check --all-targets` compila lib y bin.

## 2. Configuración de arranque (TDD)

- [x] 2.1 RED: escribir tests unitarios de `Config::from_env` en `#[cfg(test)]` (defaults, override válido, puerto inválido) y verificar que fallan con `cargo test`.
- [x] 2.2 GREEN: implementar `Config` y `ConfigError` (con `thiserror`) en `src/config.rs` y verificar que los tests pasan.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` sin avisos, con la suite en verde.

## 3. Endpoint de health (TDD)

- [x] 3.1 RED: crear `tests/health.rs` que construya el router y use `tower::ServiceExt::oneshot` para `GET /api/health` (200 + JSON), `GET /api/unknown` (404) y `POST /api/health` (405); verificar que fallan con `cargo test`.
- [x] 3.2 GREEN: implementar el handler de health y `build_router()` con el estado de la app (nombre/versión) y verificar que los tests de integración pasan.
- [x] 3.3 REFACTOR: dejar `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios con toda la suite verde.
- [x] 3.4 Documentar el contrato de `/api/health` (comentario de módulo) y verificar que el JSON de ejemplo coincide con la respuesta real de los tests.

## 4. Entrypoint binario

- [x] 4.1 Reescribir `src/main.rs` para cargar `Config`, inicializar `tracing` y servir el router con `axum::serve`; verificar `curl -fsS http://127.0.0.1:3000/api/health` devuelve `status ok` sobre `cargo run`.
- [x] 4.2 Verificar el fallo limpio de arranque con `APIMAIL_PORT=abc cargo run` (salida con error descriptivo y código distinto de cero).

## 5. Verificación de integración

- [x] 5.1 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 5.2 Confirmar que `just check-spec` pasa con este change activo.
