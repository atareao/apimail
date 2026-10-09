# AGENTS.md — apimail

> Proyecto en construcción. Verifica el estado real antes de asumir que algo existe.

## Estado actual (verificado)
- `Cargo.toml` (Rust **edition 2024**) ya tiene las dependencias base: `axum`, `tokio`, `serde`/`serde_json`, `tracing`/`tracing-subscriber`, `thiserror`; dev: `tower`, `http-body-util`.
- API implementada en estructura **lib + bin**: `src/lib.rs` (`Config`, `MailAccount`, `build_router`), `src/config.rs`, `src/http.rs`, `src/main.rs`; tests en `tests/` (`health.rs`, `auth.rs`, `account.rs`, `startup.rs`).
- Autenticación por **API key** (`APIMAIL_API_KEY`, fail-closed) y **cuenta IMAP/SMTP** configurada por entorno (fail-closed) con endpoint `GET /api/account`.
- Existe `openspec/` (schema `spec-driven`), `README.md`, `plans/PLAN.md` (roadmap completado) y `plans/PLAN-001.md` (trabajo posterior), gitflow + CI/CD (`.github/workflows/`). No hay `frontend/`.

## Qué se está construyendo
API HTTP con **Axum** que expone un CRUD sobre una cuenta de correo:
- **IMAP (`async-imap`)**: leer, buscar, mover/copiar, banderas (`\Seen`, `\Flagged`, `\Deleted`), `EXPUNGE`, descarga de cabeceras/cuerpo/partes.
- **SMTP (`lettre`)**: envío de correo saliente.
- **Parseo MIME (`mail-parser`)**: extraer texto plano, HTML y adjuntos.
- **Búsqueda avanzada**: criterios estándar de IMAP (remitente, fecha, asunto, flags).
- **Tiempo real**: extensión `IDLE`; al llegar un correo, hacer `POST` del mensaje completo + metadatos a un **endpoint configurable (webhook)**.
- Hoy implementado: arranque configurable, `GET /api/health`, autenticación (`api-auth`), configuración de la cuenta IMAP/SMTP (`mail-account`, incluye `GET /api/account`) y **envío saliente** (`smtp-send`, `POST /api/messages`). Falta la conexión de lectura IMAP, el parseo MIME y la IDLE/webhook.

## Flujo SDD + TDD (obligatorio)
Antes de escribir/editar código, `just check-spec` debe confirmar un change proposal aprobado en `openspec/changes/<feature>/`. Si no existe, detente y pide aprobación; no escribas código. Tras aprobar la spec: **RED → GREEN → REFACTOR**, verificando por CLI (`just test`, `just clippy`, `just fmt`). Nunca asumas que los tests compilan o pasan sin ejecutarlos.

Artefactos OpenSpec: `proposal.md` → `specs/<capability>/spec.md` → `design.md` → `tasks.md` (headings/`SHALL` en inglés, prosa en español). Los `## Requirement`/`#### Scenario` del delta deben copiar EXACTAMENTE el header de la spec destino, o `openspec archive` falla. Consolida con `openspec archive <feature>`; las specs resultantes viven en `openspec/specs/<capability>/`.

## Gotchas del task runner (`just`)
- El `.justfile` fue **copiado de otro proyecto ("Valet")**. Las recetas `dev`, `dev-docker`, `build`, `push`, `deploy`, `deploy-local`, `health`, `check-all` y todas las `frontend-*` referencian cosas que **no existen aquí** (`docker-compose.yml`, `frontend/`, `ghcr.io/atareao/valet-ai`, `vampus show`). **No las ejecutes** hasta que se cree esa infraestructura.
- Recetas fiables hoy: `just test`, `just clippy`, `just fmt`, `just check-spec`.

## Comandos
- Tests: `just test` == `cargo test`
- Un solo test: `cargo test <nombre>`
- Lint (CI exige cero warnings): `just clippy` == `cargo clippy --all-targets -- -D warnings`
- Formato: `just fmt` == `cargo fmt --check`

## Convenciones
- Tests unitarios en el mismo archivo bajo `#[cfg(test)]`; tests de integración/API en `tests/`.
- Prefiere lógica inyectable y testeable sin estado global (p. ej. `Config::from_lookup`) frente a mutar variables de entorno (que en edition 2024 exige `unsafe`).
- `cargo clippy --all-targets -- -D warnings` es el estándar: el REFACTOR debe dejar cero warnings (limpia variantes/código muerto que aparezcan en cascada al eliminar funcionalidad).

## Git Flow

This project follows strict gitflow. See [GIT_FLOW.md](./GIT_FLOW.md) for:
- Branch structure (main, development, feature/*, hotfix/*)
- Conventional commits with gitmoji
- How to create features, hotfixes, and releases
