# Tasks

## 1. Superficie pública de diagnóstico (TDD)

- [x] 1.1 RED: en `src/config.rs`, tests unitarios de `AccountError::public_message()/kind()` y
  `ConfigError::public_message()/kind()`: el mensaje nombra la variable pero **no** el valor, y
  `kind()` está en el conjunto esperado. En `src/http.rs`, tests unitarios de
  `AppStateError::public_message()/kind()`: delega en `SmtpError`/`ImapError` (p. ej.
  `SmtpError::Delivery("550 rejected by internal.example")` y
  `ImapError::Login("auth failed for user@internal.example")`), el mensaje no contiene ese
  texto y `kind()` es `smtp`/`imap`. Y en `tests/startup.rs`, un test que arranca el binario con
  un valor inválido (p. ej. `APIMAIL_PORT=<valor>`), espera salida no-cero y afirma que `stderr`
  contiene el nombre de la variable y **no** el valor. Ejecutar `cargo test` y confirmar el
  fallo de lo nuevo con el resto verde.
- [x] 1.2 GREEN: implementar `public_message()/kind()` en `AccountError`, `ConfigError` y
  `AppStateError` (delegando en `SmtpError`/`ImapError`) y pasar `src/main.rs` a registrar
  `kind` + `public_message()` en los dos sitios. Verificar `cargo test`.
- [x] 1.3 REFACTOR: `cargo fmt` y `cargo clippy --all-targets -- -D warnings` limpios.

## 2. Integración y verificación

- [x] 2.1 Ejecutar `just test`, `just clippy` y `just fmt` en verde.
- [x] 2.2 `openspec validate startup-log-hygiene --strict` en verde y `just check-spec` pasa
  con este change activo.

## 3. Revisión y cierre

- [x] 3.1 Revisar con `rust-reviewer` (foco: `public_message()` no filtra valores ni texto de
  terceros; los tests existentes de `tests/startup.rs` siguen verdes).
- [x] 3.2 Tras la aprobación e implementación: `openspec archive startup-log-hygiene`.
