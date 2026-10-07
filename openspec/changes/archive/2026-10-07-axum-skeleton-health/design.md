# Design

## Context

Ver `proposal.md` — Why. Estado de partida: `Cargo.toml` sin dependencias, `src/main.rs` es el `Hello, world!`, edition 2024, sin tests ni infra de despliegue (el `.justfile` es heredado de otro proyecto y no aplica todavía). Este cambio es la primera piedra del backend.

## Goals / Non-Goals

**Goals:**
- Base ejecutable con Axum y Tokio, arrancable en local.
- Lógica testeable sin abrir sockets reales.
- Contrato de health estable y consumible por CI/despliegue.
- Configuración externa validada en el arranque.

**Non-Goals:**
- Cualquier operación IMAP/SMTP, autenticación, persistencia o CORS.
- Frontend, Docker/compose, CI y saneado del `.justfile` heredado.

## Decisions

- **Axum sobre actix-web / warp**: requerido por el proyecto; integra Tokio + Tower y su `Router` es composable y testeable.
- **lib + bin**: `src/lib.rs` expone `Config` y `build_router()`; `src/main.rs` solo compone y arranca. Así los tests de integración usan el `Router` con `tower::ServiceExt::oneshot`, sin puertos ni servidor reales.
- **Tokio** con features mínimas (`rt-multi-thread`, `macros`, `net`) para no inflar el binario.
- **Config por entorno** con defaults (`APIMAIL_HOST=0.0.0.0`, `APIMAIL_PORT=3000`); el puerto se parsea y un valor inválido produce `ConfigError` en arranque, nunca un fallback silencioso.
- **Nombre y versión desde `env!("CARGO_PKG_NAME")` / `env!("CARGO_PKG_VERSION")`**: única fuente de verdad, evita desincronización con `Cargo.toml`.
- **`thiserror` para errores** y **`tracing`** para logs. `tracing-subscriber` inicializa en `main.rs`.
- **Tests**: `tower` (`util`) + `http-body-util` como dev-dependencies para leer el body del `Response` en los tests.

*Alternativas descartadas:* binario único (peor testabilidad); crate `axum-test` (se prefiere `oneshot` para minimizar dependencias).

## Risks / Trade-offs

- [El `.justfile` heredado valida `"db":"connected"` en `/api/health`, algo que este cambio no produce] → No usar `just deploy*` / `just health` todavía; el health de este cambio se verifica con `cargo test` y `curl`. Rehacer `.justfile` queda para un cambio posterior.
- [Versiones de `axum`/`tokio` incompatibles con edition 2024] → Fijar la última estable y dejar `cargo check` + `cargo clippy --all-targets -- -D warnings` como puerta.
- [Puerto `3000` ocupado en local] → Es configurable por entorno y los tests no abren socket.

## Migration Plan

No aplica: primer cambio, sin API ni despliegue previos.
