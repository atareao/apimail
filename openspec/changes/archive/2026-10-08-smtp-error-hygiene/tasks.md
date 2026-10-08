# Tasks

## 1. Higiene de errores SMTP (TDD)

- [x] 1.1 RED:
  - En `tests/send.rs`, reforzar `smtp_failure_is_502`: hacer que el fake adjunte
    el centinela de terceros real (host `smtp.internal.example` + banner
    `550 rejected`, `"smtp.internal.example said: 550 rejected"`) y afirmar que el
    `message` del `502` es
    **exactamente** el mensaje estable (`"SMTP delivery failed"`) y que **no**
    contiene el centinela.
  - En `src/smtp.rs` (`#[cfg(test)]`), añadir tests unitarios de
    `SmtpError::public_message`/`kind`: cada variante rinde su mensaje/etiqueta y
    ninguno filtra el texto de terceros.
  - Ejecutar `cargo test` y confirmar el fallo (RED).

- [x] 1.2 GREEN:
  - Añadir `public_message(&self) -> &'static str` y `kind(&self) -> &'static str`
    a `SmtpError` en `src/smtp.rs`, con el mapeo por variante del `design.md`.
  - En `src/http.rs`, usar `error.public_message()` (no `error.to_string()`) al
    construir `SendError::Smtp` en `send_message`.
  - Ejecutar `cargo test` en verde.

- [x] 1.3 REFACTOR:
  - `just fmt`, `just clippy` (cero warnings) y `just test` en verde.

## 2. Cierre

- [x] 2.1 `openspec validate smtp-error-hygiene --strict` en verde, marcar las
  tareas completadas y `openspec archive smtp-error-hygiene` tras la aprobación
  e implementación.
