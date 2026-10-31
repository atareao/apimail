# Servidor MCP — diseño y funcionamiento (referencia literal)

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
