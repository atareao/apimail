# smtp-error-hygiene

Higiene de errores del envío saliente: el `502` de `POST /api/messages` deja de
reflejar el texto del servidor SMTP (o de `lettre`) y pasa a usar un mensaje
estable y sin datos de terceros, replicando el patrón que ya existe en la ruta
IMAP (`ImapError::public_message`/`kind`).

- Objetivo: evitar que texto ajeno (banners, hosts, direcciones) acabe en el body
  del `502`; añadir `SmtpError::public_message()`/`kind()` y usarlo al construir
  `SendError::Smtp`.
- Alcance: `src/smtp.rs` (los dos métodos nuevos), `src/http.rs` (usar
  `public_message()` en `send_message`) y refuerzo de `tests/send.rs`. Sin nuevas
  dependencias y sin cambiar el contrato HTTP (status y `error` se mantienen).
- No es un hotfix de producción: el código afectado todavía no está en `main`.

Artefactos:

- [proposal.md](./proposal.md) — intención, alcance e impacto.
- [design.md](./design.md) — decisiones de mapeo y de higiene de errores.
- [tasks.md](./tasks.md) — checklist TDD (RED → GREEN → REFACTOR → cierre).
- [specs/smtp-send/spec.md](./specs/smtp-send/spec.md) — delta sobre `smtp-send`.
