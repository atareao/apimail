# Estado actual y qué se está construyendo (referencia literal)

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
