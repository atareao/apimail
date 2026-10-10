# AGENTS.md — apimail

> Este fichero describe el estado del proyecto. Verifica siempre el estado real (ficheros, tests, tags) antes de asumir que algo existe o funciona.

## Estado actual (verificado)
- `Cargo.toml` (Rust **edition 2024**) ya tiene las dependencias base: `axum`, `tokio`, `serde`/`serde_json`, `tracing`/`tracing-subscriber`, `thiserror`; dev: `tower`, `http-body-util`.
- API implementada en estructura **lib + bin**: **nueve ficheros** en `src/` (`lib.rs`, `config.rs`, `http.rs`, `imap.rs`, `smtp.rs`, `mime.rs`, `idle.rs`, `queue.rs` y `main.rs`); `Config` y `MailAccount` viven en `src/config.rs`, y `build_router` en `src/http.rs`. **12** ficheros de test en `tests/` (cubren IMAP, SMTP, MIME, IDLE, la cola y el arranque).
- Roadmap del MVP **completo** (`plans/PLAN.md`) y publicado en crates.io y GitHub Releases; para la última versión, `git tag --sort=-v:refname`.
- Autenticación por **API key** (`APIMAIL_API_KEY`, fail-closed) y **cuenta IMAP/SMTP** configurada por entorno (fail-closed) con endpoint `GET /api/account`.
- Existe `openspec/` (schema `spec-driven`), `README.md`, `plans/PLAN.md` (roadmap completado) y `plans/PLAN-001.md` (trabajo posterior), gitflow + CI/CD (`.github/workflows/`). No hay `frontend/`.

## Qué se está construyendo
API HTTP con **Axum** que expone un CRUD sobre una cuenta de correo:
- **IMAP (`async-imap`)**: leer, buscar, mover/copiar, banderas (`\Seen`, `\Flagged`, `\Deleted`), `EXPUNGE`, descarga de cabeceras/cuerpo/partes.
- **SMTP (`lettre`)**: envío de correo saliente.
- **Parseo MIME (`mail-parser`)**: extraer texto plano, HTML y adjuntos.
- **Búsqueda avanzada**: criterios estándar de IMAP (remitente, fecha, asunto, flags).
- **Tiempo real**: extensión `IDLE`; al llegar un correo, hacer `POST` del mensaje completo + metadatos a un **endpoint configurable (webhook)**.
- Estado: las **9 capabilities del roadmap están implementadas, revisadas, archivadas y publicadas** en **`v0.3.0`** — `api-auth`, `mail-account`, `imap-connection`, `imap-mailboxes`, `imap-messages`, `imap-flags`, `mime-parsing`, `smtp-send` e `imap-idle`, más `webhook-delivery` (entrega *at-least-once* con cola durable y persistencia opt-in). No queda nada del roadmap por implementar.

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

## Despliegue en producción (verificado el 2026-10-10)

Servicio en vivo: **https://apimail.territoriolinux.es** (host `co1`, detrás de Traefik con TLS).
Verificación contra producción: **20 endpoints** ejercitados (lectura y escritura, incluido
`POST /api/messages` con entrega real a un buzón externo); el artefacto desplegado es byte a byte
el publicado por CI (`id=sha256:8b3a550ce6ab…`, `digest=sha256:12e895cdd260…`).

- **Red**: el contenedor `apimail` debe estar en la red externa `proxy`, compartida con Traefik
  (`networks: [- proxy]` en el servicio + `networks: { proxy: { external: true } }` a nivel raíz).
  Sin ella, Traefik no resuelve el nombre `apimail` y la URL pública devuelve **504**. La red
  `apimail_default` queda huérfana y puede eliminarse.
- **Stack**: gestionado por **Dockge**; el compose vive en
  `co1:/home/lorenzo/docker/dockge/stacks/apimail/compose.yaml` (montado como `/opt/stacks`).
  Los secretos y la `APIMAIL_API_KEY` viven en el `.env` de ese directorio.
- **IDLE/webhook desactivado a propósito**: `APIMAIL_WEBHOOK_URL` está **vacío** en producción, así
  que `GET /api/idle/status` responde `stopped` y `POST /api/idle/start` devuelve **501
  `idle_not_configured`**. No es un fallo.
- **Aviso de infraestructura (no es apimail)**: el entrypoint `https` de Traefik aplica
  `shuul-auth@file` (un `forwardAuth` a `http://shuul:3000/api/v1/shuul`) a **todo** el tráfico.
  Devuelve **403 `Ko`** a las peticiones cuya IP origen es la pública de `co1`; por eso probar
  desde `co1` contra la URL pública da 403 y hay que usar la red interna del contenedor.
