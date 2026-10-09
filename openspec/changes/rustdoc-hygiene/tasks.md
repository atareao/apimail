# Tasks

## 1. Reproducir los warnings (evidencia)

- [x] 1.1 `cargo doc --no-deps` y contar **19** warnings (`broken_intra_doc_links`); anotar la
  evidencia y contrastarla con `just clippy` pasando con 0 warnings.

## 2. Degradar los 16 enlaces a items privados

- [x] 2.1 `src/http.rs`: degradar `[`require_api_key`]` a `` `require_api_key` `` en las líneas
  **17, 51, 95, 107, 119, 144, 187, 230, 276 y 2157**.
- [x] 2.2 `src/http.rs`: degradar `[`build_queue`]` a `` `build_queue` `` en la línea **536**
  (doc de `with_idle`).
- [x] 2.3 `src/imap.rs`: degradar `[`quote_search_string`]` a `` `quote_search_string` `` en la
  línea **378** (doc de `SearchCriteria`).
- [x] 2.4 `src/imap.rs`: degradar `[`ensure_session`](Self::ensure_session)` a
  `` `ensure_session` `` en la línea **1376** (doc de `status`).
- [x] 2.5 `src/imap.rs`: degradar `[`run_once`](Self::run_once)` y
  `[`ensure_session`](Self::ensure_session)` a texto plano en las líneas **1417** y **1418**
  (doc de `list_mailboxes`).
- [x] 2.6 `src/smtp.rs`: degradar `[`tls_strategy`]` a `` `tls_strategy` `` en la línea **196**
  (doc de `from_endpoint`).

## 3. Resolver los 3 enlaces sin resolver

- [x] 3.1 `src/imap.rs` línea **176** (`Imap`): resolver a la ruta correcta o degradar a texto
  plano si el destino es privado.
- [x] 3.2 `src/imap.rs` línea **178** (`TooLarge`, `Unparsable`): resolver a la ruta correcta o
  degradar a texto plano si el destino es privado.

## 4. Portada del crate y AGENTS.md

- [x] 4.1 `src/lib.rs`: reescribir el `//!` de portada para que describa el crate real (9
  capabilities; lectura/escritura IMAP, SMTP, MIME, búsqueda, IDLE y webhook), eliminando
  «skeleton».
- [x] 4.2 `AGENTS.md`: actualizar `## Estado actual (verificado)` — las 9 capabilities están
  implementadas y publicadas en `v0.3.0`, eliminar la frase «Falta la conexión de lectura IMAP,
  el parseo MIME y la IDLE/webhook».
- [x] 4.3 `AGENTS.md`: corregir el recuento de tests de 4 a **12** ficheros (`account.rs`,
  `auth.rs`, `flags.rs`, `health.rs`, `idle.rs`, `imap_status.rs`, `mailboxes.rs`, `messages.rs`,
  `mime.rs`, `queue.rs`, `send.rs`, `startup.rs`).

## 5. Guarda en CI

- [x] 5.1 `.github/workflows/ci.yml`: añadir un paso `cargo doc --no-deps` con
  `RUSTDOCFLAGS="-D warnings"` (nombre p. ej. «Check rustdoc warnings»), tras «Clippy» y antes de
  «Run tests».

## 6. Verificar

- [x] 6.1 `cargo doc --no-deps` termina con **0 warnings**.
- [x] 6.2 `just test` → **311** tests en verde.
- [x] 6.3 `just clippy` → 0 warnings.
- [x] 6.4 `just fmt` → sin cambios pendientes.
- [x] 6.5 Revisar el YAML de `ci.yml` (y con `actionlint` si está disponible).

## 7. Cierre

- [x] 7.1 Revisar los cambios de documentación (foco: no se toca ninguna firma, tipo, lógica ni
  test; los enlaces apuntan a API pública o quedan como texto plano).
- [x] 7.2 Marcar tareas y `openspec validate rustdoc-hygiene --strict`.
- [x] 7.3 PR `docs/rustdoc-hygiene` → `development`.
