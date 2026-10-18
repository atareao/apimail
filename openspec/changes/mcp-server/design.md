# Design

## Context

Estado real del código sobre el que se construye (verificado leyendo los ficheros):

- **Servidor y router**: `build_router(state: AppState) -> Router` en `src/http.rs:2158`. Construye
  `protected` con las rutas `/api/...` y le aplica
  `.route_layer(middleware::from_fn_with_state(state.clone(), require_api_key))`
  (`src/http.rs:2188`); después, `Router::new().route("/api/health", get(health)).merge(protected)
  .with_state(state)` (`src/http.rs:2194-2196`). **`GET /api/health` es la única ruta pública**;
  todo lo demás cuelga de `require_api_key` (`src/http.rs:2108`) — de ahí que *anidar* el servicio
  MCP dentro de `protected` reutilice la autenticación **sin código nuevo de auth**.
- **Estado y firma de `build_router`**: `AppState` (`src/http.rs:403`, `Debug` en `src/http.rs:434`)
  no conoce hoy nada de MCP, y `build_router(state: AppState)` solo recibe el estado. **Cambiar su
  firma rompería los 313 tests** que llaman `build_router(state)` (p. ej. `tests/health.rs:25`,
  `tests/auth.rs:32`). Por eso la configuración del transporte HTTP viaja **dentro de `AppState`**
  (ver Decisión 7): `build_router` conserva su firma y decide a partir del estado.
- **20 rutas** expuestas entre `protected` y `health`: `/api/health` (GET), `/api/whoami` (GET),
  `/api/account` (GET), `/api/imap/status` (GET), `/api/idle/start` (POST), `/api/idle/stop`
  (POST), `/api/idle/status` (GET), `/api/idle/queue` (GET), `/api/mailboxes` (GET),
  `/api/mailboxes/select` (POST), `/api/messages` (GET y POST), `/api/messages/{uid}`
  (GET y DELETE), `/api/messages/{uid}/flags` (PATCH), `/api/messages/{uid}/move` (POST),
  `/api/messages/{uid}/copy` (POST), `/api/messages/{uid}/body` (GET),
  `/api/messages/{uid}/attachments` (GET) y `/api/messages/{uid}/attachments/{id}` (GET). El mapeo
  a tools cuadra: **17 tools + health + idle/start + idle/stop = 20**.
- **DTOs**: los tipos de respuesta viven en `src/http.rs` — `EndpointView` (`617`), `AccountView`
  (`628`), `HealthResponse` (`637`), `WhoamiResponse` (`648`), `SentResponse` (`655`),
  `ImapStatusResponse` (`662`), `ImapUnavailableResponse` (`675`), `MailboxDto` (`686`),
  `MailboxListResponse` (`707`), `SelectMailboxDto` (`714`), `MailboxStatusResponse` (`721`),
  `MessageDto` (`845`), `MessageListResponse` (`875`), `MessageDetailResponse` (`893`),
  `MessageBodyResponse` (`961`), `ParsedAttachmentDto` (`977`), `AttachmentListResponse` (`1007`),
  `AttachmentResponse` (`1019`), `FlagUpdateDto` (`1256`), `MoveCopyDto` (`1290`),
  `FlagUpdateResponse` (`1354`), `MoveCopyResponse` (`1365`), `DeleteResponse` (`1378`),
  `SendMessageDto` (`1510`), `ErrorResponse` (`1586`). Solo `EndpointView`, `AccountView` y
  `HealthResponse` son `pub` hoy; el resto son privados del módulo y habrá que reubicarlos o
  exponerlos al extraer la capa de servicio.
- **Envoltorio de error**: `ErrorResponse { error: &'static str, message: String }`
  (`src/http.rs:1586`); el `401` usa `Unauthorized` (`src/http.rs:1635`) y `UnauthorizedResponse`
  (`src/http.rs:1639`) y emite `WWW-Authenticate: Bearer`.
- **Códigos `error` estables reales** (verificados por `grep` de los `ErrorResponse`/`error_response`
  en `src/http.rs`): `unauthorized` (`401`), `invalid_request` (`400`), `mailbox_not_found`
  (`404`), `message_not_found` (`404`), `attachment_not_found` (`404`), `message_too_large`
  (`413`), `payload_too_large` (`413`), `message_not_parsable` (`422`), `capability_not_supported`
  (`501`), `idle_not_configured` (`501`), `smtp_error` (`502`) e `imap_unavailable` (`503`).
  **`imap_tls` NO es un código**: es el nombre del campo `AppState.imap_tls: TlsMode`
  (`src/http.rs:423`) y un `kind()` de log (`"tls"`, `src/imap.rs:2117`); los fallos de TLS se
  exponen al cliente como `imap_unavailable`.
- **Paginación**: `parse_bounded_u32(..., 50, 200)` (`src/http.rs:1165`), invocado para `limit` en
  `src/http.rs:1220` ⇒ defecto **50**, máximo **200**. Son los topes que las tools deben respetar.
- **Sesión IMAP compartida**: `ConnectionManager` guarda la sesión en
  `session: tokio::sync::Mutex<Option<Box<dyn ImapSession>>>` (`src/imap.rs:1296`), serializada por
  un mutex asíncrono.
- **Logging**: `src/main.rs:13-16` instala `tracing_subscriber::fmt()...with_writer(std::io::stderr)`.
  **Verificado: los logs YA van a `stderr`**, así que el aislamiento de `stdout` en modo stdio no
  exige migrar el logging: solo hay que asegurarse de no escribir nada más en `stdout`. El cambio
  en `main.rs` es mínimo (documentarlo y servir el transporte).
- **Arranque**: `TcpListener::bind((config.host, config.port))` (`src/main.rs:33`) y
  `axum::serve(listener, build_router(state))` (`src/main.rs:58`).
- **Configuración**: `Config::from_env` (`src/config.rs:478`) delega en `Config::from_lookup`
  (`src/config.rs:497`), con `ConfigError` (`src/config.rs:344`) y `public_message()`/`kind()` que
  **nombran la variable sin exponer su valor**. **No existe hoy ningún parseo booleano**: todas las
  variables son String/puerto/tamaño/ruta/URL/Duración.
- **Dependencias actuales relevantes**: `axum 0.8.9`, `base64 0.22`, `reqwest 0.12`
  (`default-features = false`, rustls), `schemars` ausente. `rust-version = "1.88"`.
- **SDK elegido**: `rmcp` **3.5.1** (Apache-2.0, `rust-version 1.88`, repo
  `modelcontextprotocol/rust-sdk`). Su transporte HTTP es un **servicio Tower**
  (`rmcp::transport::streamable_http_server::{StreamableHttpService, StreamableHttpServerConfig}`),
  no un servidor propio ⇒ encaja con `axum::Router::nest_service`. `rmcp` depende de
  `schemars ^1.0` (opcional), compatible con `schemars 1.2.2`.

## Goals / Non-Goals

**Goals**

- Exponer las capacidades de correo ya existentes como **17 tools MCP** sin duplicar lógica de
  negocio: **una sola fuente de verdad** (la capa de servicio).
- Ofrecer **dos transportes** (stdio y Streamable HTTP) sobre el **mismo listener** `axum` y el
  **mismo estado**, con la **misma** autenticación por API key.
- Mantener el **build y el arranque por defecto intactos**: feature `mcp` **default OFF** y ninguna
  variable MCP ⇒ comportamiento actual sin cambios.
- Configuración **fail-closed** y localizada en `Config`, testeable sin tocar el entorno.

**Non-Goals**

- OAuth 2.1 / *Dynamic Client Registration* (ChatGPT): fuera de esta iteración (ver Decisiones 6).
- `resources`, `prompts`, `sampling` y el transporte HTTP+SSE legacy.
- Exponer operaciones administrativas/destructivas (`GET /api/health`, `POST /api/idle/start`,
  `POST /api/idle/stop`) como tools.
- Reescribir la API HTTP: los contratos publicados no cambian.

## Decisions

1. **Capa de servicio en proceso (`src/service.rs`) frente a auto-HTTP con `oneshot`.** Las tools
   comparten una capa de servicio nueva con los **mismos DTOs y errores de dominio**; los handlers
   de `src/http.rs` pasan a ser envoltorios delgados que mapean `ServiceError` → respuesta HTTP, y
   las tools mapean `ServiceError` → error de tool MCP. **Una sola fuente de verdad.**
   *Alternativa descartada*: que las tools invoquen el propio `Router` con
   `tower::ServiceExt::oneshot`. Reutiliza los handlers, pero obliga a un salto HTTP en bucle, a
   inyectar la API key en el proceso y a parsear códigos de estado — arquitectónicamente pobre y
   con un coste de serialización/deserialización por operación que no aporta nada.
   *Coste*: es un **refactor sensible** de código legacy con specs y 313 tests ⇒ el paso 2 de
   `tasks.md` lo aísla y exige que los tests existentes sigan en verde antes de añadir nada.
2. **`nest_service` de un servicio Tower de `rmcp` frente a un segundo listener.** El transporte
   `StreamableHttpService` se anida en el `axum::Router` existente (`nest_service`) bajo un path
   configurable (defecto `/mcp`), compartiendo listener y estado. *Alternativa descartada*: levantar
   un servidor HTTP aparte para MCP ⇒ puerto y proceso extra, dos capas de auth que sincronizar y
   un arranque más complejo, sin ninguna ventaja.
3. **Higiene de dependencias.** Se activa `rmcp` con `default-features = false` y **features
   explícitas**: `transport-io` (stdio), `transport-streamable-http-server`, `macros` y
   `schemars`. Los motivos:
   - **NO** se activa la feature `auth` de `rmcp`: arrastraría `oauth2 ^5` y `reqwest ^0.13.2`,
     mientras el proyecto ya usa `reqwest 0.12` para el webhook ⇒ **dos `reqwest` en el árbol**.
   - El `default` de `rmcp` incluye `base64 ^0.23`, mientras el proyecto usa **`base64 0.22`** ⇒
     usar `default-features = false` evita dos versiones de `base64`.
   - `schemars 1.2.2` comparte el **major** (`^1.0`) con la dependencia de `rmcp`: **sin duplicar
     tipos**.
   - Versiones a fijar en la implementación: `rmcp = "3.5.1"`, `schemars = "1.2.2"`.
   *Alternativa descartada*: **`rust-mcp-sdk` 2.0.0** (MIT, menos tracción, API propia); se
   prefiere el SDK de referencia del ecosistema MCP (`rmcp`).
4. **El buzón es un parámetro, no un estado global.** `select_mailbox` **sí** se expone como tool
   (es lo único que devuelve el estado del buzón: `total`/`unseen`/`uidnext`/`uidvalidity`), pero
   con la salvedad de que la sesión IMAP es **compartida y serializada por un `Mutex`**
   (`src/imap.rs:1296`): toda tool que necesite otro buzón lo **pasa explícitamente** y no depende
   de una selección previa implícita.
5. **Configuración fail-closed de las tres variables nuevas.** `APIMAIL_MCP_STDIO` (bool, def.
   `false`), `APIMAIL_MCP_HTTP` (bool, def. `false`) y `APIMAIL_MCP_PATH` (def. `/mcp`). Reglas:
   - Booleanas: se acepta `true`/`false` (**case-insensitive**) y `1`/`0`; cualquier otro valor ⇒
     error de configuración **nombrando la variable**. Es el **primer booleano de `Config`** (el
     proyecto ya parsea booleanos para *query params* con `parse_strict_bool`, `src/http.rs:1087`,
     p. ej. `flagged`, pero no para variables de entorno); se documenta la decisión de admitir
     `true`/`false`/`1`/`0` case-insensitive.
   - `APIMAIL_MCP_PATH` debe empezar por `/`, **no** puede ser `/` ni colisionar con el prefijo
     reservado `/api`.
   - `stdio` y `http` son **mutuamente excluyentes**: ambos a `true` ⇒ error al arrancar nombrando
     ambas variables. Esto es una **decisión de diseño, no un requisito del usuario**: un proceso
     MCP sirve **un** transporte (stdio **o** HTTP), y stdio ni siquiera enlaza un puerto, así que
     permitir ambos a la vez no aporta ningún caso de uso y sí dos caminos de arranque que probar.
     Si en el futuro se quisiera servir ambos a la vez, bastaría con **retirar la validación que lo
     prohíbe**, sin cambiar el resto del diseño.
   - Cualquier variable MCP definida con el crate compilado **sin** la feature `mcp` ⇒ error al
     arrancar nombrando la variable.
   - Sin ninguna variable MCP ⇒ comportamiento actual intacto.
6. **Sin OAuth en esta iteración (decisión explícita).** La feature `auth` de `rmcp` **no** se
   activa (ver Decisión 3). El transporte HTTP reutiliza el `Authorization: Bearer` existente,
   aplicado por `require_api_key` (`src/http.rs:2108`) dentro de `protected`. Se documenta que
   ChatGPT exige OAuth 2.1 + DCR y que eso queda para un change posterior.
7. **La configuración del transporte HTTP viaja en `AppState`, no en la firma de `build_router`.**
   `AppState` gana un campo `mcp_path: Option<String>` poblado desde `Config`
   (`Some(path)` = montar el transporte en ese path; `None` = sin transporte HTTP), de modo que
   **`build_router` conserva su firma** (`src/http.rs:2158`) y los **313 tests existentes no
   cambian** (construyen el estado con los constructores actuales, `AppState::from_config` /
   `with_services` / `with_mailer` / `with_idle`, y sin variables MCP el valor es `None`).
   *Concreto*: el campo y sus consumidores se declaran bajo `#[cfg(feature = "mcp")]`, así el build
   por defecto no contiene ni una línea de MCP y no aparecen warnings de código muerto; con la
   feature, `build_router` anida `crate::mcp::service(state.clone())` en `protected` cuando el campo
   es `Some`.
   *Alternativa descartada*: añadir un parámetro `mcp: Option<...>` a `build_router` ⇒ cambio de
   firma que obliga a tocar los 313 tests y todos los helpers de test, sin ninguna ventaja.
8. **`schemars` permanece opcional pese a que los derives `JsonSchema` recaen sobre tipos no
   gateados.** Hay una tensión real: las tools necesitan `inputSchema`/`outputSchema`, y la promesa
   es que el build por defecto **no** compila `schemars`. Resolución elegida: los tipos de
   entrada/salida de las tools viven en **`src/service.rs`** (junto a los DTOs que ya reutilizan) y
   llevan el derive **condicional**
   `#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]`; así `schemars` solo se referencia
   cuando la feature está activa y sigue siendo una dependencia **opcional** (`dep:schemars`).
   *Motivo*: mantiene **una sola fuente de verdad** (los mismos tipos sirven a la API HTTP y a los
   esquemas MCP) y **sin derivación duplicada**, coherente con la Decisión 1.
   *Alternativa descartada*: declarar tipos *mirror* de entrada/salida dentro del módulo gateado
   `src/mcp.rs` y convertir desde los DTOs del servicio. Evita `cfg_attr`, pero **duplica tipos** y
   reintroduce el riesgo de deriva entre DTO y esquema que la Decisión 1 quiere eliminar (habría que
   mantener campo a campo ambos conjuntos). Solo se adoptaría si `rmcp`/`schemars` obligaran
   a un derive inseparable del tipo, cosa que el derive condicional no exige.
   *Verificación de build* (en `tasks.md` §1): `cargo tree` **sin** la feature **no** debe listar
   `rmcp` ni `schemars`; con `--features mcp` sí.

## Risks

- **Breaking changes de la revisión de protocolo `2026-07-28`** (elimina sesiones `Mcp-Session-Id`
  por SEP-2567 ⇒ núcleo *stateless*): clientes y SDKs van a remolque y puede forzar retrabajo.
  *Mitigación*: fijar versión del SDK, leer la revisión desde la constante del propio SDK y
  revalidar antes de cada *bump*.
- **Superficie de seguridad ampliada**: exponer **escritura** (`send_message`, `update_flags`,
  `move_message`, `copy_message`, `delete_message`) a agentes externos multiplica el riesgo de fuga
  y de acciones no deseadas. *Mitigación*: reutilizar la API key (**una sola frontera de auth**),
  excluir las operaciones administrativas/destructivas, no emitir credenciales y documentar la
  semántica destructiva (borrado = `\Deleted` + `EXPUNGE`, no Trash).
- **Juventud de `rmcp`** (push diario, evolución rápida). *Mitigación*: fijar `rmcp = "3.5.1"`,
  revisar el CHANGELOG antes de actualizar y confirmar los nombres públicos usados.
- **Riesgo de regresión del refactor de `http.rs`**. *Mitigación*: TDD con tests de
  caracterización y el criterio de que los **313 tests existentes sigan en verde** sin cambiar
  ningún contrato HTTP.
- **Doble sistema de esquemas** si además se hace `plans/PLAN-003.md`: `schemars` (MCP, JSON Schema
  2020-12) conviviendo con `ToSchema` de `utoipa` para los **mismos** tipos. *Mitigación*: admitir
  una derivación por sistema y mantenerlas sincronizadas (o tipos *mirror* explícitos).
- **Aislamiento de `stdout` en modo stdio**: si algo escribiera a `stdout`, corrompería el canal del
  protocolo. *Mitigación*: los logs **ya** van a `stderr` (`src/main.rs:15`); basta con no añadir
  otra salida por `stdout`.

## Impact

Ficheros y firmas afectadas:

- `Cargo.toml` (**no se toca hasta implementar**): `[features] mcp = ["dep:rmcp", "dep:schemars"]`
  (default OFF) y deps **opcionales** `rmcp` (`default-features = false`, features
  `transport-io`, `transport-streamable-http-server`, `macros`, `schemars`) y
  `schemars = { version = "1.2.2", optional = true }`.
- `src/service.rs` (**nuevo**): capa de servicio con los DTOs y `ServiceError` compartidos. Los
  tipos de entrada/salida de las tools llevan el derive condicional
  `#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]`, de modo que `schemars` sigue siendo
  **opcional** (Decisión 8).
- `src/mcp.rs` (**nuevo**, tras `#[cfg(feature = "mcp")]`): servidor, transportes y las 17 tools.
- `src/http.rs`: `AppState` gana el campo `mcp_path: Option<String>` (poblado desde `Config`),
  declarado bajo `#[cfg(feature = "mcp")]`; **`build_router` conserva su firma** `(state: AppState)`
  y anida el servicio MCP en `protected` cuando el campo es `Some` (Decisión 7). Los handlers pasan
  a delegar en la capa de servicio.
- `src/config.rs`: campos `mcp_stdio`, `mcp_http`, `mcp_path`; parseo booleano y variantes de
  `ConfigError` (`public_message()` y `kind()`); de ellos sale el `mcp_path` que recibe `AppState`.
- `src/main.rs`: bifurcación stdio vs HTTP; en stdio **no** se crea `TcpListener` (`src/main.rs:33`).
- `src/lib.rs`: nuevo módulo `mcp` y re-exports (hoy lista módulos en `src/lib.rs:6-12`).
- `.github/workflows/ci.yml`: **job nuevo** (`test-mcp`) que compile y testee con
  `--features mcp`, **sin** alterar el job `test` por defecto (que sigue sin la feature).
- `README.md`, `docs/API.md`, `.env.example` y `.env.j2`: habilitación de la feature, variables
  `APIMAIL_MCP_*` y configuración por cliente.

Dependencias nuevas y su impacto: `rmcp` + `schemars` amplían el tiempo de compilación y el tamaño
del binario **solo** en el build con `--features mcp`; el build por defecto no las compila.

## A confirmar en implementación

- **Re-verificar la versión exacta de `rmcp`** en crates.io al empezar (la spec fija `rmcp =
  "3.5.1"`; el SDK evoluciona muy rápido).
- **Confirmar la revisión de protocolo desde la constante del SDK**: existe
  `ProtocolVersion::V_2026_07_28` en `rmcp-3.5.1/src/model/meta.rs`, y esa revisión elimina las
  sesiones (`Mcp-Session-Id`) por SEP-2567. Leerla de la constante y no de fuentes externas.
- Confirmar que los **nombres públicos** del transporte (`StreamableHttpService`,
  `StreamableHttpServerConfig`) y las **macros** (`#[tool]`, `#[tool_router]`, `#[tool_handler]`)
  se mantienen en la versión fijada.
