# Changelog
## [0.5.0] - 2026-10-18

### Documentation

- *(plans)* Registrar el release v0.4.4 del ítem #10
- *(openspec)* Propuesta del change mcp-server (pendiente de aprobación)
- *(openspec)* Corregir dos errores del diseño de mcp-server y registrar §1
- *(mcp)* Documentar el servidor MCP (§9)
- *(openspec)* Revalidar anclas, marcar tareas y archivar el change mcp-server (§10)
- Corregir el estado real del proyecto

### Features

- *(mcp)* Configuración fail-closed del servidor MCP (§3)
- *(mcp)* Servidor MCP por stdio con la primera tool (§4)
- *(mcp)* Transporte Streamable HTTP anidado en el axum existente (§5)
- *(mcp)* Derivar los esquemas JSON de los DTOs del servicio (§6, paso previo)
- *(mcp)* Las 12 tools de lectura (§6)
- *(mcp)* Las 5 tools de escritura (§7)

### Miscellaneous Tasks

- *(just)* Encadenar cargo doc en check-all y anadir check-all-mcp

### Other

- *(mcp)* Andamiaje de la feature `mcp` (§1) + job de CI

### Refactor

- *(service)* Extraer la capa de servicio y migrar whoami, account e imap_status
- *(service)* Cerrar el patrón de la capa de servicio (oleada 0)
- *(service)* Migrar GET /api/mailboxes a la capa de servicio
- *(service)* Migrar las 7 lecturas restantes y cerrar dos huecos de mapeo
- *(service)* Migrar POST /api/messages (envío)
- *(service)* Migrar select, idle start/stop, flags, move y copy
- *(service)* Migrar DELETE y cerrar §2 (las 20 rutas)
## [0.4.4] - 2026-10-12

### Bug Fixes

- *(imap)* Detectar la sesión zombi en la sonda de vida

### Documentation

- Referencia exhaustiva de la API y planes de MCP y OpenAPI
- Añadir recetas y casos de uso a la referencia de la API
- Corregir el cuerpo de idle/start|stop y el alcance del 501
- Registrar el ítem #10 (mailboxes-empty-response)
- *(readme)* Enlazar la referencia exhaustiva de la API
- *(plans)* Cerrar el ítem #6 (container-image) como completado
- *(agents)* Corregir la nota obsoleta sobre el .justfile
- *(plans)* Registrar el cierre del ítem #10 (mailboxes-empty-response)
- *(openspec)* Cerrar tasks y corregir el ítem #10 de PLAN-001
- *(openspec)* Archivar el change imap-session-liveness

### Miscellaneous Tasks

- *(justfile)* Retirar las recetas frontend-* y arreglar check-all
- Release v0.4.4
## [0.4.3] - 2026-10-10

### Bug Fixes

- Imponer la reproducibilidad del build antes de publicar
- Tratar como fallo un ancla que no se puede comparar

### Documentation

- Proponer el enforcement de la reproducibilidad
- Registrar la verificación y los residuales del enforcement
- Cerrar y archivar el change reproducibility-enforcement
- Documentar el despliegue en producción y los pendientes

### Miscellaneous Tasks

- Release v0.4.3
## [0.4.2] - 2026-10-10

### Bug Fixes

- Hacer reproducible el build de la imagen

### Documentation

- Cerrar image-publish-resilience con la verificación del release
- Proponer builds reproducibles en la imagen
- Precisar el alcance de la verificación y sus límites
- Cerrar y archivar el change reproducible-image

### Miscellaneous Tasks

- Release v0.4.2
## [0.4.1] - 2026-10-10

### Bug Fixes

- Publicar la imagen pese al límite anónimo de Docker Hub

### Documentation

- Proponer el endurecimiento de la publicación de la imagen
- Registrar el dry-run verde del pipeline de imagen
- Precisar qué prueba la verificación del artefacto publicado

### Miscellaneous Tasks

- Release v0.4.1
## [0.4.0] - 2026-10-09

### Bug Fixes

- A blank mail port counts as unset

### Documentation

- Stop pinning a real version in the health example
- Propose treating a blank mail port as unset
- Archive blank-port-as-absent
- Archive the container-image change

### Features

- Distribuir apimail como imagen de contenedor

### Miscellaneous Tasks

- Keep the container infra out of the published crate
- Release v0.4.0
## [0.3.2] - 2026-10-09

### Documentation

- Align the health example and PLAN-001 with reality
- Describe what apimail actually does on crates.io

### Miscellaneous Tasks

- Release v0.3.2
## [0.3.1] - 2026-10-09

### Bug Fixes

- *(docs)* Clear the 19 rustdoc warnings and guard them in CI

### Documentation

- Mark PLAN-001 item 3 as released in v0.3.0
- *(openspec)* Add rustdoc-hygiene change proposal
- *(openspec)* Archive rustdoc-hygiene (no spec deltas)
- Stop calling apimail a work in progress

### Miscellaneous Tasks

- Release v0.3.1
## [0.3.0] - 2026-10-09

### Documentation

- Mark PLAN-001 items 1 and 2 as done in v0.2.1
- *(openspec)* Add and archive the webhook-delivery change
- Mark PLAN-001 item 3 as done

### Features

- *(idle)* Deliver webhook notifications through a durable queue

### Miscellaneous Tasks

- Release v0.3.0
## [0.2.1] - 2026-10-09

### Bug Fixes

- *(release)* Sync Cargo.lock on bump and keep process artifacts out of the crate

### Documentation

- Move the roadmap into plans/ and add PLAN-001.md
- *(openspec)* Add and archive the release-hygiene change

### Miscellaneous Tasks

- Release v0.2.1
## [0.2.0] - 2026-10-09

### Bug Fixes

- *(smtp)* Keep third-party SMTP error text out of the 502 body
- *(logs)* Keep startup error logs free of values and third-party text

### Documentation

- Add PLAN.md roadmap for the IMAP/SMTP CRUD (#5)
- *(openspec)* Add api-auth change proposal
- Document APIMAIL_API_KEY and authentication in README (#7)
- *(openspec)* Add and archive the mail-account change
- Document the mail account configuration and GET /api/account
- *(openspec)* Add and archive the smtp-send change
- Document POST /api/messages and APIMAIL_MAX_ATTACHMENT_BYTES
- *(openspec)* Add and archive the imap-connection change
- Document GET /api/imap/status and APIMAIL_IMAP_TIMEOUT_SECS
- *(openspec)* Add and archive the imap-mailboxes change
- Document GET /api/mailboxes and POST /api/mailboxes/select
- *(openspec)* Add and archive the imap-messages change
- Document GET /api/messages and GET /api/messages/{uid}
- *(openspec)* Add and archive the smtp-error-hygiene change
- *(openspec)* Add and archive the imap-flags change
- Document the flag, move, copy and delete endpoints
- *(openspec)* Add and archive the mime-parsing change
- Document the body and attachment endpoints
- *(openspec)* Add and archive the imap-idle change
- Document the IDLE endpoints and webhook configuration
- *(openspec)* Add and archive the startup-log-hygiene change

### Features

- *(auth)* Protect the API with a static Bearer API key
- *(mail)* Configure the IMAP/SMTP account from the environment
- *(smtp)* Send outgoing mail through the configured SMTP account
- *(imap)* Add lazy IMAP connection manager and GET /api/imap/status
- *(mailboxes)* List and select mailboxes over the IMAP session
- *(messages)* Search and fetch messages over the IMAP session
- *(flags)* Update flags, move, copy and delete messages over IMAP
- *(mime)* Parse message text, HTML and attachments over IMAP
- *(idle)* Watch the mailbox over IDLE and notify a webhook

### Miscellaneous Tasks

- Release v0.2.0
## [0.1.0] - 2026-10-07

### Features

- Axum HTTP skeleton with /api/health (+ SDD setup & gitflow CI) (#1)

### Miscellaneous Tasks

- Initial commit
- Add crates.io publish metadata (description, MIT license, README) (#3)
- Release v0.1.0
