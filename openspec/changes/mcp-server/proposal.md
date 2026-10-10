# Proposal

## Why

Apimail ya expone un CRUD completo de correo por HTTP (`imap-mailboxes`, `imap-messages`,
`imap-flags`, `mime-parsing`, `smtp-send`, `imap-idle`), pero solo es consumible escribiendo HTTP
a mano. Los clientes de agentes (opencode, Claude Desktop/Code, Cursor, …) hablan **MCP** (*Model
Context Protocol*), así que hoy no pueden leer, buscar, enviar ni mover correo de la cuenta
configurada. MCP es una **capa de exposición**, no un backend nuevo: el valor está en reutilizar la
lógica de dominio existente sin duplicarla y sin arrastrar la API HTTP a nadie.

## What Changes

- Nueva capability **`mcp-server`**: el binario puede arrancar como servidor MCP, bien por
  **stdio** (uso local) o por **Streamable HTTP** montado sobre el `axum` existente
  (`nest_service`, sin segundo listener ni puerto extra). Ambas modalidades son **mutuamente
  excluyentes** y **opt-in**: sin variables MCP, el comportamiento actual queda intacto.
- Nueva capability **`mcp-tools`**: **17 tools** (12 de lectura + 5 de escritura), una por operación
  HTTP, derivadas del **mismo DTO** que la API. Se excluyen las administrativas/destructivas
  (`GET /api/health`, `POST /api/idle/start`, `POST /api/idle/stop`).
- Nueva capability **`mcp-auth`**: el endpoint HTTP reutiliza **la API key actual**
  (`Authorization: Bearer`) tal cual, aplicada por el middleware ya existente. **Sin OAuth** en
  esta iteración (ChatGPT queda fuera; se documenta como trabajo posterior).
- **Refactor interno**: se extrae una **capa de servicio** (`src/service.rs`) que concentra la
  lógica y los errores de dominio; los handlers de `src/http.rs` pasan a ser envoltorios delgados y
  las tools reutilizan esa misma capa. **Una sola fuente de verdad**.
- Las tools se exponen tras la **feature de Cargo `mcp`, por defecto OFF**: el build por defecto no
  compila las dependencias MCP ni monta endpoint alguno.
- Nuevas variables **fail-closed**: `APIMAIL_MCP_STDIO`, `APIMAIL_MCP_HTTP` (booleanas, defecto
  `false`) y `APIMAIL_MCP_PATH` (defecto `/mcp`).

No hay **BREAKING**: sin variables MCP y sin la feature, el contrato HTTP publicado y el arranque
actuales no cambian. Los cambios en `http-api` son aditivos (un escenario nuevo).

## Capabilities

### New Capabilities
- `mcp-server`: servidor MCP opcional con dos transportes (stdio y Streamable HTTP anidado en el
  `axum` existente), configuración validada al arranque y aislamiento total cuando la feature está
  apagada.
- `mcp-tools`: superficie de 17 tools MCP (lectura y escritura) mapeada 1:1 sobre las capacidades
  HTTP existentes, con esquemas derivados y errores de dominio legibles.
- `mcp-auth`: reutilización del `Authorization: Bearer` actual en el transporte HTTP y herencia del
  entorno del proceso en stdio, sin debilitar la frontera de confianza ni filtrar credenciales.

### Modified Capabilities
- `http-api`: el requisito *Server startup and configuration* se amplía para que, cuando el
  transporte MCP stdio esté activo, el servicio sirva MCP por stdin/stdout y **no** enlace ningún
  puerto TCP.

## Impact

- `Cargo.toml`: feature `mcp` (default OFF) y dependencias **opcionales** `rmcp` y `schemars`
  (higiene de dependencias en `design.md`). **No** se toca hasta la fase de implementación.
- `src/service.rs` (**nuevo**): capa de servicio compartida (DTOs y errores de dominio).
- `src/mcp.rs` (**nuevo**, tras `#[cfg(feature = "mcp")]`): servidor, transportes y las 17 tools.
- `src/http.rs`: `build_router` anida el servicio MCP cuando corresponde; los handlers delegan en
  la capa de servicio.
- `src/config.rs`: tres variables nuevas, parseo booleano y variantes de `ConfigError`.
- `src/main.rs`: bifurcación stdio vs HTTP.
- `src/lib.rs`: nuevo módulo y re-exports.
- `.github/workflows/ci.yml`: job que compile con `--features mcp` sin alterar el CI por defecto.
- `README.md` y `docs/API.md`: habilitación de la feature, variables MCP y configuración por cliente.

## Fuera de alcance

OAuth 2.1 / *Dynamic Client Registration* (ChatGPT), `resources`/`prompts`/`sampling`, transporte
HTTP+SSE legacy, y las operaciones administrativas/destructivas (`GET /api/health`,
`POST /api/idle/start`, `POST /api/idle/stop`) como tools.
