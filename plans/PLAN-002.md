# PLAN-002.md — apimail

## ✅ ESTADO DE CIERRE (2026-10-18)

**Este plan está IMPLEMENTADO, ARCHIVADO y FUSIONADO. La release sigue PENDIENTE.**

| Hecho | Evidencia |
|---|---|
| Change **archivado** | `openspec/changes/archive/2026-10-18-mcp-server/` — `proposal.md`, `design.md`, `tasks.md` y las specs nuevas `mcp-server`, `mcp-tools` y `mcp-auth` (+ un delta de `http-api`) |
| **Fusionado en `development`** | merge del **PR #54** (`ae9d751`, `feature/mcp-server` → `development`) |
| Entregado | feature de Cargo **`mcp` (default OFF)**; capa de servicio `src/service.rs` (2240 líneas); servidor `src/mcp.rs` (883 líneas) con las **17 tools** (12 de lectura + 5 de escritura); transportes **stdio** y **Streamable HTTP** anidado en el `axum` existente; job **`test-mcp`** del CI |
| Verificado | `cargo test` → **349**; `cargo test --features mcp` → **364**; `clippy`/`fmt`/`doc` en verde en ambos modos |
| **Release** | ⏳ **PENDIENTE** — **no está en ningún tag** (`git tag --contains ae9d751` no devuelve nada; el tag más reciente es `v0.4.4`): sin publicar en crates.io ni como imagen |

### Qué se desvió del plan (esto es lo valioso)

1. **Faltaba la feature `server` de `rmcp`.** El plan enumeraba `transport-io`,
   `transport-streamable-http-server`, `macros` y `schemars`, pero **no** `server`. En `rmcp 3.5.1`
   `#[cfg(feature = "server")]` **gatea** el módulo `transport`, los re-exports `ServerHandler` y
   `ServiceExt` y los de las macros: **sin ella el servidor MCP ni se puede nombrar**. Verificado con
   un crate sonda (sin `server`: `unresolved imports` / `cannot find transport in rmcp`); con
   `server`, compila y monta el transporte en `axum`. **Es obligatoria, no un extra.**
2. **El transporte HTTP reutiliza la auth en vez de OAuth.** El plan pedía **OAuth 2.1** (PKCE + DCR,
   flujo completo de `rmcp`) con la API key como camino degradado. Se entregó **solo la API key**
   (`Authorization: Bearer`) aplicada por el middleware existente, y **no** se activa la feature
   `auth` de `rmcp` (arrastraría un segundo `oauth2`/`reqwest`). ChatGPT queda fuera; es trabajo
   posterior. Resultado: **una sola frontera de auth**.
3. **La justificación del `base64` resultó falsa.** El argumento escrito para `default-features =
   false` era «el `default` de `rmcp` arrastra `base64 ^0.23` mientras el proyecto usa `base64 0.22`
   ⇒ dos `base64` en el árbol». **Medido, es inválido**: `base64 0.23.1` **ya estaba** en el árbol
   (vía `hyper-util`/`axum`) y `transport-streamable-http-server` → `server-side-http` **activa
   explícitamente** la feature `base64` de `rmcp`, así que el par `0.22.1 + 0.23.1` es **idéntico con
   y sin la feature** (`cargo tree -d` en ambos modos). La decisión se mantuvo por otros motivos
   (higiene de la feature `auth`), pero **el motivo escrito era falso y quedó retirado** en el
   `design.md` del change.
4. **La revisión del protocolo no es la que se iba a implementar.** El plan fijaba **`2026-07-28`**.
   Medido, el servidor **negocia `2025-11-25`**: en `rmcp 3.5.1` esa revisión
   (`ProtocolVersion::V_2026_07_28`) es a la vez `LATEST` y `NO_INITIALIZE` — SEP-2567 trasladó el
   ciclo de vida a `_meta` por petición —, así que a esa revisión **no queda handshake que negociar**
   y el servidor cae a `LATEST_WITH_INITIALIZE` (`V_2025_11_25`). Lo fijan dos tests de
   `tests/mcp_server.rs` (`tool_cycle_runs_in_process_over_a_pipe` y
   `binary_keeps_stdout_protocol_only`) contra la constante `NEGOTIATED_VERSION`: se asevera **lo que
   el SDK hace**, no lo que el cliente pide.
5. **Apareció un refactor que el plan no preveía**: extraer una **capa de servicio**
   (`src/service.rs`) para que los handlers HTTP y las tools MCP compartan DTOs y `ServiceError`.
   Sin él, las tools habrían duplicado la lógica de la API.

**Preguntas abiertas de §9 — resueltas:** MCP es **opt-in** (feature de Cargo, default OFF); entran
**17 tools** (12 de lectura + 5 de escritura) y **no** `resources`/`prompts`; la API key **convive**
sin OAuth; el endpoint por defecto es **`/mcp`** (`APIMAIL_MCP_PATH`), anidado en el mismo listener;
las operaciones administrativas/destructivas no se exponen.

**Pendiente de verdad:** la **release** (tag + crates.io + imagen) — §8 paso 6 / tasks 10.4 — y la
verificación con un **cliente MCP real** (§8 paso 5 / tasks 10.1, sin marcar).

---

> A partir de aquí, el plan **tal como se escribió**: registro histórico, no estado. Léelo con el
> cierre de arriba delante.

Trabajo planificado a partir de **`v0.3.0`** (+ `container-image`), ítem **#8 `mcp-server`** de
`plans/PLAN-001.md` §2.

Este documento **no sustituye a las specs de OpenSpec**: es planificación, no una propuesta
aprobada. **No se escribe ni una línea de código** (ni de tests, ni de Cargo.toml, ni de
documentación ejecutable) hasta que exista un change aprobado en
`openspec/changes/mcp-server/` y el usuario lo apruebe explícitamente (ver `AGENTS.md`).

---

## 1. Objetivo

Exponer la API HTTP de apimail como un **servidor MCP** (*Model Context Protocol*), de forma que
clientes MCP (ChatGPT, Claude Desktop/Code, Cursor, opencode, etc.) puedan leer, buscar, enviar y
mover correo de la cuenta configurada como *tools* (y, opcionalmente, *resources* / *prompts*),
sin que el usuario escriba HTTP a mano.

## 2. Alcance

- **Transportes**: **stdio** (uso local, cliente de escritorio) y **Streamable HTTP** montado en
  el mismo `axum` de apimail (uso remoto y clientes web).
- **Superficie de tools**, mapeada 1:1 sobre capabilities ya existentes (sin lógica de negocio
  nueva):
  - lectura/listado de buzones → `imap-mailboxes`;
  - listado y descarga de mensajes/partes → `imap-messages`, `mime-parsing`;
  - búsqueda avanzada por criterios IMAP estándar;
  - banderas (`\Seen`, `\Flagged`, `\Deleted`) + `EXPUNGE` → `imap-flags`;
  - envío saliente → `smtp-send`;
  - estado de la suscripción `IDLE` / cola → `imap-idle` (observabilidad, no control destructivo).
- **Autenticación**:
  - stdio: el proceso hereda el `APIMAIL_API_KEY`/credenciales del entorno del host;
  - Streamable HTTP: **OAuth 2.1** (flujo completo de `rmcp`) y, como camino degradado, la
    API key existente expuesta por cabecera (`Authorization: Bearer`).
- **Schemas**: `inputSchema`/`outputSchema` derivados de los tipos con **`schemars::JsonSchema`**.
- **Documentación**: cómo configurar el servidor en cada cliente soportado.

## 3. Fuera de alcance

- Implementar la **API de correo**: ya existe; MCP es una *capa de exposición*, no un backend nuevo.
- Transporte **HTTP+SSE legacy**: *non-goal* deliberado del SDK elegido (ver `rmcp`).
- **Roots / Sampling / Logging**: deprecados en la revisión vigente del protocolo.
- Escritura destructiva o administración del servidor (borrado de buzones, cambio de
  configuración, `POST /api/idle/start|stop` desde MCP) en la primera iteración.
- Un *marketplace*/registro público del servidor.

## 4. Decisiones técnicas

- **Revisión vigente del protocolo MCP**: **`2026-07-28`**. Núcleo *stateless* (sin
  `Mcp-Session-Id`), **depreca** Roots/Sampling/Logging, **depreca** el transporte HTTP+SSE legacy
  (mantenido ≥12 meses) y endurece OAuth 2.1. **Es una revisión con *breaking changes***: hay que
  implementar contra ella sabiendo que clientes antiguos pueden divergir.
- **SDK recomendado — `rmcp` 3.5.1** (publicado **2026-10-05**):
  - licencia **Apache-2.0**; repo oficial `modelcontextprotocol/rust-sdk` (**~4,0k ★**);
  - **MSRV 1.88**, que coincide con el `rust-version = "1.88"` del proyecto;
  - push diario, **~18,8M descargas/90 días**.
- **Transportes de `rmcp`**:
  - **stdio**: feature `transport-io`;
  - **Streamable HTTP servidor**: feature `transport-streamable-http-server`; expone un **Tower
    service**, que encaja con axum 0.8 mediante `nest_service` (no obliga a un servidor aparte);
  - cliente Streamable HTTP sobre `reqwest`;
  - **HTTP+SSE legacy = non-goal**; hay `with_json_response(true)` y `legacy_session_mode` como
    escotillas para clientes antiguos.
- **Herramientas**: macros `#[tool]`, `#[tool_router]`, `#[tool_handler]`; los esquemas
  `inputSchema`/`outputSchema` se derivan con **`schemars::JsonSchema`** (JSON Schema 2020-12),
  **no** con `utoipa`. Soporta además `resources`, `prompts`, `completions` y `subscriptions`.
- **Auth**: `rmcp` incluye **OAuth 2.1** completo (PKCE S256, RFC 8707/9728/8414, *Dynamic Client
  Registration* RFC 7591, refresh y *scope-upgrade*).
- **Clientes reales (verificado)**:
  - **ChatGPT exige OAuth 2.1 + DCR**: un Bearer simple **no** basta;
  - **opencode** acepta Streamable HTTP con cabecera **Bearer manual**;
  - Claude Desktop/Code y Cursor aceptan **stdio** y/o Streamable HTTP con OAuth.
- **Alternativa viable si `rmcp` no encaja**: **`rust-mcp-sdk` 2.0.0** (2026-08-27, licencia
  **MIT**, repo `rust-mcp-stack`, ~195 ★) — menos tracción, pero licencia distinta y API propia.
- **Re-verificación obligatoria antes de implementar**: la revisión del protocolo, la versión del
  SDK (`rmcp` / `rust-mcp-sdk`) y el conteo de estrellas/descargas cambian con rapidez. La spec
  debe fijar las versiones exactas contra las que se implementa.

## 5. Requisitos candidatos para la spec

A redactar en `openspec/changes/mcp-server/specs/<capability>/spec.md` como `## Requirement` /
`#### Scenario` (redacción definitiva en el change, no aquí):

- **`mcp-server` — servidor MCP**: el binario/servidor SHALL poder arrancar en modo stdio y en modo
  Streamable HTTP, con las mismas tools disponibles en ambos transportes.
- **`mcp-tools` — expuesto de capacidades**: cada capability de correo SHALL tener una tool
  correspondiente con esquema derivado (`schemars`); escenarios de *happy path*, entrada inválida,
  buzón/mensaje inexistente y error de conexión IMAP/SMTP.
- **`mcp-auth`**: en modo HTTP, SHALL aceptar OAuth 2.1 (PKCE + DCR) y, como *fallback*, la API key
  existente; escenarios de token ausente/expirado/scope insuficiente (401/403).
- **`mcp-streamable-http`**: el servicio SHALL montarse sobre el `axum` de apimail sin abrir un
  puerto adicional no configurado; escenarios de `POST`/`GET` del transporte y de negociación.
- **`mcp-error-mapping`**: los errores de dominio (`{error,message}`) SHALL mapearse a *tool
  errors* MCP legibles, sin filtrar secretos ni credenciales de cuenta.
- **Requisitos a **modificar** en specs existentes**: `startup-logging` (nuevo modo de arranque) y,
  si se exponen, `imap-idle` (datos de estado ya públicos vía `GET /api/idle/status`).

## 6. Riesgos

- **Breaking changes del protocolo (`2026-07-28`)**: clientes y SDKs van a remolque; puede forzar
  retrabajo y una versión mayor del proyecto.
- **Doble sistema de esquemas**: si además se hace `plans/PLAN-003.md` (OpenAPI), se convive con
  `schemars` (MCP, JSON Schema 2020-12) y `ToSchema` de `utoipa` para los **mismos** tipos ⇒ dos
  derivaciones que mantener sincronizadas (o tipos *mirror* explícitos).
- **Auth de ChatGPT**: exige OAuth 2.1 + DCR de verdad; no se puede "tirar" de API key para ese
  cliente. El *fallback* Bearer solo sirve a parte de los clientes.
- **Superficie de seguridad ampliada**: exponer correo a agentes externos multiplica el riesgo de
  fuga y de acciones no deseadas; hay que acotar tools destructivas y auditar.
- **Curva y estabilidad del SDK**: `rmcp` es joven y de evolución rápida (push diario); fijar
  versión y revisar CHANGELOG antes de cada *bump*.

## 7. Impacto

- **Capability nueva** (`mcp-server` + sub-capabilities) ⇒ specs nuevas y posible modificación de
  `startup-logging`.
- **`build_router(state) -> Router`** puede tener que devolver/encadenar el servicio MCP
  (`nest_service`) ⇒ cambio interno de firma.
- **Dependencias nuevas** (`rmcp`, `schemars`, y `reqwest` si hace falta cliente) ⇒ impacto en
  tiempo de compilación y en el tamaño del binario; `Cargo.toml` **no** se toca hasta el change.
- **Configuración nueva** (modo de arranque, puerto/ruta del transporte HTTP, parámetros OAuth)
  expuesta por entorno, *fail-closed* y con `GET /api/account`-style introspección si aplica.
- **Distribución**: documentación de configuración por cliente; posible sección en el `README.md`.

## 8. Secuencia propuesta

1. Redactar el change `openspec/changes/mcp-server/` (proposal + specs + design + tasks) y
   **esperar aprobación explícita**. Nada de código antes.
2. Fijar versiones (protocolo + SDK) y montar el esqueleto del servidor **stdio** con una sola
   tool trivial para validar el ciclo MCP.
3. RED→GREEN→REFACTOR: mapear las tools de lectura primero, luego escritura (envío, banderas),
   luego observabilidad `IDLE`.
4. Añadir el transporte **Streamable HTTP** sobre `axum` y la auth (OAuth 2.1 + *fallback* Bearer).
5. Documentar la configuración en los clientes verificados (ChatGPT, Claude, Cursor, opencode).
6. `openspec archive mcp-server` y PR a `development`; release a `main` solo con confirmación.

## 9. Preguntas abiertas

- ¿MCP es **siempre-opcional** (feature de Cargo / *runtime flag*) o parte del binario por defecto?
- ¿Qué conjunto mínimo de tools entra en la primera iteración y cuáles quedan fuera?
- ¿Se exponen también `resources` (p. ej. un mensaje como *resource*) y `prompts`?
- ¿Cómo encaja la **API key** con OAuth: conviven, o OAuth sustituye a la key en modo HTTP?
- ¿Se comparten tipos con `utoipa` (PLAN-003) o se admiten dos sistemas de esquema?
- ¿Ruta/segmento del endpoint MCP (p. ej. `/mcp`) y cómo convive con la auth actual?

---

## Definition of Done

La misma que `plans/PLAN.md` §7 y `plans/PLAN-001.md` §4: spec aprobada → TDD (RED/GREEN/REFACTOR)
verificado por CLI (`just test`, `just clippy`, `just fmt`) → revisión → `openspec archive
<mcp-server>` → PR a `development`. La release a `main` **solo** con confirmación explícita del
usuario.

**Requisito previo ineludible**: sin un change aprobado en `openspec/changes/mcp-server/` **no se
escribe código**. Este documento es planificación, no una propuesta aprobada.

---

## Fuera de alcance vigente

- Transporte HTTP+SSE legacy (deprecado por el protocolo; *non-goal* del SDK).
- Roots / Sampling / Logging (deprecados en `2026-07-28`).
- Cambios en la API HTTP existente o en `docs/API.md` como parte de este ítem.
- Acciones administrativas/destructivas sobre la cuenta expuestas como tools.
