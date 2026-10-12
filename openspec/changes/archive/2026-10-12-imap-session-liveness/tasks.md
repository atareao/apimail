# Tasks

## 1. Reproducir el estado actual (evidencia)

- [x] 1.1 Confirmar en `async-imap` 0.12 (`src/imap_stream.rs`, `do_poll_next`) que un cierre del
  par **con el buffer vacío** devuelve `Poll::Ready(None)` (fin de stream limpio), no un error;
  anotar la línea.
- [x] 1.2 Confirmar que los bucles `parse_noop`, `parse_mailbox`, `parse_ids` y `parse_mailboxes`
  consumen ese stream con `while let Some(...) { ... }` y terminan en `Ok(...)` sin exigir la
  respuesta etiquetada.
- [x] 1.3 Reproducir con un servidor de mentira (par que cierra a medias, FIN sin RST) contra
  `async-imap` real: `noop() -> Ok(())`, `select() -> Ok(exists=0)`, `uid_search() -> Ok(0 uids)`.
  Anotar la salida literal.
- [x] 1.4 Confirmar la sonda actual de `ensure_session` (`src/imap.rs`, ~línea 1341) usa
  `session.noop()` y que su `Ok(())` no prueba nada.
- [x] 1.5 Confirmar la evidencia de producción: primera llamada `200` con 65 bytes
  (`total: 0`) y segunda llamada (606 ms) reconectando y devolviendo 9 correos; el
  `WARN discarding dead IMAP session` solo aparece en la segunda.

## 2. RED — tests que fallan

- [x] 2.1 Test de la sonda de vida: con una sesión cuyo peer ha cerrado la conexión,
  `ensure_session` **descarta** la sesión y reconecta; la sesión zombi **no** se usa para el
  comando de datos. Ejecutar `cargo test` y comprobar que **falla** (la sonda actual no detecta
  la sesión muerta).
- [x] 2.2 Test de comando truncado: `uid_search()` / `select()` / `list()` sobre la conexión
  cerrada por el par devuelven **error**, no `total: 0` / `exists=0` / lista vacía. Ejecutar
  `cargo test` y comprobar que **falla**.
- [x] 2.3 Test de ruta: `GET /api/messages?mailbox=INBOX&unseen=true&limit=50` sobre una sesión con
  el par cerrado devuelve `503` `{"error":"imap_unavailable","message":"..."}`, **nunca** `200` con
  `total: 0`. Ejecutar `cargo test` y comprobar que **falla**.
- [x] 2.4 Test de ruta: `GET /api/mailboxes` sobre esa misma sesión devuelve `503`, **nunca** `200`
  con `{"mailboxes":[]}`. Ejecutar `cargo test` y comprobar que **falla**.
- [x] 2.5 Confirmar que los tests **existentes** de `imap-connection`, `imap-messages` y
  `imap-mailboxes` siguen en **verde** (solo fallan los nuevos).

## 3. GREEN — implementación mínima

- [x] 3.1 `src/imap.rs`: cambiar la sonda de vida de `ensure_session` para que use
  `run_command_and_check_ok` (o la vía equivalente que valide la respuesta final con
  `check_done_ok`), de modo que un stream truncado sin respuesta etiquetada dé
  `Error::ConnectionLost` y la sesión se descarte y reconecte.
- [x] 3.2 **Defensa en profundidad (si la API pública de `async-imap` lo permite)**: que las
  operaciones de datos cuyo stream termina sin la respuesta etiquetada del comando devuelvan error
  en vez de un resultado vacío/por defecto. Si **no** es implementable sin tocar `async-imap`,
  **no** se implementa y se deja dicho en `design.md`/reporte.
  - Se eligió la vía **2a** (`confirm_liveness_for_empty`): revalidar la vida cuando el resultado
    de datos es vacío o por defecto. La vía **(b)** (segundo parser de IMAP en las operaciones de
    datos) se descartó por exigir reimplementar el parseo de `async-imap`; la race residual queda
    declarada en `design.md`. Tras la revisión se extendió la residual a `fetch`/`store`/`uid_expunge`.
- [x] 3.3 Verificar que las rutas traducen el error de conexión a `503 imap_unavailable`
  (contrato ya existente; comprobar que no se rompe).
- [x] 3.4 Ejecutar `cargo test` → **todo en verde**.
- [x] 3.5 Ejecutar `cargo check` → sin errores.

## 4. REFACTOR

- [x] 4.1 Limpiar el código tocado sin cambiar comportamiento (nombres, mensajes de log).
- [x] 4.2 `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings` → sin avisos.
- [x] 4.3 Re-ejecutar `cargo test` para garantizar cero regresiones.
- [x] 4.4 Confirmar el contrato de éxito: un buzón de verdad vacío sigue dando `200` con lista
  vacía; los tests que lo cubren siguen en verde.
  - Cubierto por el test nuevo `legitimately_empty_mailbox_over_a_live_session_is_200` (buzón
    vacío legítimo sobre sesión viva → `200`, no `503`).

## 5. Migrar el ítem #10 de `plans/PLAN-001.md`

- [x] 5.1 Reconocer que el ítem #10 (`mailboxes-empty-response`) queda **resuelto**: el
  diagnóstico abierto («`[]` por protocolo correcto o por stream que termina sin elementos») se
  resuelve como *stream que termina sin elementos* (causa raíz común de este defecto).
- [x] 5.2 Actualizar `plans/PLAN-001.md`: pasar el ítem #10 de «⏳ pendiente (sin propuesta)» /
  «⏳ registrado y cerrado» a **resuelto por el change `imap-session-liveness`**, enlazando la
  evidencia y el escenario de `imap-mailboxes`.
- [x] 5.3 Quitar la nota de «**No reproducido**» y sustituirla por la reproducción con el
  servidor de mentira y la confirmación en producción.

## 6. Verificar y cerrar

- [x] 6.1 `openspec validate imap-session-liveness --strict` → válido.
- [x] 6.2 Revisar con `rust-reviewer` (sonda de vida, manejo de errores, no exponer secretos).
  - Veredicto: **no bloqueante**; sus dos hallazgos accionables (test del non-goal, `fetch` en la
    race residual) ya incorporados.
- [x] 6.3 Marcar las tareas y preparar el PR `fix/imap-session-liveness` → `development`.
  - PR **#50** (`fix/imap-session-liveness` → `development`).
