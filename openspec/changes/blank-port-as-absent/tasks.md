# Tasks

## 1. Reproducir el defecto (evidencia)

- [ ] 1.1 Arrancar el contenedor con `APIMAIL_IMAP_PORT` / `APIMAIL_SMTP_PORT`
  en blanco (p. ej. `podman compose up -d` con el `.env` del camino documentado,
  `cp .env.example .env`) y capturar el error real
  `ERROR failed to start apimail: invalid APIMAIL_IMAP_PORT: expected a number between 1 and 65535 kind="account"`
  antes de tocar código.
- [ ] 1.2 Anotar la evidencia y confirmar que el bucle de reinicio se debe al
  puerto vacío y no a otro problema de la cuenta.

## 2. RED — tests que fallen

Los tests de `mail-account` viven en `src/config.rs`, bloque `#[cfg(test)] mod tests`
(a partir de la línea **653**), junto a `default_ports_follow_the_tls_mode`,
`explicit_port_overrides_the_default` e `invalid_port_variants_are_an_error`.

- [ ] 2.1 Añadir el test del puerto IMAP en blanco con TLS `implicit`: para
  `""` y `"   "` en `APIMAIL_IMAP_PORT`, `MailAccount::from_lookup` carga sin
  error y `imap.port == 993`.
- [ ] 2.2 Añadir el test del puerto SMTP en blanco con TLS `starttls`: para
  `""` y `"   "` en `APIMAIL_SMTP_PORT` (con `APIMAIL_SMTP_TLS=starttls`),
  `MailAccount::from_lookup` carga sin error y `smtp.port == 587`.
- [ ] 2.3 Añadir la comprobación de que un valor **explícito** inválido sigue
  fallando: `abc` y `0` en `APIMAIL_IMAP_PORT` → `AccountError::InvalidPort`
  (los tests existentes `invalid_port_variants_are_an_error` y
  `default_ports_follow_the_tls_mode` deben seguir verdes).
- [ ] 2.4 `cargo test` → confirmar el **ROJO**: los tests nuevos del blanco fallan
  con `AccountError::InvalidPort` mientras los existentes permanecen en verde.

## 3. GREEN — cambio mínimo

- [ ] 3.1 En `load_endpoint` (`src/config.rs`, líneas ~258-272), reutilizar el
  idioma `Some(raw) if !raw.trim().is_empty() => …` para que un puerto en blanco
  caiga a `tls.default_port(protocol)`, manteniendo `InvalidPort` para un valor
  explícito inválido (`abc`, `0`).
- [ ] 3.2 `cargo test` → todo en verde.

## 4. REFACTOR

- [ ] 4.1 `cargo fmt --check` → sin cambios pendientes.
- [ ] 4.2 `cargo clippy --all-targets -- -D warnings` → cero warnings.
- [ ] 4.3 Re-ejecutar `cargo test` para descartar regresiones.

## 5. Verificar de punta a punta

- [ ] 5.1 Reconstruir la imagen del change `container-image`.
- [ ] 5.2 `podman compose up -d` con el `.env` sin los puertos → contenedor
  **sano** (sin bucle de reinicio); el puerto se deriva del TLS.

## 6. Cierre

- [ ] 6.1 `openspec validate blank-port-as-absent --strict`.
- [ ] 6.2 `openspec archive blank-port-as-absent` (fusiona el delta en
  `openspec/specs/mail-account/`).
- [ ] 6.3 PR `fix/blank-port-as-absent` → `development`.
