# PLAN.md — apimail

Roadmap de implementación de `apimail`: una **API HTTP (Axum)** que expone un CRUD completo sobre una cuenta de correo vía **IMAP** y **SMTP**, con parseo **MIME** y suscripción en tiempo real (**IDLE** + webhook).

Este documento define la secuencia de trabajo. No sustituye a las specs de OpenSpec: cada change se detalla y aprueba en `openspec/changes/<feature>/` antes de escribir código.

---

## 1. Estado actual (v0.1.0)

- Proyecto Rust **edition 2024**, un solo crate **lib + bin**:
  - `src/lib.rs` (`Config`, `build_router`)
  - `src/config.rs`
  - `src/http.rs`
  - `src/main.rs`
  - tests en `tests/`
- Servidor Axum con arranque configurable por entorno: `APIMAIL_HOST`, `APIMAIL_PORT`.
- Único endpoint operativo: `GET /api/health`.
- Capability OpenSpec `http-api` **ya especificada e implementada** (vive en `openspec/specs/`).
- **Gitflow** y **CI/CD** operativos.
- Release publicada: **v0.1.0** en crates.io y GitHub Releases.

Todavía **no existe**: autenticación de API, conexión IMAP, parseo MIME, envío SMTP ni IDLE/webhook.

---

## 2. Objetivo final

API HTTP que expone un CRUD completo sobre una cuenta de correo:

- **Leer, buscar, mover, copiar y clasificar** mensajes.
- Gestionar **banderas** (`\Seen`, `\Flagged`, `\Deleted`) y `EXPUNGE`.
- **Descargar y parsear MIME**: texto plano, HTML y adjuntos (metadatos + descarga).
- **Enviar** correo saliente por SMTP (con adjuntos).
- **Escuchar en tiempo real** con `IDLE`: al llegar un correo nuevo, hacer `POST` del mensaje completo + metadatos a un **endpoint configurable (webhook)**.

---

## 3. Decisiones transversales (fijadas)

| Área | Decisión |
|---|---|
| **Auth de la API** | **API key estática** en la cabecera `Authorization`, configurable por env (`APIMAIL_API_KEY`). Todo **excepto** `GET /api/health` queda protegido; sin clave válida → **401**. |
| **Credenciales IMAP/SMTP** | Por variables de entorno. **Nunca** en código ni en logs. |
| **Estructura** | Un solo crate (lib + bin). Organización por módulos de dominio: `config`, `imap`, `smtp`, `mime`, `http`/`routes`. |
| **Errores** | Modelo unificado con `thiserror` → respuestas **JSON**. |
| **Observabilidad** | `tracing` / `tracing-subscriber` para logs. |

---

## 4. Capabilities OpenSpec y secuencia de changes

La capability `http-api` (v0.1.0) ya existe. Las nuevas se añaden **incrementalmente**, en este orden:

| # | Capability | Descripción |
|---|---|---|
| 1 | `api-auth` | API key estática vía middleware Axum; `401` sin clave. **Protege todo lo demás.** |
| 2 | `mail-account` | Configuración de cuenta IMAP/SMTP por entorno (host, port, TLS, usuario, secreto) + validación al arranque. |
| 3 | `imap-connection` | Conexión y autenticación IMAP con TLS; gestión de sesión y reconexión. |
| 4 | `imap-mailboxes` | Listar y seleccionar buzones (opcionalmente crear/renombrar/borrar). |
| 5 | `imap-messages` | Listar y **buscar** mensajes con criterios estándar de IMAP (remitente, fecha, asunto, banderas UNSEEN/FLAGGED) + fetch de cabeceras, cuerpo MIME completo y partes. |
| 6 | `imap-flags` | Marcar `\Seen`/`\Flagged`/`\Deleted`, mover/copiar y `EXPUNGE`. |
| 7 | `mime-parsing` | Parseo con `mail-parser`: extraer texto plano, HTML y adjuntos (metadatos + descarga). |
| 8 | `smtp-send` | Envío de mensajes salientes con `lettre` (con adjuntos). |
| 9 | `imap-idle` | Extensión `IDLE`; al llegar un correo, `POST` del mensaje + metadatos al webhook configurable. |

Cada capability se desarrolla como un change `feature/<slug>`:

1. `openspec new change <feature>` → `proposal.md` → `specs/<capability>/spec.md` → `design.md` → `tasks.md`.
2. **Parar y esperar aprobación** del usuario.
3. RED → GREEN → REFACTOR con verificación por CLI.
4. `openspec archive <feature>` → la spec se consolida en `openspec/specs/<capability>/`.

---

## 5. Borrador de endpoints REST

> ⚠️ **Borrador a validar en cada spec.** Los paths, verbos, parámetros y contratos definitivos se fijan en el `specs/<capability>/spec.md` correspondiente antes de implementar.

| Método | Ruta | Notas |
|---|---|---|
| `GET` | `/api/health` | **Sin auth**. Ya existe (v0.1.0). |
| `GET` | `/api/mailboxes` | Lista de buzones. |
| `GET` | `/api/mailboxes/{mailbox}/messages` | Query params: `from`, `subject`, `since`, `before`, `unseen`, `flagged`, `limit`, `offset`. |
| `GET` | `/api/messages/{uid}` | Cabeceras o mensaje completo (p. ej. `?parts=full`). |
| `GET` | `/api/messages/{uid}/attachments` | Lista de adjuntos (metadatos). |
| `GET` | `/api/messages/{uid}/attachments/{id}` | Descarga de un adjunto concreto. |
| `PATCH` | `/api/messages/{uid}/flags` | Añadir/quitar `\Seen`, `\Flagged`, `\Deleted`. |
| `POST` | `/api/messages/{uid}/move` | Mover a otro buzón. |
| `POST` | `/api/messages/{uid}/copy` | Copiar a otro buzón. |
| `DELETE` | `/api/messages/{uid}` | Borrado + `EXPUNGE`. |
| `POST` | `/api/messages` | Envío SMTP. |
| `POST` | `/api/idle/start` | Inicia suscripción IDLE. |
| `POST` | `/api/idle/stop` (o `DELETE`) | Detiene suscripción IDLE. |

El **webhook** para IDLE se configura por entorno, p. ej. `APIMAIL_WEBHOOK_URL`.

---

## 6. Dependencias y orden (por qué)

- **`api-auth` primero**: sin auth, exponer operaciones de correo sería un agujero de seguridad. Todo lo demás queda detrás del middleware.
- **`mail-account` y `imap-connection` antes de cualquier operación**: no se puede listar, buscar ni marcar sin una sesión IMAP autenticada.
- **`imap-messages` antes de `imap-flags`**: primero leer/buscar/fetch; luego modificar estado.
- **`mime-parsing` antes de exponer adjuntos**: el fetch devuelve el cuerpo MIME crudo; el parseo lo convierte en texto/HTML/adjuntos utilizables.
- **`smtp-send`** es independiente del árbol IMAP, pero depende de `mail-account` (credenciales y config).
- **`imap-idle` al final**: depende de la conexión IMAP (`imap-connection`) **y** del webhook configurable; además reutiliza el parseo MIME para construir el payload.

---

## 7. Definition of Done (por change)

Un change se considera terminado cuando:

- [ ] Existe una spec OpenSpec **aprobada** en `openspec/changes/<feature>/`.
- [ ] Tests escritos en **RED** y llevados a **GREEN** (escenarios BDD cubiertos).
- [ ] `cargo test` en verde.
- [ ] `cargo clippy --all-targets -- -D warnings` con **cero warnings**.
- [ ] `cargo fmt --check` limpio.
- [ ] PR abierta desde `feature/*` contra `development`.
- [ ] `openspec archive <feature>` ejecutado → spec consolidada en `openspec/specs/<capability>/`.

---

## 8. Riesgos y consideraciones técnicas

- **TLS**: usar **rustls**; validar certificados y soportar puertos estándar (IMAP 993, SMTP 465/587).
- **Concurrencia IMAP**: `async-imap` exige sesión exclusiva → **una tarea por conexión**; reconexión con **backoff** ante desconexiones.
- **Credenciales**: solo por entorno; **redactar** secretos en logs de `tracing`.
- **IDLE**: gestionar reconexión y enviar **`DONE`** para salir limpiamente; evitar bloquear el runtime mientras se espera.
- **Adjuntos**: fijar **límites de tamaño** y evitar cargar en memoria payloads gigantes (streaming o límites explícitos).
- **Paginación**: `limit`/`offset` obligatorios en listados y búsquedas para no devolver buzones enteros.
- **API key**: comparación en tiempo constante para mitigar timing attacks.

---

## 9. Criterio de éxito global

- CRUD completo sobre la cuenta de correo **documentado** y **cubierto por tests**.
- Release publicada (siguiente versión semántica sobre v0.1.0).
- El **webhook recibe el correo nuevo** con el mensaje completo y su información/metadatos.
