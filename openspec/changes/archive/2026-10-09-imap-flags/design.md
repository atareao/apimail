# Design

## Context

`imap-connection` provee la sesión autenticada perezosa (`ConnectionManager` con
`tokio::sync::Mutex<Option<Box<dyn ImapSession>>>`, verificación por `NOOP`, reconexión con
backoff y `Debug` redactado). `imap-mailboxes` añadió `list_mailboxes()`/`select()` y
`imap-messages` amplió el trait con `search()`/`fetch()`. La superficie actual del trait
inyectable es:

```rust
pub trait ImapSession: Send {
    fn noop(&mut self) -> SendFuture<'_, Result<(), ImapError>>;
    fn list_mailboxes(&mut self) -> SendFuture<'_, Result<Vec<MailboxInfo>, ImapError>>;
    fn select(&mut self, mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>>;
    fn search(&mut self, criteria: SearchCriteria) -> SendFuture<'_, Result<Vec<u32>, ImapError>>;
    fn fetch(&mut self, uids: Vec<u32>, format: FetchFormat)
        -> SendFuture<'_, Result<Vec<Message>, ImapError>>;
}
```

`async-imap 0.12` no ofrece constructores tipados para estas operaciones. Los comandos
relevantes interpolan strings **crudos**, sin pasar por el `validate_str` que sí usan
`list`/`select`:

```rust
Session::uid_store(uid_set, query)   -> impl Stream<Item = Result<Fetch>>; // `UID STORE <set> <query>`
Session::uid_copy(uid_set, mailbox)  -> Result<()>;                       // `UID COPY <set> <validate_str(mailbox)>`
Session::uid_mv(uid_set, mailbox)    -> Result<()>;                       // `UID MOVE <set> <validate_str(mailbox)>`
Session::uid_expunge(uid_set)        -> Result<()>;                       // `UID EXPUNGE <set>`
Session::capabilities()              -> Result<Vec<Capability>>;
```

`uid_store` y `uid_expunge` interpolan el *set* y (en el caso de `STORE`) la *query* tal
cual, sin validar. `uid_copy`/`uid_mv` sí validan el buzón (`\n`/`\r` rechazados, cita el
string), pero el *set* sigue siendo crudo. Como el comando se termina con `CRLF`, un valor
con `CR`/`LF` permitiría **inyectar comandos IMAP** en la sesión autenticada. Por eso esta
capa se trata aquí como un límite de seguridad. `Uid = u32`; las capacidades se comprueban
con `Capability::Atom("MOVE".to_string())` y `"UIDPLUS"`.

## Goals / Non-Goals

**Goals:**
- Actualizar banderas (`STORE`), copiar (`COPY`), mover (`MOVE`, con emulación vía
  `UIDPLUS`) y borrar (`UID EXPUNGE`) un único mensaje por `{uid}`.
- Componer `STORE`/`EXPUNGE` sin permitir inyección de comandos: *uid set* numérico y
  *query* de banderas limitada a un allowlist + items fijos.
- Detectar `MOVE`/`UIDPLUS` y elegir la estrategia correcta, degradando con `501` cuando el
  servidor no soporta lo imprescindible.
- Reutilizar la sesión existente.
- Errores HTTP claros y estables, sin credenciales ni texto de terceros; todo testeable sin
  red.

**Non-Goals:**
- `EXPUNGE` global (purgar todos los `\Deleted`): solo se usa `UID EXPUNGE <uid>`.
- Operaciones en lote o multi-uid: cada petición actúa sobre un único `{uid}`.
- Parseo MIME a texto/HTML/adjuntos (`mime-parsing`).
- `IDLE`/webhook (`imap-idle`), `SORT`/`THREAD`, IMAP modified UTF-7.

## Decisions

- **Buzón como query (`?mailbox=INBOX`), mensaje por `{uid}` en el path** — consistente con
  `imap-messages`: el nombre del buzón puede contener `/`, espacios o UTF-7 y como query se
  decodifica de forma segura.
- **Allowlist de banderas (`SystemFlag`)** — enum cerrado con `Seen`, `Answered`, `Flagged`,
  `Draft`, `Deleted`; parseo *case-insensitive* y canonicalización a la forma con `\` y
  capitalización estándar. Cualquier otro valor → `400 invalid_request`. El allowlist es lo
  que impide que texto del usuario llegue a la *query* de `uid_store`.
- **Composición segura del `STORE`** (pura, en `src/imap.rs`, unitaria): la *query* se
  construye **solo** con items fijos (`+FLAGS` para `add`, `-FLAGS` para `remove`) y las
  banderas del allowlist, p. ej. `+FLAGS.SILENT (\Seen \Flagged)`. Se usa `.SILENT` para no
  depender del `FETCH` implícito de la respuesta; tras el `STORE` se ejecuta un `fetch`
  (`FLAGS`) para devolver las banderas resultantes. El *uid set* se rinde con un único
  `u32` (`uid.to_string()`), nunca con texto libre. Validación en el borde HTTP: mismo
  flag no en `add` y `remove`, al menos un campo no vacío, `\r`/`\n`/`\0` rechazados
  (defensa en profundidad).
- **Detección de capacidades y estrategia de `move`** — tras `select`, se consulta
  `capabilities()`:
  - `MOVE` anunciado → `uid_mv(uid, to)`.
  - Sin `MOVE` pero con `UIDPLUS` → emulación: `uid_copy(uid, to)` +
    `uid_store(uid, "+FLAGS.SILENT (\\Deleted)")` + `uid_expunge(uid)`.
  - Ninguna de las dos → `ImapError::CapabilityNotSupported` → `501`.
- **`UID EXPUNGE` y no `EXPUNGE` global** — `expunge()` sin argumentos purga **todos** los
  `\Deleted` del buzón, lo que borraría mensajes ajenos al `uid` solicitado. `uid_expunge`
  purga solo los `\Deleted` del conjunto indicado (RFC 4315). `DELETE` exige `UIDPLUS`: si
  el servidor no lo anuncia → `501` en vez de degradar a un `EXPUNGE` destructivo.
- **`copy` sin capability extra** — `UID COPY` es IMAP4rev1 base; no requiere anuncio.
- **Emulación de `move`: trade-off** — `COPY` + `STORE \Deleted` + `UID EXPUNGE` no es
  atómico (si falla entre pasos puede quedar una copia sin borrar o un mensaje marcado). Se
  asume y documenta porque solo se activa en servidores sin `MOVE`; la alternativa
  (rechazar con `501`) dejaría sin mover a esos servidores. El orden prioriza no perder
  correo: primero copiar, luego marcar, luego expurgar.
- **Ampliar `ImapSession`** con:
  ```rust
  fn store(&mut self, uid: u32, query: FlagQuery) -> SendFuture<'_, Result<(), ImapError>>;
  fn copy(&mut self, uid: u32, mailbox: String) -> SendFuture<'_, Result<(), ImapError>>;
  fn move_message(&mut self, uid: u32, mailbox: String) -> SendFuture<'_, Result<(), ImapError>>;
  fn uid_expunge(&mut self, uid: u32) -> SendFuture<'_, Result<(), ImapError>>;
  fn capabilities(&mut self) -> SendFuture<'_, Result<Capabilities, ImapError>>;
  ```
  Los fakes existentes se amplían. La composición (qué comando y en qué orden) vive en
  `ConnectionManager`, no en HTTP.
- **Composición en `ConnectionManager`** (una sola toma del lock por operación):
  - `update_flags(mailbox, uid, add, remove)` = `select` → `store` → `fetch([uid], Summary)`
    para devolver `flags`.
  - `copy_message(mailbox, uid, to)` = `select` → `copy`.
  - `move_message(mailbox, uid, to)` = `select` → `capabilities` → `move_message` o la
    emulación.
  - `delete_message(mailbox, uid)` = `select` → `capabilities` (exige `UIDPLUS`) → `store
    +FLAGS.SILENT (\Deleted)` → `uid_expunge`.
- **Validación en el borde HTTP** (envelope JSON `400`): `mailbox` no vacío tras `trim` sin
  `\r`/`\n`/`\0`; `uid` entero `> 0`; body parseado y validado (`add`/`remove` conocidos,
  no contradictorios; `to` no vacío, sin `\r`/`\n`/`\0`).
- **Errores**: `ImapError::CapabilityNotSupported` (nuevo) → `501 capability_not_supported`;
  `MailboxNotFound` → `404 mailbox_not_found`; `MessageNotFound` → `404 message_not_found`;
  el resto → `503 imap_unavailable` con `public_message()`.
- **Respuestas**: `PATCH` → `{"mailbox","uid","flags"}`; `move`/`copy` →
  `{"mailbox","uid","to","status"}` (`"moved"`/`"copied"`); `DELETE` →
  `{"mailbox","uid","status":"deleted"}`.
- **Precedencia mensaje-inexistente vs capability**: en `update_flags`, `move` y
  `delete` la existencia del mensaje se comprueba **antes** de consultar y evaluar
  `capabilities()`, y **antes** de cualquier `STORE`. Así un `uid` inexistente
  devuelve siempre `404 message_not_found` de forma consistente (nunca `501` ni un
  `STORE` inútil) y solo se decide la estrategia `MOVE`/emulación/`501` cuando el
  mensaje existe. `copy` ya comprobaba la existencia antes del `UID COPY`.

## Risks / Trade-offs

- [Sin servidor IMAP en tests] → sesiones falsas guionizadas para `store`/`copy`/`move`/
  `uid_expunge`/`capabilities`; la composición exacta de la *query* `STORE` y del *set* se
  cubre con tests unitarios de las funciones puras y con verificación manual documentada.
- [`async-imap` interpola strings sin validar en `STORE`/`EXPUNGE`] → mitigado con el
  allowlist de banderas (la *query* nunca lleva texto del usuario), el *uid set* numérico y
  el rechazo de `CR`/`LF`/`NUL` en el borde. Es el riesgo central del change.
- [Emulación de `move` no atómica] → solo en servidores sin `MOVE`; se prioriza no perder
  correo (copiar antes de borrar) y se documenta la ventana.
- [`SELECT` deja el buzón en lectura-escritura] → es justo lo que habilita `STORE`; se
  ejecuta bajo el mismo lock que el resto de la operación para mantenerla autocontenida.

## Migration Plan

No hay despliegue en producción. El `README` documentará los cuatro endpoints nuevos.
