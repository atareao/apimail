# mime-parsing

Interpretar el mensaje MIME crudo que ya sabe descargar `imap-messages` para exponer su
**texto plano**, su **HTML** y sus **adjuntos** (metadatos y contenido), sin escribir nada
a disco.

## Endpoints

| Método | Ruta | Respuesta |
|---|---|---|
| `GET` | `/api/messages/{uid}/body?mailbox=INBOX` | `{"mailbox","uid","text","html"}` |
| `GET` | `/api/messages/{uid}/attachments?mailbox=INBOX` | `{"mailbox","uid","attachments":[…]}` |
| `GET` | `/api/messages/{uid}/attachments/{id}?mailbox=INBOX` | `{"mailbox","uid","id","filename","content_type","size","content_base64"}` |

## Enlaces

- [`proposal.md`](./proposal.md) — por qué y qué cambia.
- [`design.md`](./design.md) — decisiones y riesgos.
- [`tasks.md`](./tasks.md) — checklist TDD.
- [`specs/mime-parsing/spec.md`](./specs/mime-parsing/spec.md) — delta de especificación.
- Referencia de formato: `openspec/changes/archive/2026-10-09-imap-flags/`.
- Specs consolidadas relacionadas: `openspec/specs/imap-messages/spec.md`,
  `openspec/specs/imap-connection/spec.md`.
