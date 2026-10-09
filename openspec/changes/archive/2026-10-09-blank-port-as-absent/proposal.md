# Proposal

## Why

Un puerto de correo **en blanco** aborta el arranque, aunque en todo el resto del
código un valor en blanco significa «no configurado»:

1. **Defecto reproducible.** `compose.yml` (del change `container-image`) pasa a
   la aplicación `APIMAIL_IMAP_PORT=${APIMAIL_IMAP_PORT:-}` y
   `APIMAIL_SMTP_PORT=${APIMAIL_SMTP_PORT:-}`. Cuando la variable no está
   definida en el entorno, esa sustitución produce una cadena **vacía pero
   presente** (no ausente). Con el camino documentado (`cp .env.example .env`),
   `podman compose up -d` entra en **bucle de reinicio** y registra:
   `ERROR failed to start apimail: invalid APIMAIL_IMAP_PORT: expected a number between 1 and 65535 kind="account"`.
2. **El punto exacto.** `src/config.rs`, función `load_endpoint` (líneas
   ~258-272): el puerto se lee con `match lookup(&port_var) { Some(raw) => raw.parse::<u16>()…, None => tls.default_port(protocol) }`.
   Una cadena vacía **no parsea** → `AccountError::InvalidPort` → el proceso falla
   en el arranque. El código solo contempla «presente y parseable» o «ausente»;
   nunca «presente pero en blanco».
3. **No es solo de `compose.yml`.** Cualquier despliegue que entregue variables
   **vacías** reproduce el mismo fallo: **Kubernetes ConfigMaps** con una clave
   vacía, **`systemd EnvironmentFile=`** con una línea `APIMAIL_IMAP_PORT=`, o
   **`docker run --env-file`**. La causa es del código, no del orquestador.
4. **Convención del propio código violada.** El crate ya declara «en blanco ≡
   ausente» en varios puntos y el puerto es su **única** excepción:
   - `APIMAIL_QUEUE_PATH`: `Some(raw) if !raw.trim().is_empty() => …, _ => None` (config.rs:535).
   - `APIMAIL_WEBHOOK_URL`: igual, con el comentario *«A blank value counts as absent, coherently with `required()`: an empty string means "not configured", not an invalid URL.»* (config.rs:563-568).
   - `APIMAIL_IDLE_MAILBOX`: `Some(raw) if !raw.trim().is_empty() => …, _ => DEFAULT_IDLE_MAILBOX` (config.rs:569-572).
   - `required()` (config.rs:213-217): un valor vacío o solo-espacios cuenta como ausente.

## What Changes

- Un `APIMAIL_IMAP_PORT` / `APIMAIL_SMTP_PORT` **en blanco** (vacío o
  solo-espacios) **SHALL** contar como **ausente** y caer al puerto derivado del
  modo TLS. Un valor explícito inválido (`abc`, `0`) **SHALL** seguir fallando.
- El arreglo se fija con **tests** en el bloque `#[cfg(test)]` de `src/config.rs`:
  puerto IMAP en blanco (`""` y `"   "`) con TLS `implicit` → carga con `993`;
  SMTP en blanco con TLS `starttls` → carga con `587`; y un valor explícito
  (`abc`, `0`) sigue produciendo `AccountError::InvalidPort`.
- **`compose.yml` no se toca.** Tras este arreglo su paso uniforme de variables
  (incluidas las vacías) es correcto; el change no introduce infraestructura.

Sin cambios de API ni de contrato hacia el cliente. Un puerto en blanco pasa de
**error** a «deriva del TLS», que es exactamente lo que ya significa «ausente»:
**no hay BREAKING**.

## Capabilities

### New Capabilities
<!-- Ninguna: el cambio corrige un requisito existente de mail-account. -->

### Modified Capabilities
- `mail-account`: el requisito **Port validation** acepta un valor en blanco como
  «no configurado» y el requisito **Default ports** amplía su texto a «unset or
  blank».

## Impact

- `src/config.rs` (función `load_endpoint` y tests del bloque `#[cfg(test)]`).
- `openspec/specs/mail-account/spec.md` (spec destino, vía el delta de este change).
- Sin cambios en `compose.yml` ni en el resto del repo.
- Se publica como **patch**; commit `🐛 fix`.
