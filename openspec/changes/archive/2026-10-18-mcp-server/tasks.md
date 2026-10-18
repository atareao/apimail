# Tasks

> Checklist TDD (RED → GREEN → REFACTOR) para `mcp-server`. Cada sección verifica por CLI con
> `cargo test`, `cargo clippy --all-targets -- -D warnings` y `cargo fmt --check`.
> **Estado: §1–§9 implementadas y verificadas** (andamiaje, capa de servicio, configuración, los dos
> transportes, las 17 tools, mapeo de errores y documentación), con el CI por defecto y el job
> `test-mcp` en verde. Solo queda **§10** (cierre): la verificación con un cliente MCP real, el PR y
> el release.
>
> *Anclas `fichero:línea`:* revalidadas en la **pasada de cierre** (§10), con `src/http.rs` ya
> congelado (HEAD `27531f0`, **1483** líneas). Donde la línea es frágil se cita el **nombre del
> símbolo**, que sobrevive a un reflow. Los números de esta checklist son los del árbol al cerrar el
> change.

## 1. Andamiaje de la feature (Cargo.toml + CI)

- [x] 1.1 RED (línea base, no es un test unitario): fijar el estado de partida del build por
  defecto — `cargo build` y `cargo test` en verde, y `cargo tree` **sin** la feature **no** debe
  listar `rmcp` ni `schemars`. Registrar la salida como referencia. **MEDIDO**: `cargo tree` sin la
  feature → **0** apariciones de `rmcp`/`schemars`; `cargo tree --features mcp` → **4** paquetes
  (`rmcp`, `rmcp-macros`, `schemars`, `schemars_derive`).
- [x] 1.2 GREEN: en `Cargo.toml`, declarar `[features] mcp = ["dep:rmcp", "dep:schemars"]`
  (**default OFF**) y las deps **opcionales** con la higiene del `design.md`: `rmcp` con
  `default-features = false` y features `server`, `transport-io`,
  `transport-streamable-http-server`, `macros`, `schemars`; `schemars = { version = "1.2.2",
  optional = true }`. **NO** activar la feature `auth` de `rmcp`. Verificación (de build, no test
  unitario): `cargo build` (por defecto) y `cargo build --features mcp` compilan; `cargo tree
  --features mcp` **no** muestra un segundo `reqwest` (sigue un único `reqwest 0.12.28`) ni un
  segundo `http`. *(Nota corregida: la comprobación original pedía «ni `base64` 0.23» — es un
  criterio equivocado: `base64 0.23.1` es **preexistente** y el par `0.22.1 + 0.23.1` es idéntico con
  y sin la feature.)* **`server` es obligatoria**: sin ella el servidor MCP no se puede ni nombrar
  (ver design.md, Decisión 3).
- [x] 1.3 GREEN (CI): añadir un **job nuevo** `test-mcp` a `.github/workflows/ci.yml` que ejecute
  `cargo test --features mcp` (y `cargo clippy --all-targets --features mcp -- -D warnings`) **sin**
  tocar el job `test` por defecto, que sigue sin la feature. Verificación: el CI por defecto no
  cambia y el job nuevo compila con la feature. **MEDIDO**: el job se llama `test-mcp` y el job
  `test` no se modificó.
- [x] 1.4 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` limpios en
  ambos modos. **Verificación de la promesa de opcionalidad** (build, no test unitario):
  `cargo tree` / `cargo tree --no-default-features` **sin** la feature **no** listan `rmcp` ni
  `schemars`; `cargo tree --features mcp` sí los lista. **MEDIDO**: 0 vs 4 paquetes (ver 1.1).

## 2. Capa de servicio `src/service.rs` (el paso más delicado)

- [x] 2.1 RED: tests de **caracterización** del comportamiento HTTP actual — por cada handler, un
  test que fije la entrada, la salida (DTO) y el código/`error` de dominio observable, incluyendo
  los casos de error reales (`invalid_request`, `mailbox_not_found`, `message_not_found`,
  `attachment_not_found`, `message_too_large`, `message_not_parsable`, `capability_not_supported`,
  `smtp_error`, `imap_unavailable` y `idle_not_configured`). **Nota**: `idle_not_configured` es
  caracterización de la capa de servicio **para HTTP** (`POST /api/idle/start`); **no** es un error
  alcanzable desde las tools MCP, porque esas rutas están excluidas de la superficie. Ejecutar
  `cargo test` y confirmar que **los 318 tests del baseline medido al iniciar la implementación y
  los nuevos pasan** sobre el código aún sin refactorizar.
- [x] 2.2 GREEN (DTOs): **reubicar y exponer** los DTOs que eran privados de `src/http.rs`. El
  refactor los movió a `src/service.rs`, donde viven hoy todos `pub` (ya no son `pub(crate)`) para
  que `src/mcp.rs` los use: `MailboxDto` (`src/service.rs:551`), `MailboxListResponse` (`576`),
  `MailboxStatusResponse` (`587`), `MessageDto` (`681`), `MessageListResponse` (`715`),
  `MessageDetailResponse` (`736`), `MessageBodyResponse` (`807`), `ParsedAttachmentDto` (`826`),
  `AttachmentListResponse` (`860`), `AttachmentResponse` (`876`), `FlagUpdateResponse` (`901`),
  `MoveCopyResponse` (`917`), `DeleteResponse` (`935`), `SendMessageDto` (`977`),
  `WhoamiResponse` (`526`), `SentResponse` (`947`) e `ImapStatusResponse` (`534`), más los **nuevos**
  que el refactor añadió: `AddressDto` (`625`), `EnvelopeDto` (`647`) y `AttachmentDto` (`959`).
  `EndpointView` (`504`) y `AccountView` (`516`) ya eran `pub`. Lo que **sigue** en `src/http.rs` es
  transporte puro —tipos de *query*/*body* y respuestas atadas a un status—: `HealthResponse`
  (`src/http.rs:342`), `ImapUnavailableResponse` (`358`), `SelectMailboxDto` (`369`),
  `ListMessagesQuery` (`422`), `FetchMessageQuery` (`451`), `MailboxQuery` (`626`),
  `FlagUpdateDto` (`636`), `MoveCopyDto` (`670`) y `ErrorResponse` (`788`). Verificación:
  `cargo test` sigue en verde (sin cambios de contrato).
- [x] 2.3 GREEN: crear las operaciones de dominio de `src/service.rs` y `ServiceError` (mismos DTOs
  y mismos códigos). El *derive* de esquema va **condicionado**: los tipos de entrada/salida de las
  tools llevan `#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]`, de modo que `schemars`
  sigue siendo **opcional** (Decisión 8). Reconvertir los handlers de `src/http.rs` en envoltorios
  delgados que mapean `ServiceError` → `ErrorResponse`/estado HTTP. Verificación: `cargo test` con
  **los 318 tests del baseline medido al iniciar la implementación en verde** y **ningún contrato
  HTTP cambiado** (ninguna aserción de status/JSON modificada).
- [x] 2.4 REFACTOR: eliminar lógica duplicada entre `src/http.rs` y `src/service.rs`,
  `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` limpios. Criterio de cierre del
  paso: `cargo test` sigue en verde y el diff de contratos HTTP es **cero**.

## 3. Configuración MCP en `src/config.rs`

- [x] 3.1 RED: tests de `Config` para `APIMAIL_MCP_STDIO` y `APIMAIL_MCP_HTTP` (defecto `false`;
  `true`/`false` case-insensitive y `1`/`0`; cualquier otro valor → error que **nombra la
  variable**), para `APIMAIL_MCP_PATH` (defecto `/mcp`; relativo, `/` o dentro de `/api` → error) y
  para la exclusión mutua (`stdio` y `http` a `true` → error que nombra **ambas** variables).
  Ejecutar `cargo test` y confirmar el fallo.
- [x] 3.2 GREEN: campos `mcp_stdio`, `mcp_http`, `mcp_path` en `Config` con el parseo booleano
  (**primer booleano de `Config`**; el proyecto ya parsea `flagged` como booleano de *query param*
  con `parse_strict_bool`, `src/http.rs:467`) y las variantes de `ConfigError` con
  `public_message()`/`kind()` que **nunca** exponen el valor. Verificación: `cargo test`.
- [x] 3.3 GREEN: cuando el crate se compila **sin** la feature `mcp` y se define cualquier variable
  MCP, fallar al arrancar nombrando la variable (fail-closed). Verificación: test que lo comprueba y
  `cargo test` sin la feature.
- [x] 3.4 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` limpios.

## 4. Servidor MCP + transporte stdio

- [x] 4.1 RED (en proceso): probar el *handler*/router MCP con una **tubería en memoria**
  (`tokio::io::duplex`) que haga de transporte, con una **tool trivial** de validación, y comprobar
  que responde al ciclo MCP (initialize + tools/list + tools/call) y que **no** abre ningún
  `TcpListener`. Ejecutar `cargo test --features mcp` y confirmar el fallo.
- [x] 4.2 GREEN: `src/mcp.rs` con el servidor `rmcp` (`#[tool_router]`/`#[tool_handler]`) y el
  transporte `transport-io`; bifurcar `src/main.rs` para servir stdio **sin** crear `TcpListener`
  (`src/main.rs:36-46`, la rama va por delante del `bind` de `src/main.rs:48`). Verificación:
  `cargo test --features mcp` (el test en proceso de 4.1).
- [x] 4.3 RED+GREEN (integración por subproceso): la pureza de `stdout` («stdout carries only
  protocol data») **no** se puede probar en proceso —stdin/stdout son los del propio *harness* de
  test—, así que se prueba con un **test de integración por subproceso** que ejecuta el **binario
  compilado** en modo stdio y habla JSON-RPC por tuberías, afirmando que todo lo que llega por
  `stdout` es JSON-RPC válido y que los diagnósticos van a `stderr` (los logs **ya** se emiten por
  `stderr`, `src/main.rs:15`). Verificación: `cargo test --features mcp` con el test de subproceso.
- [x] 4.4 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios.

## 5. Transporte Streamable HTTP anidado + auth reutilizada

- [x] 5.1 RED: tests de `build_router` con el transporte HTTP activo (el `mcp_path` del `AppState`
  es `Some`): el path MCP (defecto `/mcp` y configurable) responde al transporte; **sin** API key →
  `401` con el envoltorio existente y `WWW-Authenticate: Bearer`; con la API key válida → llega al
  transporte; una ruta fuera del path MCP sigue igual. Confirmar que **los tests existentes que
  llaman `build_router(state)` no cambian** (con `mcp_path: None` no se monta nada). Ejecutar
  `cargo test --features mcp` y confirmar el fallo.
- [x] 5.2 GREEN: añadir a `AppState` (`src/service.rs:222`) el campo `mcp_path: Option<String>`
  poblado desde `Config` en `from_config`/`with_*` (bajo `#[cfg(feature = "mcp")]`, para no dejar
  código muerto en el build por defecto), **sin tocar la firma de `build_router`**
  (`src/http.rs:1334`); anidar el `StreamableHttpService` de `rmcp` con `nest_service` dentro de
  `protected` (reutiliza `require_api_key`, `src/http.rs:1276`) cuando el campo es `Some`, sobre el
  **mismo** listener. Verificación: `cargo test --features mcp` y `cargo test` (por defecto, sin
  cambios).
- [x] 5.3 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios; comprobar que **no** se abre un puerto adicional.

## 6. Tools de lectura (12)

- [x] 6.1 RED: tests por tool — `whoami`, `get_account`, `imap_status`, `list_mailboxes`,
  `select_mailbox`, `list_messages`, `get_message`, `get_message_body`, `list_attachments`,
  `get_attachment`, `idle_status`, `idle_queue` — verificando que el resultado **coincide** con el de
  su ruta HTTP equivalente (misma fuente de verdad) y que `list_messages` respeta los topes
  (defecto 50, máximo 200). Ejecutar `cargo test --features mcp` y confirmar el fallo.
- [x] 6.2 GREEN: implementar las 12 tools delegando en `src/service.rs`, con `inputSchema`
  derivado por `schemars` mediante el derive **condicional**
  `#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]` sobre los tipos de `src/service.rs`
  (opcionales opcionales), de forma que `schemars` siga siendo opcional. Verificación:
  `cargo test --features mcp` y `cargo tree` (sin la feature) sin `schemars`.
- [x] 6.3 GREEN: garantizar que la metadata de adjuntos **no** lleva bytes (`get_attachment` es la
  única que devuelve base64, con los mismos topes que la API). Verificación: test que lo comprueba.
- [x] 6.4 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios; sin lógica de dominio duplicada respecto a `src/http.rs`.

## 7. Tools de escritura (5)

- [x] 7.1 RED: tests por tool — `send_message`, `update_flags`, `move_message`, `copy_message`,
  `delete_message` — verificando equivalencia con la ruta HTTP y la semántica destructiva:
  `delete_message` marca `\Deleted` + `EXPUNGE` (no mueve a Trash) y exige `UIDPLUS`;
  `move_message` exige buzón destino explícito; `update_flags` cambia **solo** las banderas
  nombradas. Ejecutar `cargo test --features mcp` y confirmar el fallo.
- [x] 7.2 GREEN: implementar las 5 tools delegando en `src/service.rs`, con documentación de la
  semántica destructiva en la descripción de la tool. Verificación: `cargo test --features mcp`.
- [x] 7.3 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios.

## 8. Mapeo de errores y no-fuga de secretos

- [x] 8.1 RED: tests que fuerzan cada `ServiceError` y comprueban que el error de tool MCP lleva
  **el mismo código `error` estable** que la API HTTP y un mensaje legible; y que ningún resultado,
  error ni log contiene la API key ni el password de la cuenta. Ejecutar `cargo test --features mcp`
  y confirmar el fallo.
- [x] 8.2 GREEN: mapear `ServiceError` → error de tool MCP reutilizando `public_message()`/`kind()`
  (sin narrar el valor). Verificación: `cargo test --features mcp`.
- [x] 8.3 REFACTOR: `cargo fmt --check` y `cargo clippy --all-targets --features mcp -- -D warnings`
  limpios.

## 9. Documentación

- [x] 9.1 Documentar en `README.md` cómo habilitar la feature `mcp`, las variables
  `APIMAIL_MCP_STDIO`/`APIMAIL_MCP_HTTP`/`APIMAIL_MCP_PATH` y **la configuración en cada cliente
  verificado** (opencode, Claude Code/Desktop, Cursor). Verificación: siguen los pasos tal cual y el
  servidor arranca en la modalidad descrita.
- [x] 9.2 Añadir en `docs/API.md` una nota de que MCP es una **capa de exposición** sobre la API
  existente (no un backend nuevo). Verificación: la nota es coherente con el mapeo de tools.
- [x] 9.3 Añadir las tres variables nuevas (`APIMAIL_MCP_STDIO`, `APIMAIL_MCP_HTTP`,
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
