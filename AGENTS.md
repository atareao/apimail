# AGENTS.md — apimail

> Este fichero describe el estado del proyecto. Verifica siempre el estado real (ficheros, tests, tags) antes de asumir que algo existe o funciona.

## Flujo SDD + TDD (obligatorio)

Antes de tocar código, `just check-spec` debe confirmar un change aprobado en `openspec/changes/<feature>/`; si no existe, detente y pide aprobación. Tras aprobar la spec: **RED → GREEN → REFACTOR**, verificando con `just check-all`. Nunca asumas que los tests compilan o pasan sin ejecutarlos.

⚠️ **`openspec` 1.13.2 no tiene `openspec new change`**: el change se crea **a mano** (`openspec/changes/<feature>/` con `.openspec.yaml`, `proposal.md`, `design.md`, `tasks.md` y `specs/<capability>/spec.md`; plantillas `openspec templates`). El delta debe copiar EXACTAMENTE el header de la spec destino o `openspec archive` falla.

## Servidor MCP (opt-in, feature `mcp`)

- Build por defecto **sin MCP**: `GET /mcp` → `404`; rechaza al arrancar si ve una variable `APIMAIL_MCP_*` (fail-closed, `McpFeatureDisabled`).
- Dos transportes mutuamente excluyentes: **stdio** (`APIMAIL_MCP_STDIO=true`, JSON-RPC por stdin/stdout, diagnósticos solo a `stderr`) y **Streamable HTTP** (`APIMAIL_MCP_HTTP=true`, bajo `APIMAIL_MCP_PATH`, defecto `/mcp`, con `Authorization: Bearer <APIMAIL_API_KEY>`, sin OAuth).
- **17 tools** en `src/mcp.rs` que delegan en la capa de servicio (`src/service.rs`).

> **Prueba SIEMPRE en los DOS modos** (con y sin `mcp`): un cambio que pasa en uno puede romper el otro; el CI lo comprueba por separado (job `test-mcp`).

## Comandos
- Tests: `just test` == `cargo test`
- Un solo test: `cargo test <nombre>`
- Lint (CI exige cero warnings): `just clippy` == `cargo clippy --all-targets -- -D warnings`
- Formato: `just fmt` == `cargo fmt --check` (el CI usa `cargo fmt --all -- --check`)
- Documentación: `just doc` == `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`
- Con la feature MCP: `cargo clippy --features mcp --all-targets -- -D warnings`,
  `cargo test --features mcp --no-fail-fast` y
  `RUSTDOCFLAGS="-D warnings" cargo doc --features mcp --no-deps` — o todo junto con
  `just check-all-mcp`.
- CI local: **`just check-all`** (`fmt`+`clippy`+`doc`+`test`) y `just check-all-mcp` (`--features mcp`); no lo compongas a mano.

## Convenciones
- Tests unitarios en el mismo archivo bajo `#[cfg(test)]`; tests de integración/API en `tests/`.
- Prefiere lógica inyectable y testeable sin estado global (p. ej. `Config::from_lookup`) frente a mutar variables de entorno (que en edition 2024 exige `unsafe`).
- El REFACTOR debe dejar cero warnings con `cargo clippy --all-targets -- -D warnings`.

## Gotchas del task runner (`just`)
- `just check-spec` **fallará** mientras no haya un change activo en `openspec/changes/` — es lo esperado.
- `dev`/`build`/`push`/`deploy*` usan **podman**; `dev-docker` requiere **docker**. Recetas `frontend-*` retiradas.

## Git Flow

Gitflow estricto (main/development/feature/*/hotfix/*, commits convencionales con gitmoji): ver [GIT_FLOW.md](./GIT_FLOW.md).

## Dónde está el resto

| Tema | Fichero |
| --- | --- |
| Estado actual, qué se construye y roadmap | `docs/estado-proyecto.md` |
| Servidor MCP | `docs/mcp.md` |
| Flujo SDD + TDD | `docs/sdd-tdd.md` |
| Gotchas del task runner | `docs/just-gotchas.md` |
| El CI de apimail | `docs/ci.md` |
| Despliegue en producción | `docs/despliegue.md` |
| Gitflow | `docs/gitflow.md` |
| Convenciones | `docs/convenciones.md` |
