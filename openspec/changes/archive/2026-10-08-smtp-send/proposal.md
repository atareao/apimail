# Proposal

## Why

`mail-account` ya define y valida la cuenta SMTP, pero la API **no puede enviar
nada todavía**: el envío es la mitad "de escritura" del CRUD y la base de
cualquier caso de uso real (responder, notificar, reenviar con adjuntos). Además
es independiente del árbol IMAP, así que se puede entregar ya sin esperar a
`imap-connection`.

## What Changes

- Nueva capability `smtp-send`: endpoint protegido **`POST /api/messages`** que
  envía correo saliente con **`lettre`**.
- Petición **JSON** con `to` (obligatorio, ≥1), `cc`, `bcc`, `subject`, `text`,
  `html`, `from` (opcional) y `attachments` (adjuntos en **base64**).
- **Remitente**: `from` opcional en la petición; si falta, se usa el usuario
  SMTP configurado (`APIMAIL_SMTP_USER`).
- **TLS**: el envío respeta el modo configurado en `mail-account`
  (`implicit`/`starttls`/`none`) usando **rustls**.
- **Límite de adjuntos** configurable por entorno
  **`APIMAIL_MAX_ATTACHMENT_BYTES`** (por defecto **10 MiB**) con validación
  *fail-closed* al arranque; el tamaño total de los adjuntos decodificados
  acotado por ese valor.
- **Contrato de errores**: `400` para peticiones inválidas, `413` si se supera el
  límite de tamaño, `502` si el servidor SMTP falla y `401` si falta la API key.
- Los secretos SMTP **nunca** se registran ni se exponen (tampoco en `Debug`).

> Nota sobre el límite: la opción aprobada hablaba de `400` para validación; se
> añade explícitamente **`413 Payload Too Large`** para el caso de tamaño
> excedido, por ser el código correcto. A confirmar en la revisión de la spec.

## Capabilities

### New Capabilities
- `smtp-send`: envío de correo saliente por SMTP (JSON + adjuntos base64), límite
  de tamaño configurable y contrato de errores del envío.

### Modified Capabilities
<!-- Ninguna: mail-account y http-api mantienen sus requisitos. -->

## Impact

- `Cargo.toml`: añadir `lettre` (rustls, sin `native-tls`) y `base64`.
- `src/config.rs`: `Config.max_attachment_bytes` y su validación.
- `src/smtp.rs` (nuevo): modelo del mensaje saliente, trait `MailSender`,
  implementación `SmtpSender` con `lettre` y su error.
- `src/http.rs`: `AppState` incorpora el emisor (`Arc<dyn MailSender>`) y el
  límite; handler `POST /api/messages` con `DefaultBodyLimit` ajustado.
- `src/lib.rs`: reexportar los tipos públicos del envío.
- `src/main.rs`: la construcción del estado puede fallar (transporte SMTP).
- Tests: unitarios del mapeo mensaje/adjuntos; nuevo `tests/send.rs` con un
  emisor falso (sin red); `tests/*` existentes ajustados si cambia `AppState`.
- Fuera de alcance: colas/entrega asíncrona, reintentos, DKIM, plantillas,
  tracking, `Reply-To`/cabeceras personalizadas y verificación de la cuenta al
  arranque (no se abre red en startup).
