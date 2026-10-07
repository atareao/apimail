# Proposal

## Why

`apimail` está greenfield: `Cargo.toml` no tiene dependencias y `src/main.rs` es el `Hello, world!` por defecto. Antes de construir el CRUD de correo (IMAP/SMTP) hace falta una base ejecutable y verificable sobre la que apoyar los cambios siguientes: un servidor Axum que arranque desde configuración y exponga un health check.

## What Changes

- Añadir dependencias base: `axum`, `tokio` (runtime async), `serde`/`serde_json`, `tracing`/`tracing-subscriber` y `thiserror`; dev-dependencies de test (`tower` con `util`, `http-body-util`).
- Pasar de un binario suelto a estructura **lib + bin**: `src/lib.rs` expone `Config` y `build_router()`; `src/main.rs` compone y arranca.
- Nuevo endpoint `GET /api/health` que responde `200` con JSON (estado, nombre y versión de la app).
- Arranque configurable por variables de entorno (`APIMAIL_HOST`, `APIMAIL_PORT`) con valores por defecto y fallo explícito ante valores inválidos.

No hay **BREAKING**: no existe API previa.

## Capabilities

### New Capabilities
- `http-api`: servidor HTTP Axum con configuración de arranque y endpoint de salud.

### Modified Capabilities
<!-- Ninguna: no hay specs existentes en el proyecto. -->

## Impact

- `Cargo.toml`: primeras dependencias del proyecto.
- `src/main.rs` (reescrito), nuevos `src/lib.rs`, `src/config.rs` y módulo de rutas.
- Primer test de integración en `tests/`.
- Habilita los cambios posteriores (`imap-access`, `smtp-send`, adjuntos, IDLE/webhook).
- Fuera de alcance: operaciones de correo, autenticación, persistencia, frontend, Docker/compose y saneado del `.justfile` heredado.
