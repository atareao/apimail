# imap-idle

Suscribirse al buzón mediante la extensión **`IDLE`** (RFC 2177) sobre una **conexión IMAP
propia** —separada de la sesión compartida por el resto de rutas— y, al llegar correo nuevo,
hacer un `POST` JSON a un **webhook configurable** con los metadatos del mensaje y su
contenido parseado (texto, HTML y metadatos de adjuntos), reutilizando `mime-parsing`.

## Endpoints

| Método | Ruta | Respuesta |
|---|---|---|
| `POST` | `/api/idle/start` | `{"status":"running","mailbox":"INBOX"}` |
| `POST` | `/api/idle/stop` | `{"status":"stopped"}` |
| `GET` | `/api/idle/status` | `{"status":"running"\|"stopped","mailbox":"INBOX","last_error":null}` |

## Enlaces

- [`proposal.md`](./proposal.md) — por qué y qué cambia.
- [`design.md`](./design.md) — decisiones y riesgos.
- [`tasks.md`](./tasks.md) — checklist TDD.
- [`specs/imap-idle/spec.md`](./specs/imap-idle/spec.md) — delta de especificación.
- Referencia de formato: `openspec/changes/archive/2026-10-09-mime-parsing/`.
- Specs consolidadas relacionadas: `openspec/specs/imap-connection/spec.md`,
  `openspec/specs/mime-parsing/spec.md`, `openspec/specs/imap-messages/spec.md`.
