# PLAN-001.md — apimail

Trabajo planificado **después** de `v0.2.0`. El roadmap original (`plans/PLAN.md`) está
**completado**: las 9 capabilities se implementaron, revisaron y publicaron.

Este documento no sustituye a las specs de OpenSpec: cada ítem que toque código se detalla y
aprueba en `openspec/changes/<feature>/` **antes** de escribir código (ver `AGENTS.md`).

---

## 1. Punto de partida (v0.2.0)

- **Release publicada**: `v0.2.0` en crates.io y GitHub Releases (binarios `x86_64` y
  `aarch64`).
- **Roadmap completado**: `api-auth`, `mail-account`, `imap-connection`, `imap-mailboxes`,
  `imap-messages`, `imap-flags`, `mime-parsing`, `smtp-send`, `imap-idle`.
- **11 specs** consolidadas en `openspec/specs/`; **14 changes archivados** en
  `openspec/changes/archive/`; sin changes activos.
- **Calidad**: 270 tests, `clippy --all-targets -- -D warnings` en cero, `fmt` limpio.
- `development` y `main` sincronizadas.

Del roadmap y de las revisiones **no queda ninguna deuda pendiente**. Lo de abajo es trabajo
**nuevo**: dos defectos detectados tras la release, una mejora de empaquetado y tres
capabilities/extensions opcionales.

---

## 2. Ítems planificados

| # | Ítem | Tipo | Prioridad |
|---|---|---|---|
| 1 | `release-lockfile` — sincronizar `Cargo.lock` al liberar | Defecto (CI) | Alta (barata) |
| 2 | `packaging` — reducir el paquete de crates.io | Mejora de empaquetado | Alta (barata) |
| 3 | `webhook-delivery` — entrega *at-least-once* con cola persistente | Capability nueva | Media |
| 4 | `idle-scope` — `IDLE` multi-buzón / multi-cuenta | Capability nueva | Baja |
| 5 | `attachment-content` — contenido de adjuntos en el payload | Extensión de `imap-idle` | Baja |

### 1. `release-lockfile` — sincronizar `Cargo.lock` al liberar

- **Qué**: que el bump de versión de la release actualice también `Cargo.lock` (p. ej. un paso
  `cargo update --workspace`/`cargo check` en `release-prepare.yml` **después** de
  `vampus upgrade` y **antes** del `git add -A`), y sincronizar el lock actual.
- **Por qué**: hoy `origin/main` y `origin/development` tienen `Cargo.toml` con
  `version = "0.2.0"` pero `Cargo.lock` con `apimail 0.1.0`. El workflow hace el bump con
  `vampus` y nunca regenera el lockfile ⇒ **cualquier** `cargo build`/`cargo test` en un clon
  deja el árbol sucio (reproducido: `cargo test` cambió el lock a `0.2.0`).
- **Alcance**: `.github/workflows/release-prepare.yml` + la sincronización puntual de
  `Cargo.lock`.
- **Criterio de aceptación**: tras una release, `git status` queda limpio en un clon recién
  hecho y `Cargo.lock` refleja la misma versión que `Cargo.toml`.
- **Nota**: se publica como **patch** (`v0.2.1`); puede ir junto con el ítem 2.

### 2. `packaging` — reducir el paquete de crates.io

- **Qué**: añadir `exclude = [...]` a `[package]` en `Cargo.toml` para no publicar `openspec/`
  (14 changes archivados + 11 specs) ni otros artefactos de proceso.
- **Por qué**: `cargo publish --dry-run` empaqueta **142 ficheros / 1,1 MiB** (256 KiB
  comprimidos). `v0.1.0` y `v0.2.0` salieron igual: es ruido, no un fallo.
- **Alcance**: `Cargo.toml`; verificar con `cargo package --list` y `cargo publish --dry-run`.
- **Criterio de aceptación**: el paquete contiene solo `src/`, `tests/`, `Cargo.toml`,
  `README.md` y `LICENSE`.
- **Nota**: requiere su propio change y sale como **patch** (`v0.2.1`); puede ir junto con el
  ítem 1.

### 3. `webhook-delivery` — entrega *at-least-once* con cola persistente

- **Qué**: que ningún correo nuevo se pierda si el webhook está caído: cola persistente
  (p. ej. SQLite) + reintentos con backoff + *dead-letter* y `last_error` observable.
- **Por qué**: hoy `imap-idle` es **at-most-once** con 3 intentos (`WEBHOOK_ATTEMPTS`);
  documentado como *Fuera de alcance* en `openspec/specs/imap-idle/spec.md`.
- **A fijar en la spec**: almacenamiento y límites, orden de entrega, política de descarte,
  endpoints de inspección/reenvío y higiene de errores (códigos estables, sin texto de
  terceros).
- **Impacto**: capability nueva; dependencia de persistencia; el `IdleSupervisor` deja de
  "notificar y olvidar".

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
  memoria (parseo acotado, nada a disco).
- **Alternativa**: que el receptor descargue el adjunto con
  `GET /api/messages/{uid}/attachments/{id}` (ya existe); evaluar si compensa el ítem.

---

## 3. Secuencia recomendada

1. **#1 `release-lockfile` + #2 `packaging`** — dos correcciones pequeñas y aisladas que
   pueden ir en el mismo PR y publicarse juntas como **`v0.2.1`**; dejan la release limpia
   (lockfile coherente y paquete sin ruido).
2. **#3 `webhook-delivery`** — el de mayor valor real (fiabilidad de la notificación), con
   diseño propio de persistencia.
3. **#4** y **#5** — cuando haya necesidad real; hoy son *Fuera de alcance* conscientes.

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
