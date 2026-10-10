# PLAN-001.md — apimail

Trabajo planificado a partir de **`v0.3.0`**. El roadmap original (`plans/PLAN.md`) está
**completado**: las 9 capabilities se implementaron, revisaron y publicaron.

Este documento no sustituye a las specs de OpenSpec: cada ítem que toque código se detalla y
aprueba en `openspec/changes/<feature>/` **antes** de escribir código (ver `AGENTS.md`).

---

## 1. Punto de partida (v0.3.0)

- **Releases publicadas** hasta el cierre de `v0.3.0`: `v0.3.0`, `v0.2.1`, `v0.2.0` y `v0.1.0` en
  crates.io y GitHub Releases (binarios `x86_64` y `aarch64`); para la última versión,
  `git tag --sort=-v:refname`.
- **Roadmap completado**: `api-auth`, `mail-account`, `imap-connection`, `imap-mailboxes`,
  `imap-messages`, `imap-flags`, `mime-parsing`, `smtp-send`, `imap-idle`.
- **12 specs** consolidadas en `openspec/specs/`; **16 changes archivados** en
  `openspec/changes/archive/`; sin changes activos.
- **Calidad**: 311 tests, `clippy --all-targets -- -D warnings` en cero, `fmt` limpio.
- `development` y `main` sincronizadas.

### Completado en `v0.2.1` (change `release-hygiene`, `skip_specs`)

| # | Ítem | Evidencia |
|---|---|---|
| 1 | `release-lockfile` | `release-prepare.yml` ejecuta `cargo update --workspace` tras el bump. Verificado en CI: en el tag `v0.2.1`, `Cargo.toml` **0.2.1** == `Cargo.lock` **0.2.1**. Antes el tag arrastraba el lock en la versión anterior y cualquier `cargo` dejaba el árbol sucio al clonar. |
| 2 | `packaging` | `exclude` en `[package]`. El paquete pasó de **142 a 26 ficheros** (1,1 MiB → 600 KiB; `crate_size` **263 KB → 129 KB**), verificado sobre el `.crate` real publicado: fuera `openspec/`, `plans/`, `.opencode/`, `.github/` y los ficheros de proceso. |

### Completado en `v0.3.0`

| # | Ítem | Evidencia |
|---|---|---|
| 3 | `webhook-delivery` | Entrega **at-least-once**: ingesta y entrega desacopladas (worker FIFO con backoff), cola **persistente opt-in** (`APIMAIL_QUEUE_PATH`, log JSONL con `fsync` y compactación atómica, permisos `0600`), **reanudación** desde el watermark y `GET /api/idle/queue`. Change archivado; 12 specs; **311 tests**. Publicado como **v0.3.0** (crates.io + GitHub Release; paquete de **28 ficheros**). |

Lo que queda abajo es trabajo **nuevo**: solo dos extensiones opcionales.

---

## 2. Ítems planificados

| # | Ítem | Tipo | Prioridad | Estado |
|---|---|---|---|---|
| 1 | `release-lockfile` — sincronizar `Cargo.lock` al liberar | Defecto (CI) | Alta (barata) | ✅ hecho en `v0.2.1` |
| 2 | `packaging` — reducir el paquete de crates.io | Mejora de empaquetado | Alta (barata) | ✅ hecho en `v0.2.1` |
| 3 | `webhook-delivery` — entrega *at-least-once* con cola persistente | Capability nueva | Alta (siguiente) | ✅ hecho en `v0.3.0` |
| 4 | `idle-scope` — `IDLE` multi-buzón / multi-cuenta | Capability nueva | Baja | ⏳ pendiente |
| 5 | `attachment-content` — contenido de adjuntos en el payload | Extensión de `imap-idle` | Baja | ⏳ pendiente |
| 6 | `container-image` — despliegue contenedorizado | Infraestructura de distribución | Alta | 🚧 en curso |
| 7 | `manifest-digest` — comprobar el digest del manifiesto en el enforcement | Endurecimiento de CI | Media | ⏳ pendiente (sin propuesta) |

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
- **Estado**: 🚧 **en curso** — change
  [`container-image`](../openspec/changes/container-image/); se marca como hecho al fusionar.
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

## 3. Secuencia recomendada

1. **#4 `idle-scope`** y **#5 `attachment-content`** — cuando haya necesidad real; hoy son
   *Fuera de alcance* conscientes.

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
