# PLAN-001.md — apimail

Trabajo planificado a partir de **`v0.3.0`**. El roadmap original (`plans/PLAN.md`) está
**completado**: las 9 capabilities se implementaron, revisaron y publicaron.

Este documento no sustituye a las specs de OpenSpec: cada ítem que toque código se detalla y
aprueba en `openspec/changes/<feature>/` **antes** de escribir código (ver `AGENTS.md`).

---

## 0. Estado actual (medido el 2026-10-18, no histórico)

| Dato | Comando | Valor |
|---|---|---|
| Tag más reciente | `git tag --sort=-v:refname \| head -1` | **`v0.4.4`** |
| `development` por delante de `main` | `git rev-list --count origin/main..origin/development` | **22** commits *(no están sincronizadas)* |
| Specs consolidadas | `ls -1 openspec/specs/ \| wc -l` | **15** |
| Changes archivados | `ls -1 openspec/changes/archive/ \| wc -l` | **24** |
| Changes activos | `openspec list` | **0** |
| Ficheros en `src/` | `ls -1 src/*.rs \| wc -l` | **11** |
| Ficheros en `tests/` | `ls -1 tests/*.rs \| wc -l` | **15** |
| Tests (build por defecto) | `cargo test` | **349** |
| Tests (feature `mcp`) | `cargo test --features mcp` | **364** |

**Estado de los ítems de §2:** #1 y #2 ✅ (`v0.2.1`), #3 ✅ (`v0.3.0`), **#6 ✅ (`v0.4.0`)**,
**#8 ✅ implementado y archivado — release pendiente**, #10 ✅ (`v0.4.4`).
**Pendientes: #4, #5, #7 y #9.** No queda nada más por hacer del resto.

---

## 1. Punto de partida (v0.3.0) — *snapshot*

- **Releases publicadas** hasta el cierre de `v0.3.0`: `v0.3.0`, `v0.2.1`, `v0.2.0` y `v0.1.0` en
  crates.io y GitHub Releases (binarios `x86_64` y `aarch64`); para la última versión,
  `git tag --sort=-v:refname`. *(El estado medido hoy es el de §0.)*
- **Roadmap completado**: `api-auth`, `mail-account`, `imap-connection`, `imap-mailboxes`,
  `imap-messages`, `imap-flags`, `mime-parsing`, `smtp-send`, `imap-idle`.
- **12 specs** consolidadas en `openspec/specs/`; **16 changes archivados** en
  `openspec/changes/archive/`; sin changes activos. *(Recuentos de `v0.3.0`; hoy: 15 y 24 — ver §0.)*
- **Calidad**: 311 tests, `clippy --all-targets -- -D warnings` en cero, `fmt` limpio.
  *(Snapshot de `v0.3.0`; hoy son 349 en el build por defecto y 364 con `--features mcp`.)*
- `development` y `main` sincronizadas. *(Cierto al cerrar `v0.3.0`; hoy `development` va 22 commits
  por delante de `main` — ver §0.)*

### Completado en `v0.2.1` (change `release-hygiene`, `skip_specs`)

| # | Ítem | Evidencia |
|---|---|---|
| 1 | `release-lockfile` | `release-prepare.yml` ejecuta `cargo update --workspace` tras el bump. Verificado en CI: en el tag `v0.2.1`, `Cargo.toml` **0.2.1** == `Cargo.lock` **0.2.1**. Antes el tag arrastraba el lock en la versión anterior y cualquier `cargo` dejaba el árbol sucio al clonar. |
| 2 | `packaging` | `exclude` en `[package]`. El paquete pasó de **142 a 26 ficheros** (1,1 MiB → 600 KiB; `crate_size` **263 KB → 129 KB**), verificado sobre el `.crate` real publicado: fuera `openspec/`, `plans/`, `.opencode/`, `.github/` y los ficheros de proceso. |

### Completado en `v0.3.0`

| # | Ítem | Evidencia |
|---|---|---|
| 3 | `webhook-delivery` | Entrega **at-least-once**: ingesta y entrega desacopladas (worker FIFO con backoff), cola **persistente opt-in** (`APIMAIL_QUEUE_PATH`, log JSONL con `fsync` y compactación atómica, permisos `0600`), **reanudación** desde el watermark y `GET /api/idle/queue`. Change archivado; 12 specs; **311 tests**. Publicado como **v0.3.0** (crates.io + GitHub Release; paquete de **28 ficheros**). |

Lo que queda abajo es trabajo **nuevo**: cuatro ítems pendientes (**#4**, **#5**, **#7** y **#9**),
de los cuales solo #4 y #5 son extensiones opcionales conscientes; dos ya se cerraron después de
escribir esto (**#6** completado en `v0.4.0`, **#8** implementado y archivado, release pendiente).

---

## 2. Ítems planificados

| # | Ítem | Tipo | Prioridad | Estado |
|---|---|---|---|---|
| 1 | `release-lockfile` — sincronizar `Cargo.lock` al liberar | Defecto (CI) | Alta (barata) | ✅ hecho en `v0.2.1` |
| 2 | `packaging` — reducir el paquete de crates.io | Mejora de empaquetado | Alta (barata) | ✅ hecho en `v0.2.1` |
| 3 | `webhook-delivery` — entrega *at-least-once* con cola persistente | Capability nueva | Alta (siguiente) | ✅ hecho en `v0.3.0` |
| 4 | `idle-scope` — `IDLE` multi-buzón / multi-cuenta | Capability nueva | Baja | ⏳ pendiente |
| 5 | `attachment-content` — contenido de adjuntos en el payload | Extensión de `imap-idle` | Baja | ⏳ pendiente |
| 6 | `container-image` — despliegue contenedorizado | Infraestructura de distribución | Alta | ✅ hecho en `v0.4.0` (imagen en GHCR; desplegada — ver §6) |
| 7 | `manifest-digest` — comprobar el digest del manifiesto en el enforcement | Endurecimiento de CI | Media | ⏳ pendiente (sin propuesta) |
| 8 | `mcp-server` — exponer la API como servidor MCP | Capability nueva / distribución | Media | ✅ implementado y archivado (`2026-10-18-mcp-server`, PR #54) — **release pendiente** |
| 9 | `openapi` — especificación OpenAPI + UI de documentación | Documentación / distribución | Media | ⏳ pendiente → `plans/PLAN-003.md` |
| 10 | `mailboxes-empty-response` — respuesta vacía transitoria en `GET /api/mailboxes` | Defecto (intermitente) | Media | ✅ hecho en `v0.4.4` — change `imap-session-liveness`, PR #50 |

### 3. `webhook-delivery` — entrega *at-least-once* con cola persistente

- **Qué**: que ningún correo nuevo se pierda si el webhook está caído o el proceso se reinicia:
  cola persistente, reintentos con backoff, *dead-letter*/**descarte** observable y reanudación
  de la suscripción donde se quedó.
- **Por qué**: hoy `imap-idle` es **at-most-once** con 3 intentos
  (`WEBHOOK_ATTEMPTS`); está documentado como *Fuera de alcance* en
  `openspec/specs/imap-idle/spec.md`.
- **A fijar en la spec**: mecanismo y ubicación de la persistencia (¿opt-in?), límites de la
  cola y política de descarte, semántica de reinicio (¿reanudar o saltar el backlog?),
  duplicados/idempotencia, endpoint de observabilidad y qué requisito de `imap-idle` hay que
  **modificar** (hoy la spec exige literalmente «no mail content is written to disk»).
- **Impacto**: capability nueva; dependencia de persistencia (o implementación sin
  dependencias nuevas); el `IdleSupervisor` deja de "notificar y olvidar".

### 4. `idle-scope` — `IDLE` multi-buzón / multi-cuenta

- **Qué**: suscribirse a varios buzones y/o varias cuentas a la vez (hoy: un buzón de una
  cuenta, `APIMAIL_IDLE_MAILBOX`).
- **Por qué**: caso de uso real (bandejas triage, varias cuentas); hoy es *Fuera de alcance*
  explícito.
- **Impacto**: el supervisor gestiona N conexiones dedicadas y `GET /api/idle/status` pasa de
  un objeto a una colección ⇒ **cambio de contrato** ⇒ requiere spec.

### 5. `attachment-content` — contenido de adjuntos en el payload

- **Qué**: opción para incluir el contenido de los adjuntos (base64) en el payload del webhook.
- **Por qué**: hoy los adjuntos van **solo con metadatos** (`id`, `filename`, `content_type`,
  `size`, `inline`, `content_id`) por diseño.
- **Impacto**: payload potencialmente muy grande ⇒ límites, tope por adjunto y cuidado con la
  memoria (parseo acotado).
- **Alternativa**: que el receptor descargue el adjunto con
  `GET /api/messages/{uid}/attachments/{id}` (ya existe); evaluar si compensa el ítem.

### 6. `container-image` — despliegue contenedorizado

- **Qué**: poder ejecutar y distribuir apimail como imagen de contenedor, sin compilarlo ni
  montar el *unit* de systemd a mano. Incluye el `Dockerfile` multi-stage, `compose.yml`,
  `.dockerignore`, la plantilla `.env.j2` (Jinja2) y su equivalente `.env.example`, el workflow
  `.github/workflows/image.yml` que publica en GHCR (**solo `linux/amd64`**), una sección de
  despliegue en el `README.md` y el arreglo de las recetas de contenedor y salud del `.justfile`
  (que hoy apuntan a la imagen y el contrato de salud de otro proyecto).
- **Por qué**: la distribución actual solo ofrece binario (GitHub Releases) y crate (crates.io);
  desplegar en un servidor obligaba a compilar y configurar el servicio a mano, y el `.justfile`
  heredado no aplicaba a apimail.
- **Estado**: ✅ **completado** — change `container-image` **archivado**
  (`openspec/changes/archive/2026-10-09-container-image`); se publicó como **minor `v0.4.0`**
  (`CHANGELOG`: «Distribuir apimail como imagen de contenedor»), con la imagen en GHCR
  (`linux/amd64`) y **desplegada en producción** (`v0.4.3`, digest
  `sha256:0f95967ed8d2b7f6461b57fbfe717e16e284adff11b32a2687a8319b7cef5176`).
  El arreglo del `.justfile` quedó **a medias**: las recetas de contenedor y salud sí se
  adaptaron a apimail, pero sobrevivieron las `frontend-*` (el proyecto no tiene
  `frontend/`); se retiran en la limpieza posterior.
- **Impacto**: infraestructura de distribución y documentación; **no** toca el crate (sin cambios
  de API, configuración ni Rust). Sin *BREAKING*.

---

### 7. `manifest-digest` — endurecer el enforcement contra el residual de compresión

- **Qué**: comprobar el **digest del manifiesto** (no solo que dos builds coincidan) para cubrir el
  residual documentado del change `reproducibility-enforcement`: dos builds con el mismo contenido
  pueden producir manifiestos con digest distinto si cambia el nivel o la biblioteca de compresión.
- **Por qué**: se demostró en local que `gzip -1` y `gzip -9` producen capas de tamaño distinto
  (payload de 20 MiB: 4.246.405 B vs 2.918.641 B) y, por tanto, manifiestos con digest distinto
  — deterministas por nivel, no aleatorios.
- **Estado**: ⏳ **no aprobado**; sin propuesta ni rama. Requiere un change en `openspec/changes/`
  antes de escribir código.

### 8. `mcp-server` — exponer la API como servidor MCP

- **Qué se hizo**: el servidor MCP (*Model Context Protocol*) completo: capa de servicio compartida
  (`src/service.rs`, 2240 líneas), servidor con **17 tools** (12 de lectura + 5 de escritura) en
  `src/mcp.rs` (883 líneas), **dos transportes** — stdio y Streamable HTTP anidado en el `axum`
  existente reutilizando la API key— y la **feature de Cargo `mcp`, por defecto OFF**, más el job
  **`test-mcp`** del CI. El plan detallado vive en `plans/PLAN-002.md`, que ya lleva su **estado de
  cierre** (incluidas las desviaciones respecto al plan).
- **Evidencia**: change **`mcp-server` archivado** como `openspec/changes/archive/2026-10-18-mcp-server/`
  (con las specs nuevas `mcp-server`, `mcp-tools` y `mcp-auth` y un delta de `http-api`), **fusionado
  en `development`** por el merge del **PR #54** (`ae9d751`). Verificado con
  `cargo test --features mcp`: **364 tests**, y **349** en el build por defecto.
- **Estado**: ✅ **implementado y archivado** — ⚠️ **release PENDIENTE**: **no está en ningún tag**
  (`git tag --contains ae9d751` no devuelve nada; el tag más reciente sigue siendo `v0.4.4`), así que
  no está publicado ni en crates.io ni como imagen. Falta también la verificación con un cliente MCP
  real (casilla 10.1 del `tasks.md` del change, sin marcar).

### 10. `mailboxes-empty-response` — respuesta vacía transitoria en `GET /api/mailboxes`

- **Qué**: en producción, `GET /api/mailboxes` devolvió **una vez** `200` con
  `{"mailboxes":[]}` (2026-10-10), cuando la cuenta tiene **5** buzones (`INBOX`, `Drafts`,
  `Sent`, `Trash`, `Spam`).
- **Por qué importa**: un `200` con la lista vacía es indistinguible de «la cuenta no tiene
  buzones». Un cliente puede darlo por cierto y actuar en consecuencia (p. ej. no archivar
  al no encontrar buzón destino) **sin ver ningún error**.
- **Evidencia**: la llamada vacía fue la **primera** tras horas de contenedor levantado
  (sesión IMAP ociosa). Las **2** llamadas inmediatamente siguientes devolvieron la lista
  completa y un *burst* posterior de **40** llamadas dio **40/40** con 5 buzones
  (**0** vacías). **(actualizado 2026-10-12)** Volvió a observarse, y esta vez se reprodujo.
- **Análisis del código** (hecho): `list_mailboxes` → `run_once` → `ensure_session` +
  `session.list(None, Some("*"))` (`LIST "" "*"`), con `map_err(...)?` y `try_collect()`
  que **propagan** los errores: no hay ninguna rama que traduzca un error a lista vacía.
  El `[]` solo puede proceder de (a) una respuesta `OK` del servidor sin ningún `* LIST`
  (protocolo correcto: «no hay buzones») o (b) un *stream* de respuestas que termina sin
  elementos (p. ej. `EOF` de una conexión muerta) sin error. **Se determinó que era (b).**
- **Reproducción (2026-10-12)**: con un servidor de mentira (par que cierra a medias, FIN sin
  RST, contra `async-imap` 0.12) se comprobó que el cierre limpio del par **con el buffer vacío**
  produce `Poll::Ready(None)` en `do_poll_next` (`src/imap_stream.rs`), y que los parsers
  `parse_noop`, `parse_mailbox`, `parse_ids` y `parse_mailboxes` —bucles `while let Some(...)`
  que terminan en `Ok`— confunden ese fin de stream con «el servidor no mandó nada»: `noop →
  Ok(())`, `select → Ok(exists=0)`, `uid_search → Ok(0 uids)`. La sonda de `ensure_session` era
  `session.noop()` (`src/imap.rs`), cuyo `Ok(())` no prueba nada. Evidencia de producción: la
  primera llamada devolvió `200` con **65 bytes** (`total: 0`, 06:30:11), el `WARN discarding dead
  IMAP session` apareció a continuación, y la reconexión devolvió **9 correos** (4973 bytes,
  06:30:59).
- **Arreglo (2026-10-12)**: sonda de vida honesta (`SessionHandle::noop` pasa por
  `run_command_and_check_ok("NOOP")`, de modo que el truncado da `Error::ConnectionLost`, ya
  clasificado como error de conexión, y la zombi se descarta y reconecta) más una **defensa en
  profundidad** (vía 2a: `confirm_liveness_for_empty` revalida la vida cuando el resultado de
  datos es vacío o por defecto). La vía (b) —un segundo parser de IMAP— se descartó por exigir
  reimplementar el parseo de `async-imap`; la race residual queda declarada en `design.md`
  (incluidos `fetch`/`store`/`uid_expunge`). Detallado y aprobado en el change
  `imap-session-liveness`; PR **#50** (`fix/imap-session-liveness` → `development`).
- **Decisión (2026-10-10)**: se **cierra sin intervención** en producción. Se descarta la prueba
  dirigida (cortar el socket IMAP ocioso) porque la condición observada —la primera llamada
  tras horas de sesión ociosa— no es reproducible a voluntad y un resultado negativo no
  aportaría evidencia concluyente. Se retoma **solo** si la respuesta vacía vuelve a
  observarse, entonces con datos frescos.
- **(actualizado 2026-10-12)**: la respuesta vacía **volvió a ocurrir**, así que se retomó según
  lo acordado y quedó corregido por el change `imap-session-liveness` (PR #50). El bloque de
  decisión anterior se conserva como registro de lo que se decidió entonces.
- **Estado**: ✅ **resuelto**, **publicado en `v0.4.4`** (PR #52 `development` → `main`; `release-prepare`
  determinó *patch*, `vampus upgrade --patch` → 0.4.4, changelog con `git-cliff`, commit
  `chore: release v0.4.4` (`063cd45`) y tag `v0.4.4`) y **desplegado en producción**. La imagen en
  GHCR (`v0.4.4`, `0.4`, `latest` y el ancla `sha-063cd45`) apunta al mismo digest
  `sha256:6d3ea18b406e…`.
- **Verificación en producción (co1)**: el contenedor `apimail` corre con el id de imagen
  `sha256:cdcff4ca3f4e…`, **idéntico al `config.digest` publicado** en el registro; `/api/health`
  responde `{"status":"ok","name":"apimail","version":"0.4.4"}`; `GET /api/mailboxes` devuelve los
  **5** buzones reales (`Drafts`, `Sent`, `Trash`, `Spam`, `INBOX`); `GET /api/messages?mailbox=INBOX&unseen=true&limit=50`
  devuelve **`total: 9`** con los 9 correos; el contenedor está `healthy` con 0 reinicios y el log no
  tiene ni un aviso `discarding dead IMAP session`. La imagen anterior (`sha256:5351b61b…`) sigue
  local en co1 como punto de vuelta atrás.
- **Cierre y seguimiento**: el defecto se cerró por la vía del **arreglo** (sonda de vida honesta +
  defensa en profundidad). Lo que queda es **vigilar que no aparezcan `503` nuevos**, porque el
  arreglo **endurece** la sonda de vida (ahora `NOOP` pasa por `run_command_and_check_ok`).

## 3. Secuencia recomendada

1. **Release del MCP pendiente** (#8): el código está en `development` (PR #54) pero **sin publicar**;
   falta la release a `main` (con confirmación explícita del usuario) y la verificación con un cliente
   MCP real.
2. **#4 `idle-scope`** y **#5 `attachment-content`** — cuando haya necesidad real; hoy son
   *Fuera de alcance* conscientes.
3. **#7 `manifest-digest`** y **#9 `openapi`** — sin propuesta todavía; requieren un change en
   `openspec/changes/` antes de escribir código.

---

## 4. Definition of Done

La misma que `plans/PLAN.md` §7: spec aprobada → TDD (RED/GREEN/REFACTOR) verificado por CLI
(`just test`, `just clippy`, `just fmt`) → revisión → `openspec archive <feature>` → PR a
`development`. La release a `main` **solo** con confirmación explícita del usuario.

---

## 5. Fuera de alcance vigente (decisiones conscientes)

- Correo crudo en el payload del webhook.
- `IDLE` sobre la sesión compartida (se usa una conexión dedicada por diseño).
- Los logs de `bind`/`serve` del binario (texto estándar de `std::io::Error`, sin secretos ni
  texto de terceros) — ver `openspec/specs/startup-logging/spec.md`.
