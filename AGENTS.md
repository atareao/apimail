# AGENTS.md — apimail

> Este fichero describe el estado del proyecto. Verifica siempre el estado real (ficheros, tests, tags) antes de asumir que algo existe o funciona.

## Estado actual (verificado)
- `Cargo.toml` (Rust **edition 2024**) tiene las dependencias base: `axum`, `tokio`, `serde`/`serde_json`, `tracing`/`tracing-subscriber`, `thiserror`; dev: `tower`, `http-body-util`. Las de MCP (`rmcp` 3.5.1 y `schemars`) son **opcionales**, detrás de la feature `mcp` (ver la sección del servidor MCP).
- API implementada en estructura **lib + bin**: **11 ficheros** en `src/` (`lib.rs`, `config.rs`, `http.rs`, `service.rs`, `imap.rs`, `smtp.rs`, `mime.rs`, `idle.rs`, `queue.rs`, `mcp.rs` y `main.rs`).
  - `Config` y `MailAccount` viven en `src/config.rs`; `build_router` en `src/http.rs`.
  - **`src/service.rs`** (2240 líneas) es la **capa de servicio**: concentra la lógica de dominio, los DTOs y `ServiceError`; la consumen por igual los handlers de `http.rs` y las tools MCP. Una sola fuente de verdad.
  - **`src/mcp.rs`** (883 líneas) es el **servidor MCP**, tras `#[cfg(feature = "mcp")]`.
- **15** ficheros de test en `tests/` (cubren IMAP, SMTP, MIME, IDLE, la cola, el arranque y, con la feature `mcp`, el transporte stdio — `tests/mcp_server.rs` — y el Streamable HTTP — `tests/mcp_http.rs`).
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
- Estado: las **9 capabilities del roadmap original** (`plans/PLAN.md`) están implementadas, revisadas, archivadas y publicadas. Sobre ese roadmap se han entregado además `webhook-delivery`, `container-image`, `imap-session-liveness` y el **servidor MCP** (implementado y archivado, **aún sin publicar**). El **tag más reciente** es `v0.4.4` — no lo copies de aquí: mide con `git tag --sort=-v:refname | head -1`.
- **Queda roadmap pendiente** (nada de esto está implementado): los ítems **#4** (`idle-scope`), **#5** (`attachment-content`), **#7** (`manifest-digest`) y **#9** (`openapi`) de `plans/PLAN-001.md` §2. Los #4 y #5 son *Fuera de alcance* conscientes; #7 y #9 siguen sin propuesta.

## Servidor MCP (opt-in, feature `mcp`)

El proyecto **también** es un servidor **MCP** (*Model Context Protocol*): expone la misma cuenta de
correo como *tools* para clientes de agentes. Es la pieza más grande después de la API HTTP.

- **Feature de Cargo `mcp`, por defecto OFF** (`[features] mcp = ["dep:rmcp", "dep:schemars"]`).
  **El build por defecto no incluye nada de MCP**: no compila `rmcp`/`schemars`, no declara el
  módulo `mcp`, no monta ruta ni transporte alguno (`GET /mcp` responde `404`) y **rechaza al
  arrancar** si ve una variable `APIMAIL_MCP_*` (fail-closed, error `McpFeatureDisabled`).
  Sirve para todo el proyecto anterior intacto.
- **Capa de servicio** (`src/service.rs`): el refactor que hizo posible el MCP sin duplicar lógica.
  Los handlers HTTP y las tools MCP comparten DTOs y `ServiceError`.
- **Dos transportes, mutuamente excluyentes**:
  - **stdio** (`APIMAIL_MCP_STDIO=true`): JSON-RPC por `stdin`/`stdout`; **no enlaza ningún puerto
    TCP**. Los diagnósticos van a `stderr` (nada puede escribir en `stdout` o corrompería el
    protocolo).
  - **Streamable HTTP** (`APIMAIL_MCP_HTTP=true`), **anidado en el mismo `axum`** (`nest_service`,
    sin segundo listener) bajo `APIMAIL_MCP_PATH` (defecto `/mcp`) y **reutilizando la auth actual**:
    la misma `Authorization: Bearer <APIMAIL_API_KEY>` del middleware existente, **sin OAuth**.
- **17 tools** (`grep -c '#\[tool(' src/mcp.rs`): 12 de lectura + 5 de escritura. Cada una delega en
  la capa de servicio; no hay lógica de negocio nueva. No se exponen las administrativas/
  destructivas (`GET /api/health`, `POST /api/idle/start|stop`).
- **CI**: `.github/workflows/ci.yml` tiene un job propio, **`test-mcp`**, que ejecuta `fmt`, `clippy`,
  `cargo doc` y `test` **con `--features mcp`**; el job `test` sigue corriendo **sin** la feature.

> **Al tocar cualquier cosa hay que probar en los DOS modos.** Un cambio que pasa con la feature
> puede romper el build por defecto (variantes muertas, imports huérfanos, gates mal puestos) y
> viceversa. El build por defecto **no** es MCP, y el CI lo comprueba por separado.

## Flujo SDD + TDD (obligatorio)

Antes de escribir/editar código, `just check-spec` debe confirmar un change proposal aprobado en `openspec/changes/<feature>/`. Si no existe, detente y pide aprobación; no escribas código. Tras aprobar la spec: **RED → GREEN → REFACTOR**, verificando por CLI (`just test`, `just clippy`, `just fmt`). Nunca asumas que los tests compilan o pasan sin ejecutarlos.

⚠️ **`openspec` 1.13.2 no tiene `openspec new change`** (ese subcomando no existe: `openspec change` solo ofrece `show`, `list` y `validate`). El change se crea **a mano**: el directorio `openspec/changes/<feature>/` con su `.openspec.yaml` (`schema: spec-driven`, `created`, `goal`) más `proposal.md`, `design.md`, `tasks.md` y `specs/<capability>/spec.md`. Plantillas: `openspec templates`.

Artefactos OpenSpec: `proposal.md` → `specs/<capability>/spec.md` → `design.md` → `tasks.md` (headings/`SHALL` en inglés, prosa en español). Los `## Requirement`/`#### Scenario` del delta deben copiar EXACTAMENTE el header de la spec destino, o `openspec archive` falla. Consolida con `openspec archive <feature>`; las specs resultantes viven en `openspec/specs/<capability>/`.

## Gotchas del task runner (`just`)
- El `.justfile` está **adaptado a apimail**: las recetas de contenedor y salud usan `ghcr.io/atareao/apimail` y el `compose.yml` real (existen `Dockerfile`, `.dockerignore`, `.env.example` y `.env.j2`), y la versión de la imagen se resuelve con `vampus show` (`vampus` instalado + `.vampus.yml`). Las recetas heredadas `frontend-*` se **retiraron** (el proyecto no tiene `frontend/`). `dev`/`build`/`push`/`deploy*` usan **podman** (instalado aquí); `dev-docker` requiere **docker** (no instalado en esta máquina).
- Recetas fiables hoy: `just test`, `just clippy`, `just fmt`, `just check-spec` y `just check-all` (encadena **tres**: `fmt`, `clippy`, `test`). ⚠️ **`just check-all` no es el CI**: le falta `cargo doc` (ver «Comandos»).
- `just check-spec` **fallará** mientras no haya un change activo en `openspec/changes/` — es lo esperado, no un fallo del repo. El mensaje de error explica cómo crearlo a mano.

## Comandos
- Tests: `just test` == `cargo test`
- Un solo test: `cargo test <nombre>`
- Lint (CI exige cero warnings): `just clippy` == `cargo clippy --all-targets -- -D warnings`
- Formato: `just fmt` == `cargo fmt --check` (el CI usa `cargo fmt --all -- --check`)
- Documentación: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`
- Con la feature MCP: `cargo clippy --features mcp --all-targets -- -D warnings`,
  `cargo test --features mcp --no-fail-fast` y
  `RUSTDOCFLAGS="-D warnings" cargo doc --features mcp --no-deps`.

### El equivalente al CI son CUATRO cosas, no tres

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`
4. `cargo test`

⚠️ **`just check-all` no ejecuta `cargo doc`.** Encadena solo `fmt`, `clippy` y `test` — tres de las
cuatro. Por eso es fácil creerse verde con `just check-all` y que el CI falle por un enlace roto del
rustdoc. Ejecuta (3) a mano.

Con la feature `mcp` hay **además** un segundo juego completo en el job `test-mcp`:
`clippy`/`doc`/`test` con `--features mcp`. Prueba **ambos modos** antes de dar algo por terminado.

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
