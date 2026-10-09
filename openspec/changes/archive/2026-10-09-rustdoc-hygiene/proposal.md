# Proposal

## Why

Tres defectos de documentación, ninguno detectado por el CI actual:

1. **`cargo doc` emite 19 warnings de rustdoc** que el CI no ve, porque
   `rustdoc::broken_intra_doc_links` es un lint de **rustdoc**, no de clippy:
   `just clippy` (`cargo clippy --all-targets -- -D warnings`) pasa con **0 warnings** mientras
   docs.rs renderiza la página del crate publicado con 19 avisos. Inventario real (verificado
   ejecutando `cargo doc --no-deps`):
   - **16 enlaces a items privados** (hay que degradarlos a texto plano, `` `nombre` `` en vez de
     `` [`nombre`] ``):
     - `src/http.rs`: `require_api_key` en las líneas **17, 51, 95, 107, 119, 144, 187, 230, 276,
       2157**; `build_queue` en la línea **536** (doc de `with_idle`).
     - `src/imap.rs`: `quote_search_string` (línea **378**, doc de `SearchCriteria`);
       `Self::ensure_session` (línea **1376**, doc de `status`); `Self::run_once` y
       `Self::ensure_session` (líneas **1417** y **1418**, doc de `list_mailboxes`).
     - `src/smtp.rs`: `tls_strategy` (línea **196**, doc de `from_endpoint`).
   - **3 enlaces sin resolver** en `src/imap.rs` líneas **176** (`Imap`) y **178** (`TooLarge`,
     `Unparsable`): hay que resolverlos a la ruta correcta o degradarlos a texto plano si apuntan
     a algo privado.
2. **`AGENTS.md` desactualizado** (es el primer fichero que lee cualquier agente que entra al
   repo): su sección `## Estado actual (verificado)` afirma que lo implementado es solo
   `api-auth`, `mail-account` y `smtp-send`, y que **«Falta la conexión de lectura IMAP, el
   parseo MIME y la IDLE/webhook»** — pero las 9 capabilities del roadmap están implementadas y
   publicadas en `v0.3.0`. Además lista **4** ficheros de test (`health.rs`, `auth.rs`,
   `account.rs`, `startup.rs`) cuando en `tests/` hay **12** (`account.rs`, `auth.rs`, `flags.rs`,
   `health.rs`, `idle.rs`, `imap_status.rs`, `mailboxes.rs`, `messages.rs`, `mime.rs`, `queue.rs`,
   `send.rs`, `startup.rs`).
3. **Doc de portada del crate**: `src/lib.rs` se describe como
   `//! \`apimail\` — HTTP API skeleton for the mail service.` — el término *skeleton* no
   describe un crate con 9 capabilities, 311 tests y 4 releases.

## What Changes

- Degradar a texto plano los **16 enlaces a items privados** (`` [`x`] `` → `` `x` ``) en
  `src/http.rs`, `src/imap.rs` y `src/smtp.rs`, según el inventario del *Why*.
- Resolver los **3 enlaces sin resolver** de `src/imap.rs` (líneas 176 y 178) a la ruta correcta,
  o degradarlos a texto plano si el destino no es API pública.
- Reescribir el `//!` de portada de `src/lib.rs`: de *«HTTP API skeleton»* a una descripción que
  refleje el crate real (9 capabilities, lectura/escritura IMAP, SMTP, MIME, búsqueda, IDLE y
  webhook).
- Actualizar `AGENTS.md`: el estado real (las 9 capabilities implementadas y publicadas en
  `v0.3.0`, sin el «falta…») y el recuento de **12** ficheros de test.
- Añadir a `.github/workflows/ci.yml` (el workflow que ya ejecuta los tests en PRs) un paso de
  guarda `cargo doc --no-deps` con `RUSTDOCFLAGS="-D warnings"`, para que esto no vuelva a
  degradarse. Es la única forma de que el arreglo no se pierda: hoy el CI es ciego a este lint.

Solo **comentarios de documentación** (`//!` y `///`), `AGENTS.md` y un paso de CI. **Cero
cambios de comportamiento**: no se toca ninguna firma, ningún tipo exportado, ninguna lógica ni
ningún test.

Sin cambios de API ni de configuración. No hay **BREAKING**.

## Capabilities

### New Capabilities
<!-- Ninguna: cambio de documentación y de CI, sin comportamiento observable (§ skip_specs). -->

### Modified Capabilities
<!-- Ninguna. -->

## Impact

- `src/http.rs`, `src/imap.rs`, `src/smtp.rs`, `src/lib.rs`, `AGENTS.md` y
  `.github/workflows/ci.yml`.
- Efecto: `cargo doc --no-deps` con **0 warnings** (docs.rs limpio para el crate publicado),
  `AGENTS.md` describiendo el estado real y un paso de CI que impide la regresión.
- `skip_specs: true` en el change: no cambia ninguna spec.
- Se publica como **patch** (`v0.3.1`).
