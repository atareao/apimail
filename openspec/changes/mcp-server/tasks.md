# Tasks

> Checklist TDD (RED → GREEN → REFACTOR) para `mcp-server`. Cada sección verifica por CLI con
> `cargo test`, `cargo clippy --all-targets -- -D warnings` y `cargo fmt --check`. **Todas las
> casillas están sin marcar**: no se ha escrito código todavía (fase SDD, pendiente de aprobación).

## 1. Andamiaje de la feature (Cargo.toml + CI)

- [ ] 1.1 RED (línea base, no es un test unitario): fijar el estado de partida del build por
  defecto — `cargo build` y `cargo test` en verde, y `cargo tree` **sin** la feature **no** debe
  listar `rmcp` ni `schemars`. Registrar la salida como referencia.
- [ ] 1.2 GREEN: en `Cargo.toml`, declarar `[features] mcp = ["dep:rmcp", "dep:schemars"]`
  (**default OFF**) y las deps **opcionales** con la higiene del `design.md`: `rmcp` con
  `default-features = false` y features `transport-io`, `transport-streamable-http-server`,
  `macros`, `schemars`; `schemars = { version = "1.2.2", optional = true }`. **NO** activar la
  feature `auth` de `rmcp`. Verificación (de build, no test unitario): `cargo build` (por defecto) y
  `cargo build --features mcp` compilan; `cargo tree --features mcp` no muestra un segundo
  `reqwest` 0.13 ni `base64` 0.23.
- [ ] 1.3 GREEN (CI): añadir un **job nuevo** `test-mcp` a `.github/workflows/ci.yml` que ejecute
  `cargo test --features mcp` (y `cargo clippy --all-targets --features mcp -- -D warnings`) **sin**
  tocar el job `test` por defecto, que sigue sin la feature. Verificación: el CI por defecto no
  cambia y el job nuevo compila con la feature.
- [ ] 1.4 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` limpios en
  ambos modos. **Verificación de la promesa de opcionalidad** (build, no test unitario):
  `cargo tree` / `cargo tree --no-default-features` **sin** la feature **no** listan `rmcp` ni
  `schemars`; `cargo tree --features mcp` sí los lista.

## 2. Capa de servicio `src/service.rs` (el paso más delicado)

- [ ] 2.1 RED: tests de **caracterización** del comportamiento HTTP actual — por cada handler, un
  test que fije la entrada, la salida (DTO) y el código/`error` de dominio observable, incluyendo
  los casos de error reales (`invalid_request`, `mailbox_not_found`, `message_not_found`,
  `attachment_not_found`, `message_too_large`, `message_not_parsable`, `capability_not_supported`,
  `smtp_error`, `imap_unavailable` y `idle_not_configured`). **Nota**: `idle_not_configured` es
  caracterización de la capa de servicio **para HTTP** (`POST /api/idle/start`); **no** es un error
  alcanzable desde las tools MCP, porque esas rutas están excluidas de la superficie. Ejecutar
  `cargo test` y confirmar que **los 313 tests existentes y los nuevos pasan** sobre el código aún
  sin refactorizar.
- [ ] 2.2 GREEN (DTOs): **reubicar o exponer** los DTOs hoy privados de `src/http.rs` — moverlos a
  `src/service.rs` (o `pub(crate)` desde `http.rs`) para que `src/mcp.rs` los use: `MailboxDto`
  (`src/http.rs:686`), `MailboxListResponse` (`707`), `MailboxStatusResponse` (`721`), `MessageDto`
  (`845`), `MessageListResponse` (`875`), `MessageDetailResponse` (`893`), `MessageBodyResponse`
  (`961`), `ParsedAttachmentDto` (`977`), `AttachmentListResponse` (`1007`), `AttachmentResponse`
  (`1019`), `FlagUpdateDto` (`1256`), `MoveCopyDto` (`1290`), `FlagUpdateResponse` (`1354`),
  `MoveCopyResponse` (`1365`), `DeleteResponse` (`1378`), `SendMessageDto` (`1510`),
  `ErrorResponse` (`1586`), `WhoamiResponse` (`648`), `SentResponse` (`655`),
  `ImapStatusResponse` (`662`), `ImapUnavailableResponse` (`675`), `SelectMailboxDto` (`714`).
  Hoy solo `EndpointView`, `AccountView` y `HealthResponse` son `pub`. Verificación: `cargo test`
  sigue en verde (sin cambios de contrato).
- [ ] 2.3 GREEN: crear las operaciones de dominio de `src/service.rs` y `ServiceError` (mismos DTOs
  y mismos códigos). El *derive* de esquema va **condicionado**: los tipos de entrada/salida de las
  tools llevan `#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]`, de modo que `schemars`
  sigue siendo **opcional** (Decisión 8). Reconvertir los handlers de `src/http.rs` en envoltorios
  delgados que mapean `ServiceError` → `ErrorResponse`/estado HTTP. Verificación: `cargo test` con
  **los 313 tests existentes en verde** y **ningún contrato HTTP cambiado** (ninguna aserción de
  status/JSON modificada).
- [ ] 2.4 REFACTOR: eliminar lógica duplicada entre `src/http.rs` y `src/service.rs`,
  `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` limpios. Criterio de cierre del
  paso: `cargo test` sigue en verde y el diff de contratos HTTP es **cero**.

## 3. Configuración MCP en `src/config.rs`

- [ ] 3.1 RED: tests de `Config` para `APIMAIL_MCP_STDIO` y `APIMAIL_MCP_HTTP` (defecto `false`;
  `true`/`false` case-insensitive y `1`/`0`; cualquier otro valor → error que **nombra la
  variable**), para `APIMAIL_MCP_PATH` (defecto `/mcp`; relativo, `/` o dentro de `/api` → error) y
  para la exclusión mutua (`stdio` y `http` a `true` → error que nombra **ambas** variables).
  Ejecutar `cargo test` y confirmar el fallo.
- [ ] 3.2 GREEN: campos `mcp_stdio`, `mcp_http`, `mcp_path` en `Config` con el parseo booleano
  (**primer booleano de `Config`**; el proyecto ya parsea `flagged` como booleano de *query param*
  con `parse_strict_bool`, `src/http.rs:1087`) y las variantes de `ConfigError` con
  `public_message()`/`kind()` que **nunca** exponen el valor. Verificación: `cargo test`.
- [ ] 3.3 GREEN: cuando el crate se compila **sin** la feature `mcp` y se define cualquier variable
  MCP, fallar al arrancar nombrando la variable (fail-closed). Verificación: test que lo comprueba y
  `cargo test` sin la feature.
- [ ] 3.4 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` limpios.

## 4. Servidor MCP + transporte stdio

- [ ] 4.1 RED (en proceso): probar el *handler*/router MCP con una **tubería en memoria**
  (`tokio::io::duplex`) que haga de transporte, con una **tool trivial** de validación, y comprobar
  que responde al ciclo MCP (initialize + tools/list + tools/call) y que **no** abre ningún
  `TcpListener`. Ejecutar `cargo test --features mcp` y confirmar el fallo.
- [ ] 4.2 GREEN: `src/mcp.rs` con el servidor `rmcp` (`#[tool_router]`/`#[tool_handler]`) y el
  transporte `transport-io`; bifurcar `src/main.rs` para servir stdio **sin** crear `TcpListener`
  (`src/main.rs:33`). Verificación: `cargo test --features mcp` (el test en proceso de 4.1).
- [ ] 4.3 RED+GREEN (integración por subproceso): la pureza de `stdout` («stdout carries only
  protocol data») **no** se puede probar en proceso —stdin/stdout son los del propio *harness* de
  test—, así que se prueba con un **test de integración por subproceso** que ejecuta el **binario
  compilado** en modo stdio y habla JSON-RPC por tuberías, afirmando que todo lo que llega por
  `stdout` es JSON-RPC válido y que los diagnósticos van a `stderr` (los logs **ya** se emiten por
  `stderr`, `src/main.rs:15`). Verificación: `cargo test --features mcp` con el test de subproceso.
- [ ] 4.4 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios.

## 5. Transporte Streamable HTTP anidado + auth reutilizada

- [ ] 5.1 RED: tests de `build_router` con el transporte HTTP activo (el `mcp_path` del `AppState`
  es `Some`): el path MCP (defecto `/mcp` y configurable) responde al transporte; **sin** API key →
  `401` con el envoltorio existente y `WWW-Authenticate: Bearer`; con la API key válida → llega al
  transporte; una ruta fuera del path MCP sigue igual. Confirmar que **los tests existentes que
  llaman `build_router(state)` no cambian** (con `mcp_path: None` no se monta nada). Ejecutar
  `cargo test --features mcp` y confirmar el fallo.
- [ ] 5.2 GREEN: añadir a `AppState` (`src/http.rs:403`) el campo `mcp_path: Option<String>`
  poblado desde `Config` en `from_config`/`with_*` (bajo `#[cfg(feature = "mcp")]`, para no dejar
  código muerto en el build por defecto), **sin tocar la firma de `build_router`**
  (`src/http.rs:2158`); anidar el `StreamableHttpService` de `rmcp` con `nest_service` dentro de
  `protected` (reutiliza `require_api_key`, `src/http.rs:2188`) cuando el campo es `Some`, sobre el
  **mismo** listener. Verificación: `cargo test --features mcp` y `cargo test` (por defecto, sin
  cambios).
- [ ] 5.3 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios; comprobar que **no** se abre un puerto adicional.

## 6. Tools de lectura (12)

- [ ] 6.1 RED: tests por tool — `whoami`, `get_account`, `imap_status`, `list_mailboxes`,
  `select_mailbox`, `list_messages`, `get_message`, `get_message_body`, `list_attachments`,
  `get_attachment`, `idle_status`, `idle_queue` — verificando que el resultado **coincide** con el de
  su ruta HTTP equivalente (misma fuente de verdad) y que `list_messages` respeta los topes
  (defecto 50, máximo 200). Ejecutar `cargo test --features mcp` y confirmar el fallo.
- [ ] 6.2 GREEN: implementar las 12 tools delegando en `src/service.rs`, con `inputSchema`
  derivado por `schemars` mediante el derive **condicional**
  `#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]` sobre los tipos de `src/service.rs`
  (opcionales opcionales), de forma que `schemars` siga siendo opcional. Verificación:
  `cargo test --features mcp` y `cargo tree` (sin la feature) sin `schemars`.
- [ ] 6.3 GREEN: garantizar que la metadata de adjuntos **no** lleva bytes (`get_attachment` es la
  única que devuelve base64, con los mismos topes que la API). Verificación: test que lo comprueba.
- [ ] 6.4 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios; sin lógica de dominio duplicada respecto a `src/http.rs`.

## 7. Tools de escritura (5)

- [ ] 7.1 RED: tests por tool — `send_message`, `update_flags`, `move_message`, `copy_message`,
  `delete_message` — verificando equivalencia con la ruta HTTP y la semántica destructiva:
  `delete_message` marca `\Deleted` + `EXPUNGE` (no mueve a Trash) y exige `UIDPLUS`;
  `move_message` exige buzón destino explícito; `update_flags` cambia **solo** las banderas
  nombradas. Ejecutar `cargo test --features mcp` y confirmar el fallo.
- [ ] 7.2 GREEN: implementar las 5 tools delegando en `src/service.rs`, con documentación de la
  semántica destructiva en la descripción de la tool. Verificación: `cargo test --features mcp`.
- [ ] 7.3 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios.

## 8. Mapeo de errores y no-fuga de secretos

- [ ] 8.1 RED: tests que fuerzan cada `ServiceError` y comprueban que el error de tool MCP lleva
  **el mismo código `error` estable** que la API HTTP y un mensaje legible; y que ningún resultado,
  error ni log contiene la API key ni el password de la cuenta. Ejecutar `cargo test --features mcp`
  y confirmar el fallo.
- [ ] 8.2 GREEN: mapear `ServiceError` → error de tool MCP reutilizando `public_message()`/`kind()`
  (sin narrar el valor). Verificación: `cargo test --features mcp`.
- [ ] 8.3 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios.

## 9. Documentación

- [ ] 9.1 Documentar en `README.md` cómo habilitar la feature `mcp`, las variables
  `APIMAIL_MCP_STDIO`/`APIMAIL_MCP_HTTP`/`APIMAIL_MCP_PATH` y **la configuración en cada cliente
  verificado** (opencode, Claude Code/Desktop, Cursor). Verificación: siguen los pasos tal cual y el
  servidor arranca en la modalidad descrita.
- [ ] 9.2 Añadir en `docs/API.md` una nota de que MCP es una **capa de exposición** sobre la API
  existente (no un backend nuevo). Verificación: la nota es coherente con el mapeo de tools.
- [ ] 9.3 Añadir las tres variables nuevas (`APIMAIL_MCP_STDIO`, `APIMAIL_MCP_HTTP`,
  `APIMAIL_MCP_PATH`) a **`.env.example`** y a **`.env.j2`** (la plantilla de entorno que se
  despliega). Verificación: `grep` de las tres variables en ambos ficheros.

## 10. Integración real y cierre

- [ ] 10.1 Integración real con un cliente MCP (opencode en local) y verificación end-to-end
  contra la cuenta configurada: listar buzones, leer un mensaje, buscarlo y, con cuidado, ejercitar
  una escritura no destructiva (`update_flags`). Verificación: traza del cliente con resultados
  reales.
- [ ] 10.2 `just check-all` (== `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`) en verde en ambos modos (por defecto y `--features mcp`).
- [ ] 10.3 `openspec validate mcp-server --strict` en verde y `openspec archive mcp-server`.
- [ ] 10.4 PR a `development`. Release a `main` **solo** con confirmación explícita del usuario.
