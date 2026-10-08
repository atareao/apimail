# Proposal

## Why

En `src/http.rs:1297`, el handler `POST /api/messages` construye el fallo de
entrega así:

```rust
.map_err(|error| SendError::Smtp(error.to_string()))?;
```

`error` es un `SmtpError` cuyo `Display` **propaga texto de terceros** de
`lettre` (por ejemplo, `SmtpError::Delivery(String)` rinde
`"SMTP delivery failed: <texto del servidor>"`). Ese texto acaba tal cual en el
body del `502` que recibe el cliente. Es exactamente la fuga que la ruta IMAP ya
evita con `ImapError::public_message`/`kind` (ver `src/imap.rs`), pero SMTP se
quedó fuera.

El test `tests/send.rs:350` (`smtp_failure_is_502`) solo comprueba
`json["message"].is_string()`, así que la fuga queda **enmascarada**: el test
pasa aunque el mensaje contenga el texto del servidor. Es un bug real de
higiene de errores, no una capability nueva.

## What Changes

- Añadir a `SmtpError` dos métodos estables y sin texto de terceros, replicando
  el patrón ya existente en `ImapError`:
  - `public_message(&self) -> &'static str` — mensaje externo que usa la capa
    HTTP.
  - `kind(&self) -> &'static str` — etiqueta estática para **logs**.
- Usar `error.public_message()` (nunca `error.to_string()`) al construir
  `SendError::Smtp` en `src/http.rs`.
- Reforzar `tests/send.rs::smtp_failure_is_502` para afirmar que el `message` del
  `502` es el mensaje estable y **no** contiene el centinela de terceros del fake
  (`"smtp.internal.example said: 550 rejected"`), y añadir tests unitarios de
  `SmtpError::public_message`/`kind`.
- Delta sobre la capability existente `smtp-send`: se refuerzan dos requirements
  (`SMTP delivery and TLS` y `SMTP secret protection`). No se crea ninguna
  capability nueva.

## Capabilities

### New Capabilities
<!-- Ninguna: no se añade superficie funcional nueva, solo se endurece la higiene
de errores de una ruta existente. -->

### Modified Capabilities
- `smtp-send`: el `502` de `POST /api/messages` SHALL usar un mensaje estable e
  independiente del servidor (nunca el texto del servidor SMTP), y la protección
  de secretos SHALL extenderse a no reflejar texto producido por el servidor
  SMTP.

## Impact

- `src/smtp.rs`: métodos `SmtpError::public_message()` y `SmtpError::kind()` +
  tests unitarios bajo `#[cfg(test)]`. El `Display` de `SmtpError` se conserva
  para diagnóstico interno (fuera de HTTP).
- `src/http.rs`: `send_message` pasa a usar `error.public_message()` al construir
  `SendError::Smtp`.
- `tests/send.rs`: `smtp_failure_is_502` deja de ser laxo y comprueba el mensaje
  estable y la ausencia del centinela.
- Sin dependencias nuevas; sin cambios en `Cargo.toml`.
- Contrato HTTP estable: el status (`502`) y el código (`smtp_error`) no cambian;
  solo se fija el **texto** del `message`.
- **No es un hotfix de producción**: el código afectado todavía no está en `main`,
  así que no hay release urgente. Se entrega como hardening normal vía
  `development`.
- **Fuera de alcance**: el `serde_json` de un body malformado (`SendError::InvalidRequest`)
  queda como está: es texto estructural generado por el parser, no texto de
  terceros.
