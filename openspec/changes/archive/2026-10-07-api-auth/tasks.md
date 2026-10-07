# Tasks

## 1. Dependencia de comparación en tiempo constante

- [x] 1.1 Añadir `subtle` a `[dependencies]` de `Cargo.toml` y verificar con `cargo check`.

## 2. Configuración de la API key (TDD)

- [x] 2.1 RED: ampliar los tests de `Config::from_lookup` en `src/config.rs` — clave válida se carga; clave ausente → `ConfigError::MissingApiKey`; clave vacía → error. Ejecutar `cargo test` y confirmar el fallo.
- [x] 2.2 GREEN: añadir `api_key: String` a `Config`, la variante `MissingApiKey` (con `thiserror`) y la validación en `from_lookup` (tras host y port). Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios con la suite en verde.

## 3. Middleware de autorización (TDD)

- [x] 3.1 RED: crear `tests/auth.rs` construyendo el router con una clave conocida — `GET /api/whoami` con `Authorization: Bearer <key>` → 200; sin cabecera → 401; clave errónea → 401; esquema distinto (p. ej. `Basic`) → 401; `GET /api/health` sin cabecera → 200; cuerpo del 401 es JSON y hay `WWW-Authenticate: Bearer`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 3.2 GREEN: ampliar `AppState` con `api_key`, añadir `AppState::from_config`, implementar el middleware `require_api_key` (extrayendo el Bearer token y comparándolo en tiempo constante con `subtle`), el tipo de error `401` (JSON + `WWW-Authenticate`), el handler `whoami` y `build_router(state: AppState)` con el sub-router protegido por `route_layer`. Verificar `cargo test`.
- [x] 3.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios con la suite en verde.

## 4. Integración con el arranque

- [x] 4.1 Actualizar `tests/health.rs` para construir el router con un `AppState` con clave, y confirmar que sigue en verde.
- [x] 4.2 Actualizar `tests/startup.rs`: el test de puerto inválido pasa ahora `APIMAIL_API_KEY`; añadir un test de **clave ausente** (arranque con salida distinta de cero y mensaje que mencione `APIMAIL_API_KEY`) y uno de **clave vacía**.
- [x] 4.3 Reescribir `src/main.rs` para construir `AppState::from_config(&config)` y servir `build_router(state)`; verificar manualmente que sin `APIMAIL_API_KEY` el proceso falla y con ella responde `200` en `/api/health` y `401` en `/api/whoami` (curl).

## 5. Verificación de integración

- [x] 5.1 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 5.2 Confirmar que `just check-spec` pasa con este change activo.
- [x] 5.3 Documentar el contrato de `401` y el endpoint `GET /api/whoami` en el comentario de módulo de `src/http.rs`.

## 6. Cierre

- [x] 6.1 `openspec validate api-auth --strict` en verde y `openspec archive api-auth` tras la aprobación e implementación.
