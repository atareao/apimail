# imap-flags

Modificar las banderas de un mensaje (`PATCH .../flags`), moverlo (`POST .../move`),
copiarlo (`POST .../copy`) y borrarlo (`DELETE ...`) sobre la sesión IMAP perezosa,
componiendo `UID STORE`, `UID COPY`, `UID MOVE` y `UID EXPUNGE` sin permitir inyección
de comandos IMAP.

## Enlaces

- [`proposal.md`](./proposal.md) — por qué y qué cambia.
- [`design.md`](./design.md) — decisiones y riesgos.
- [`tasks.md`](./tasks.md) — checklist TDD.
- [`specs/imap-flags/spec.md`](./specs/imap-flags/spec.md) — delta de especificación.
- Referencia de formato: `openspec/changes/archive/2026-10-08-imap-messages/`.
- Specs consolidadas relacionadas: `openspec/specs/imap-messages/spec.md`,
  `openspec/specs/imap-connection/spec.md`.
