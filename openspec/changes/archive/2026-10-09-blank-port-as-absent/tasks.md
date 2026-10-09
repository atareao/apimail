# Tasks

## 1. Reproducir el defecto (evidencia)

- [x] 1.1 Arrancar el contenedor con `APIMAIL_IMAP_PORT` / `APIMAIL_SMTP_PORT`
  en blanco (p. ej. `podman compose up -d` con el `.env` del camino documentado,
  `cp .env.example .env`) y capturar el error real
  `ERROR failed to start apimail: invalid APIMAIL_IMAP_PORT: expected a number between 1 and 65535 kind="account"`
  antes de tocar código.
- [x] 1.2 Anotar la evidencia y confirmar que el bucle de reinicio se debe al
  puerto vacío y no a otro problema de la cuenta.

Evidencia (contenedor, **antes** del arreglo, con el change `container-image`
sobre esta rama y un `.env` que **no** define los puertos): `podman compose up -d`
entró en **bucle de reinicio** y los logs mostraron
`ERROR failed to start apimail: invalid APIMAIL_IMAP_PORT: expected a number between 1 and 65535 kind="account"`.
La causa es el puerto en blanco: `compose.yml` pasa `APIMAIL_IMAP_PORT=${APIMAIL_IMAP_PORT:-}`,
que se resuelve a una cadena vacía pero presente.

## 2. RED — tests que fallen

Los tests de `mail-account` viven en `src/config.rs`, bloque `#[cfg(test)] mod tests`
(a partir de la línea **653**), junto a `default_ports_follow_the_tls_mode`,
`explicit_port_overrides_the_default` e `invalid_port_variants_are_an_error`.

- [x] 2.1 Añadir el test del puerto IMAP en blanco con TLS `implicit`: para
  `""` y `"   "` en `APIMAIL_IMAP_PORT`, `MailAccount::from_lookup` carga sin
  error y `imap.port == 993`.
- [x] 2.2 Añadir el test del puerto SMTP en blanco con TLS `starttls`: para
  `""` y `"   "` en `APIMAIL_SMTP_PORT` (con `APIMAIL_SMTP_TLS=starttls`),
  `MailAccount::from_lookup` carga sin error y `smtp.port == 587`.
- [x] 2.3 Añadir la comprobación de que un valor **explícito** inválido sigue
  fallando: `abc` y `0` en `APIMAIL_IMAP_PORT` → `AccountError::InvalidPort`
  (los tests existentes `invalid_port_variants_are_an_error` y
  `default_ports_follow_the_tls_mode` deben seguir verdes).
- [x] 2.4 `cargo test` → confirmar el **ROJO**: los tests nuevos del blanco fallan
  con `AccountError::InvalidPort` mientras los existentes permanecen en verde.

Evidencia del **RED** (antes del arreglo): `cargo test blank_` →
`test config::tests::blank_imap_port_counts_as_unset ... FAILED` y
`test config::tests::blank_smtp_port_counts_as_unset ... FAILED`, con
`panicked at src/config.rs: AccountError::InvalidPort { name: "APIMAIL_IMAP_PORT", value: "" }`
(y el equivalente de SMTP). El módulo completo quedó en rojo:
`test result: FAILED. 57 passed; 2 failed`.

## 3. GREEN — cambio mínimo

- [x] 3.1 En `load_endpoint` (`src/config.rs`, líneas ~258-272), reutilizar el
  idioma `Some(raw) if !raw.trim().is_empty() => …` para que un puerto en blanco
  caiga a `tls.default_port(protocol)`, manteniendo `InvalidPort` para un valor
  explícito inválido (`abc`, `0`).
- [x] 3.2 `cargo test` → todo en verde.

Evidencia del **GREEN**: `cargo test` → **313 tests, 0 failed**. La suite tenía
311 tests; los 2 nuevos del puerto en blanco la llevan a 313.

## 4. REFACTOR

- [x] 4.1 `cargo fmt --check` → sin cambios pendientes.
- [x] 4.2 `cargo clippy --all-targets -- -D warnings` → cero warnings.
- [x] 4.3 Re-ejecutar `cargo test` para descartar regresiones.

Evidencia del **REFACTOR**: `cargo fmt --check` → exit 0;
`cargo clippy --all-targets -- -D warnings` → exit 0, cero warnings. Los tests de
validación legítima siguen verdes: `invalid_port_variants_are_an_error`,
`default_ports_follow_the_tls_mode`, `explicit_port_overrides_the_default`,
`startup::invalid_port_exits_non_zero` y `startup::invalid_mail_port_exits_non_zero`.

## 5. Verificar de punta a punta

- [x] 5.1 Reconstruir la imagen del change `container-image`.
- [x] 5.2 `podman compose up -d` con el `.env` sin los puertos → contenedor
  **sano** (sin bucle de reinicio); el puerto se deriva del TLS.

Evidencia de la verificación **end-to-end en contenedor** (change `container-image`
con compose + Dockerfile, `.env` que **no** define los puertos):

- **Antes del arreglo:** `podman compose up -d` → bucle de reinicio con
  `ERROR failed to start apimail: invalid APIMAIL_IMAP_PORT: expected a number between 1 and 65535 kind="account"`.
- **Después del arreglo:** `podman compose up -d --build` →
  `apimail | Up 9 seconds (healthy)`, `RestartCount = 0`, y en los logs
  `INFO apimail listening on 0.0.0.0:3000`.
- El `healthcheck` de `compose.yml` reportó `healthy`.
- `APIMAIL_PUBLISHED_PORT=3978 just health` → `{"status":"ok","name":"apimail","version":"0.3.2"}`.
- `APIMAIL_PUBLISHED_PORT=3978 just _verify-health` → `✅ Servicio sano tras 1 intento(s)`
  (la receta que antes **siempre** fallaba).
- El proceso corrió como `uid=1000` / `whoami=apimail` y `queue.jsonl` se creó con
  `-rw-------` (0600) dentro del volumen con nombre.

## 6. Cierre

- [x] 6.1 `openspec validate blank-port-as-absent --strict`.
- [x] 6.2 `openspec archive blank-port-as-absent` (fusiona el delta en
  `openspec/specs/mail-account/`).
- [x] 6.3 PR `fix/blank-port-as-absent` → `development` (PR #33).
