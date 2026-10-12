# Design

## Context

- **El fallo vive en `async-imap`, no en `apimail`.** En `async-imap` 0.12,
  `src/imap_stream.rs`, `do_poll_next` trata un cierre del par **con el buffer
  vacío** como fin de stream limpio: `Poll::Ready(None)`. Solo si queda algún byte
  sin consumir devuelve `Ready(Some(Err(UnexpectedEof)))`. Es decir, el «FIN sin
  RST» del par se ve como un cierre ordenado.
- **Los tres consumidores de ese stream en `apimail` lo leen como «comando
  completado».** `parse_noop`, `parse_mailbox`, `parse_ids` y `parse_mailboxes`
  (en `async-imap`) recorren el stream con `while let Some(...) { ... }` y terminan
  en `Ok(...)` cuando el stream se agota. El fin de stream limpio no produce ni un
  `Err` ni una respuesta final, así que:
  - `noop()` → `Ok(())`: **la sonda de vida no comprueba nada**.
  - `select()` → `Ok(Mailbox::default())` con `exists = 0`.
  - `uid_search()` / `search()` → `Ok(HashSet::new())`: **cero UIDs**.
  - `list()` → lista vacía.
- **`async_imap::Session` ya expone la vía que sí distingue.** `run_command_and_check_ok`
  ejecuta un comando y valida la respuesta final con `check_done_ok`, que convierte
  el mismo fin de stream limpio en `Error::ConnectionLost`. `apimail` ya la usa en
  `src/imap.rs` para `STARTTLS` (línea ~1016), así que la vía existe y es la idiomática.
- **Confirmación en producción** (access log de Traefik): una llamada de 106 ms
  devolvió `200` con `total: 0` (65 bytes) y 48 s después la misma consulta devolvió
  9 correos (606 ms, reconexión real). El `WARN discarding dead IMAP session` aparece
  **solo** en la segunda llamada del episodio.

## Goals / Non-Goals

**Goals**
- Que la sonda de vida de `ensure_session` (en `src/imap.rs`) **distinga de verdad**
  una sesión viva de una zombi: una sesión cuyo par cerró la conexión se descarta y
  se reconecta **antes** de ejecutar el comando de datos.
- Que un comando cuya respuesta IMAP termina **antes de su respuesta etiquetada** se
  trate como fallo de conexión, nunca como un resultado vacío o por defecto válido.
- Cerrar el ítem #10 de `plans/PLAN-001.md` con escenario y test.

**Non-Goals**
- No cambiar el contrato de éxito de `GET /api/messages` / `GET /api/mailboxes` para
  casos legítimos (bandeja vacía real sigue siendo `200` con lista vacía).
- No añadir reintentos que oculten fallos.
- No parchear `async-imap` (queda como propuesta *upstream* futura).
- No tocar producción.

## Decisions

1. **Arreglo principal (elegido): la sonda usa `run_command_and_check_ok` (o
   equivalente), no `noop()`.**
   `run_command_and_check_ok` valida la respuesta final del comando; sobre una
   conexión cerrada por el par, `check_done_ok` la convierte en
   `Error::ConnectionLost` y la sonda falla. Entonces `ensure_session` descarta la
   sesión y reconecta. Es la vía que el propio crate usa para `STARTTLS`, así que no
   inventa mecanismo nuevo y no depende de parchear la dependencia.
   *Alternativa interna*: ejecutar `noop()` y **comprobar además** que el stream
   acabó en la respuesta etiquetada (`* OK`). Se descarta como arreglo principal
   porque re-implementa lo que `check_done_ok` ya hace; se conserva solo como la
   idea de la defensa en profundidad (decisión 2).

2. **Defensa en profundidad: un comando sin respuesta etiquetada no puede
   presentarse como resultado vacío válido.** Si la API pública de `async-imap` lo
   permite desde `apimail` (p. ej. envolviendo la llamada con la misma validación de
   `check_done_ok`, o ejecutando `select()`/`search()` por una vía que valide la
   respuesta final), se aplica a las operaciones de datos para que un truncado
   **posterior** a la sonda no se traduzca en `total: 0` / lista vacía. Si la API
   **no** lo permite sin tocar `async-imap`, esta defensa **no** se implementa y se
   documenta como dependencia de la decisión 1. La decisión exacta se toma en la
   fase de implementación, leyendo la API pública disponible; **el change no la
   exige** como condición de éxito.

   **Resuelto en implementación (GREEN).**
   - **(b) — ejecución propia validando la respuesta etiquetada: DESCARTADA.** La
     vía limpia exigiría ejecutar `SELECT`/`UID SEARCH`/`LIST` leyendo el stream
     hasta la respuesta etiquetada y **reconstruir** el resultado. Los parsers de
     `async-imap` 0.12 que lo hacen (`parse_mailbox`, `parse_ids`, `parse_mailboxes`,
     en `src/parse.rs`) son `pub(crate)`: inaccesibles desde `apimail`. Lo único
     publico es `Session::run_command` + `read_response`, que devuelven
     `ResponseData` **sin** interpretar; reconstruir UIDs, el estado del buzón o la
     lista de buzones obligaría a escribir un **segundo parser de IMAP** que
     mantener. Descartada por eso.
   - **(a) — revalidación de un resultado vacío: IMPLEMENTADA.** Cuando un comando
     de datos devuelve un resultado **vacío o por defecto** (`exists == 0` en
     `SELECT`, cero UIDs en `UID SEARCH`, lista vacía en `LIST`), `SessionHandle`
     vuelve a comprobar la vida de la sesión con la sonda honesta
     (`run_command_and_check_ok("NOOP")`, la de la decisión 1) antes de aceptar el
     vacío. Si la sonda falla, el error de conexión se propaga; si la sesión está
     viva y responde, el vacío legítimo se acepta sin cambios. Coste: **un**
     round-trip **solo** cuando el resultado es vacío. No convierte un buzón
     legítimamente vacío en error (non-goal preservado). No duplica protocolo ni
     cambia la forma del trait `ImapSession`.

3. **Race residual, explícita.** Queda una ventana entre la sonda de vida y el
   comando de datos: si el par cierra **justo en medio**, la sesión pasa la sonda y
   el comando de datos encuentra el stream truncado. La **opción (a)** de la
   decisión 2 la **cubre para el caso observado** (el par ya cerró → el stream se
   agota con el buffer vacío → el parser devuelve un resultado vacío o por defecto),
   porque el vacío desencadena una nueva sonda honesta que falla. Queda **abierta y
   declarada** la variante más estrecha de un truncado **a mitad de respuesta** que
   dejase un resultado **no vacío parcial** (el par envía unos bytes y cierra): (a)
   no la detecta, porque solo revalida resultados vacíos, y (b) era inviable sin
   forkear `async-imap`. No se finge cobertura que no exista.

## Alternatives Considered

- **(a) Guardia en el cliente de arriba (valet), tipo «si `total == 0`, reintenta».**
  Rechazada: enmascara el problema, mete la lógica de IMAP en el cliente y **sigue sin
  distinguir «no hay» de «no pude»**. Un buzón legítimamente vacío provocaría el mismo
  reintento, con coste y latencia extra pero sin ganar corrección.
- **(b) Parchear `async-imap`.** Rechazada para este change: es una dependencia externa,
  el parche quedaría en un fork y hay que mantenerlo. Se registra como propuesta
  *upstream* futura (que `do_poll_next` devuelva `UnexpectedEof` también con el buffer
  vacío si el comando no ha terminado), fuera del alcance de aquí.

## Risks / Trade-offs

- *Riesgo*: la decisión 2 puede no ser implementable sin tocar `async-imap`.
  *Aceptación*: el arreglo principal (decisión 1) ya elimina el caso de producción
  observado; la decisión 2 es refuerzo, no requisito. Si no es viable, se declara.
- *Riesgo*: la **race residual** (decisión 3) deja una ventana de milisegundos.
  *Aceptación*: se documenta explícitamente; el defecto observado no es esa carrera
  (la sesión estaba muerta **horas** antes de la llamada), sino la ausencia de una
  sonda real.
- *Trade-off*: la sonda pasa de un `noop()` barato a `run_command_and_check_ok`.
  Sigue siendo **un** round-trip por reutilización de sesión (no por cada operación
  encadenada), así que el coste es el mismo orden.

## Migration Plan

- Aditivo y localizado: sonda de vida en `src/imap.rs` + (si procede) validación de
  respuesta final en las operaciones de datos, más tests. Sin migración de datos ni
  cambios de contrato para casos legítimos.
- Rollback: revertir la rama `fix/imap-session-liveness`.

## Verification

- **Test de la sonda**: con un servidor de mentira que cierra a medias (FIN sin RST),
  `ensure_session` **descarta** la sesión y reconecta; la sesión zombi no se usa para
  el comando de datos.
- **Test de comando truncado**: `uid_search()` / `select()` / `list()` sobre una
  conexión cerrada por el par devuelven **error** (que la ruta HTTP traduce a `503`),
  **nunca** `total: 0` ni lista vacía.
- **Test de regresión (contrato de éxito)**: un buzón de verdad vacío sigue dando
  `200` con lista vacía; los tests existentes de `imap-connection`, `imap-messages` y
  `imap-mailboxes` siguen en verde.
- `cargo test` en verde; `cargo fmt --check` y `cargo clippy --all-targets -- -D warnings`
  sin avisos.
