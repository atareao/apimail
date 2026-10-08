# Tasks

## 1. Dependencias y configuración

- [x] 1.1 Añadir `lettre` (rustls) y `base64` a `Cargo.toml`; verificar con `cargo check`.
- [x] 1.2 RED: en `src/config.rs`, tests de `APIMAIL_MAX_ATTACHMENT_BYTES` — ausente → 10 MiB; valor válido se usa; `0` y no numérico → error que nombra la variable. Ejecutar `cargo test` y confirmar el fallo.
- [x] 1.3 GREEN: añadir `Config.max_attachment_bytes`, la constante de defecto (10 MiB) y la variante de error, encadenando la validación tras la cuenta. Verificar `cargo test`.
- [x] 1.4 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Dominio y transporte SMTP (TDD)

- [x] 2.1 RED: en `src/smtp.rs`, tests unitarios de `OutgoingMessage`/`OutgoingAttachment` — construcción y conversión a `lettre::Message` (remitente por defecto = usuario SMTP; `text`+`html` como alternative; adjuntos decodificados con su `content_type`; dirección inválida → error). Ejecutar `cargo test` y confirmar el fallo.
- [x] 2.2 GREEN: implementar `OutgoingMessage`, `OutgoingAttachment`, `SmtpError`, el trait `MailSender` (futuro boxeado) y `SmtpSender` sobre `AsyncSmtpTransport::<Tokio1Executor>` respetando el modo TLS (`relay`/`starttls_relay`/`builder_dangerous`+`Tls::None`). Verificar `cargo test`.
- [x] 2.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 3. Endpoint `POST /api/messages` (TDD)

- [x] 3.1 RED: crear `tests/send.rs` con un `MailSender` falso inyectado en `AppState` — envío mínimo `to`+`subject` → `200 {"status":"sent"}` y el falso recibe el remitente por defecto; `from` explícito se usa; sin `to` → `400`; dirección inválida → `400`; JSON malformado → `400`; `data_base64` inválido → `400`; adjuntos que superan el límite → `413`; el falso devuelve error → `502`; sin API key → `401`. Ejecutar `cargo test` y confirmar el fallo.
- [x] 3.2 GREEN: añadir el DTO con `Deserialize`, la conversión+validación a `OutgoingMessage`, el handler `send`, el `DefaultBodyLimit` de la ruta ajustado al límite, el campo `mailer`/`max_attachment_bytes` en `AppState` y la construcción del `SmtpSender` real en `AppState::from_config` (ahora `Result`); exponer `AppState::with_mailer` para inyectar el falso. Verificar `cargo test`.
- [x] 3.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios; documentar el contrato del endpoint en el comentario de módulo de `src/http.rs`.

## 4. Integración con el arranque

- [x] 4.1 Ajustar `src/main.rs` al nuevo `AppState::from_config` que devuelve `Result` (fallo → `ExitCode::FAILURE` con mensaje).
- [x] 4.2 Ajustar los tests existentes (`tests/health.rs`, `tests/auth.rs`, `tests/account.rs`) al nuevo constructor (`.expect(...)`), sin debilitar sus aserciones.
- [x] 4.3 Añadir a `tests/startup.rs` un caso de `APIMAIL_MAX_ATTACHMENT_BYTES` inválido → salida ≠ 0 y stderr menciona la variable.

## 5. Verificación de integración

- [x] 5.1 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 5.2 Confirmar que `just check-spec` pasa con este change activo.

## 6. Documentación y cierre

- [x] 6.1 Documentar `POST /api/messages` (cuerpo, adjuntos base64, códigos de respuesta) y `APIMAIL_MAX_ATTACHMENT_BYTES` en `README.md`.
- [x] 6.2 `openspec validate smtp-send --strict` en verde y `openspec archive smtp-send` tras la aprobación e implementación.
