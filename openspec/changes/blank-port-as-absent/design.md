# Design

## Context

- `load_endpoint` (`src/config.rs`) lee el puerto con
  `match lookup(&port_var) { Some(raw) => raw.parse::<u16>()…, None => tls.default_port(protocol) }`.
  Un valor **vacío pero presente** entra por la rama `Some(raw)`, no parsea y
  devuelve `AccountError::InvalidPort`; el proceso aborta en el arranque.
- Ese valor vacío es lo normal en despliegues por orquestador: `compose.yml`
  pasa `APIMAIL_IMAP_PORT=${APIMAIL_IMAP_PORT:-}` (se resuelve a `""` cuando la
  variable no está definida), y lo mismo hacen un ConfigMap de Kubernetes con
  una clave vacía, un `EnvironmentFile=` de `systemd` con la línea
  `APIMAIL_IMAP_PORT=`, o `docker run --env-file`.
- El crate ya tiene una convención clara: **un valor en blanco cuenta como
  ausente**. La aplican `required()` (config.rs:213-217), `APIMAIL_QUEUE_PATH`
  (config.rs:535), `APIMAIL_WEBHOOK_URL` (config.rs:563-568) y
  `APIMAIL_IDLE_MAILBOX` (config.rs:569-572). El puerto es la **única**
  violación de esa convención.

## Goals / Non-Goals

**Goals**
- Un `APIMAIL_IMAP_PORT` / `APIMAIL_SMTP_PORT` en blanco (vacío o
  solo-espacios) cuenta como **ausente** y cae al puerto derivado del modo TLS.
- Un valor explícito inválido (`abc`, `0`) sigue fallando con
  `AccountError::InvalidPort`.
- La convención «en blanco ≡ ausente» deja de tener excepciones en el puerto.

**Non-Goals**
- `APIMAIL_IMAP_TLS` / `APIMAIL_SMTP_TLS` en blanco **sigue** siendo inválido.
  Es el mismo patrón, pero `compose.yml` siempre le da un valor (`implicit`), así
  que se deja fuera para no ampliar el alcance.
- `APIMAIL_HOST` / `APIMAIL_PORT` (bind HTTP) quedan fuera: no forman parte de
  `load_endpoint` ni los entrega vacíos el camino de despliegue documentado.
- **No se cambia `compose.yml`** en absoluto: tras este arreglo su paso uniforme
  de variables es correcto.

## Decisions

1. **Reutilizar el idioma ya existente en el fichero**,
   `Some(raw) if !raw.trim().is_empty() => …, _ => default`, en lugar de inventar
   otro. Es el mismo patrón que ya usan `APIMAIL_QUEUE_PATH`, `APIMAIL_WEBHOOK_URL`
   y `APIMAIL_IDLE_MAILBOX`, así que el puerto pasa a ser coherente con ellos y
   el lector reconoce la regla sin documentación adicional.
2. **La regla vive en `load_endpoint`, no en un `trim` global.** No se envuelve
   `lookup` ni se normalizan todas las variables de entrada: sería un cambio de
   mayor superficie y afectaría a valores donde los espacios son legítimos (p. ej.
   un password). Cada lector decide si el blanco significa «ausente», y el del
   puerto debe hacerlo explícitamente.

## Alternatives Considered

- **Quitar `APIMAIL_IMAP_PORT` / `APIMAIL_SMTP_PORT` de `compose.yml`.** Rechazada:
  no arregla la clase de fallo (cualquier otro orquestador que entregue variables
  vacías lo reproduce) y deja los puertos **sin poder configurarse** desde `.env`
  / `.env.j2`.
- **Usar `env_file` en `compose.yml`.** Rechazada por las mismas razones y, además,
  porque una variable escrita vacía en ese fichero seguiría rompiendo: `env_file`
  no distingue «ausente» de «presente y vacío».
- **Tratar el blanco como `0`.** Descartada: `0` ya es un valor inválido por diseño
  (`InvalidPort`), y convertirlo en una derivación implícita difumina la frontera
  entre «no configurado» y «mal configurado». El significado correcto de «en
  blanco» es «ausente», no «cero».

## Risks / Trade-offs

- *Riesgo*: un `APIMAIL_IMAP_PORT` deliberadamente vacío deja de fallar y pasa a
  derivarse del TLS. *Impacto*: alguien que esperara que un valor vacío detuviera
  el arranque perdería esa señal. *Aceptación*: se acepta, porque «ausente» ya
  tiene un significado bien definido —derivar del TLS— y porque el fallo actual
  rompe **todo el arranque** en despliegues perfectamente válidos. Un valor
  explícito inválido (`abc`, `0`) sigue deteniendo el proceso.
- *Trade-off*: la regla del blanco no se aplica a `APIMAIL_IMAP_TLS` /
  `APIMAIL_SMTP_TLS`. Es deliberado (ver Non-Goals); no introduce inconsistencia
  mientras `compose.yml` siga dando un valor por defecto al modo TLS.

## Migration Plan

- Aditivo y de bajo riesgo: un cambio localizado en `load_endpoint` más tests.
  Sin migración de datos ni cambios de contrato.
- Rollback: revertir la rama `fix/blank-port-as-absent`.

## Verification

- Tests nuevos en `src/config.rs`: puerto IMAP en blanco (`""` y `"   "`) con TLS
  `implicit` → `993`; SMTP en blanco con TLS `starttls` → `587`; un valor
  explícito (`abc`, `0`) sigue dando `AccountError::InvalidPort`. Los tests
  existentes (`default_ports_follow_the_tls_mode`,
  `explicit_port_overrides_the_default`, `invalid_port_variants_are_an_error`)
  siguen en verde.
- `cargo test` → todo en verde.
- `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` → sin avisos.
- De punta a punta: reconstruir la imagen del change `container-image` y comprobar
  que `podman compose up -d` (con `.env` sin los puertos) queda **sano** en vez de
  en bucle de reinicio.
