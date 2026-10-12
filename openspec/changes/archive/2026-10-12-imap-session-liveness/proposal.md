# Proposal

## Why

`apimail` devuelve a veces un **resultado vacío válido** (HTTP 200) cuando en realidad **no pudo
preguntar**, porque la sesión IMAP cacheada llevaba horas muerta (el par cerró la conexión) y nadie
se enteró. Un `total: 0` o un `{"mailboxes":[]}` se vuelven **indistinguibles** de «no hay correos»
o «no hay buzones».

El defecto está **diagnosticado** y **reproducido** por tres vías

1. **`async-imap` 0.12** — `src/imap_stream.rs`, en `do_poll_next`: un cierre del par con el buffer
   vacío **no es un error**. Devuelve `Poll::Ready(None)` (fin de stream limpio), frente a
   `Ready(Some(Err(UnexpectedEof)))` cuando queda algún byte.
2. **Los tres bucles que consumen ese stream lo interpretan como «comando completado»**, porque
   todos son `while let Some(...) { ... }` y terminan en `Ok`:
   - `parse_noop` → `noop()` → `Ok(())`. **La sonda de vida de `ensure_session` no comprueba nada.**
   - `parse_mailbox` → `select()` → `Ok(Mailbox::default())` (`exists=0`).
   - `parse_ids` → `uid_search()` / `search()` → `Ok(HashSet::new())` (**cero resultados**).
   - `parse_mailboxes` → `list()` → lista vacía (**el mismo agujero que el ítem #10 de
     `plans/PLAN-001.md`**, `mailboxes-empty-response`, que tenía el diagnóstico abierto).
3. **Reproducido con un servidor de mentira** (par que cierra a medias, FIN sin RST) contra
   `async-imap` real:
   ```
   noop()       -> Ok(())
   select()     -> Ok(exists=0)
   uid_search() -> Ok(0 uids)  =>  total = 0
   ```
4. **Confirmado en producción** (access log de Traefik, `co1`):
   ```
   06:30:11  GET /api/messages?mailbox=INBOX&unseen=true&limit=50   200  65 bytes   106ms
   06:30:59  GET /api/messages?mailbox=INBOX&unseen=true&limit=200  200  4973 bytes 606ms
   ```
   65 bytes = `{"mailbox":"INBOX","total":0,"limit":50,"offset":0,"messages":[]}`. La segunda
   (606 ms) reconectó y devolvió 9 correos. Los logs de `apimail` (journald) muestran
   `WARN discarding dead IMAP session kind="imap"` **solo** en las tres segundas llamadas del
   episodio (07:46:35, 18:40:28, 06:30:59 del 11-12/10/2026), **nunca** en las primeras.

**Efecto en cadena**: el cliente de arriba (valet) recibe `total: 0` y reporta al usuario «0 correos
sin leer» —una mentira indistinguible de «el buzón está vacío»— y solo al insistir el usuario llega
la reconexión y la verdad (37, 28 y 9 correos). Lo mismo con `GET /api/mailboxes` (ítem #10: un
`200 {"mailboxes":[]}` con 5 buzones reales).

## What Changes

- **La sonda de vida de `ensure_session` (`src/imap.rs`) se endurece.** No puede conformarse con un
  stream truncado. `async_imap::Session` ya expone `run_command_and_check_ok`, cuyo
  `check_done_ok` convierte ese mismo fin de stream limpio en `Error::ConnectionLost`. La sonda
  pasa a usar esa vía (o equivalente), de modo que una sesión zombi se **descarta y reconecta
  antes** de ejecutar el comando de datos. La llamada de las 06:30:11 se convierte así en una
  reconexión honesta.
- **Defensa en profundidad**: que una operación cuyo stream termina **sin la respuesta etiquetada
  del comando** (p. ej. `* OK` de NOOP, `* OK [READ-WRITE]` de SELECT, la línea `tag OK` del
  comando) no pueda presentarse nunca como un resultado vacío o por defecto válido, si la API
  pública de `async-imap` lo permite (se decide en `design.md`).
- **Contrato de error, no de éxito**: una conexión cerrada por el par durante un listado de
  mensajes o de buzones se traduce en un **error** (`503 imap_unavailable`), nunca en una página
  vacía ni en `200 {"mailboxes":[]}`.
- **Cierre del ítem #10 de `plans/PLAN-001.md`** (`mailboxes-empty-response`): su diagnóstico
  abierto («`[]` por protocolo correcto o por stream que termina sin elementos») queda resuelto
  como *stream que termina sin elementos*, y se cubre con escenario y test.

## Capabilities

### New Capabilities
<!-- Ninguna. -->

### Modified Capabilities
- `imap-connection`: la detección de sesión muerta debe ser real y un comando truncado debe tratarse
  como fallo de conexión.
- `imap-messages`: un listado sobre una conexión cerrada por el par es un error, nunca una página
  vacía.
- `imap-mailboxes`: un listado de buzones sobre una conexión cerrada por el par es un error, nunca
  `200 {"mailboxes":[]}`.

## Non-goals

- **No se cambia el contrato de éxito** de `GET /api/messages` ni de `GET /api/mailboxes` para
  casos legítimos: una bandeja de verdad vacía sigue siendo `200` con lista vacía.
- **No se añaden reintentos que oculten fallos**: se reconecta cuando la sesión está muerta
  (comportamiento ya especificado), no se reintenta para disfrazar un `0`.
- **No se parchea `async-imap`** (dependencia externa): queda como propuesta *upstream* futura,
  fuera del alcance de este change.
- **No se añade un guardia en el cliente de arriba** (valet) tipo «si `total == 0`, reintenta»:
  enmascara el problema y sigue sin distinguir «no hay» de «no pude».
- **No se toca producción.**

## Impact

- Modificados (código, en la fase de implementación; **no** en este change): `src/imap.rs`
  (sonda de vida de `ensure_session` y, si procede, la defensa en profundidad `run_once`/`run_command`).
- Specs modificadas: `openspec/specs/imap-connection/spec.md`,
  `openspec/specs/imap-messages/spec.md`, `openspec/specs/imap-mailboxes/spec.md`.
- Efecto: un resultado vacío deja de ser ambiguo; el cliente de arriba recibe un error honesto
  cuando el servicio no pudo preguntar, en vez de una página vacía creíble.

## Risks

- **Race residual**: si el par cierra **justo entre la sonda y el comando de datos**, la defensa en
  profundidad es la única barrera. Se documenta en `design.md` y se decide si queda cubierta.
