# Design

## Context

`imap-connection` (spec consolidada) ya provee la sesión autenticada perezosa:
`ConnectionManager` con `tokio::sync::Mutex<Option<Box<dyn ImapSession>>>`, verificación
por `NOOP`, reconexión con backoff y `Debug` redactado. Los traits inyectables son:

```rust
pub trait ImapSession: Send { fn noop(&mut self) -> SendFuture<'_, Result<(), ImapError>>; }
pub trait ImapConnector: Send + Sync { fn connect(&self) -> SendFuture<'_, Result<Box<dyn ImapSession>, ImapError>>; }
```

`async-imap 0.12` expone `Session::list(reference, pattern) -> impl Stream<Item = Result<Name>>`,
`Session::select(name) -> Result<Mailbox>` (y `create`/`rename`/`delete`, no usados aquí).
`Name` da `attributes()`, `delimiter()` y `name()`; `Mailbox` da `exists`, `recent`,
`unseen`, `flags`, `permanent_flags`, `uid_next`, `uid_validity`. `imap-proto 0.17` **no**
implementa `Display` para `Flag` ni `NameAttribute`, así que su representación textual
(`\Seen`, `\HasNoChildren`, …) se resuelve con un mapeo explícito y testeable.
`NameAttribute::Extension` llega **con** la barra invertida ya incluida (se parsea con
`tag("\\")`), de modo que el mapeo la normaliza en lugar de volver a anteponerla.

## Goals / Non-Goals

**Goals:**
- Listar buzones (nombre, delimitador, atributos) y seleccionar uno con sus metadatos.
- Reutilizar la sesión y la reconexión ya existentes, sin abrir red nueva.
- Errores HTTP claros y estables, sin filtrar credenciales ni texto de terceros.
- Testear todo sin red.

**Non-Goals:**
- Crear/renombrar/borrar buzones, `STATUS` de conteos, `SUBSCRIBE`/`LSUB`.
- Árbol anidado (lista plana + delimitador), IMAP modified UTF-7.
- `imap-messages`, `imap-flags`, `IDLE`.

## Decisions

- **Ampliar `ImapSession`** con `list_mailboxes()` y `select()` (futuros boxeados), en vez
  de crear una abstracción nueva: mantiene un único punto de inyección y reutiliza los
  fakes. `ImapConnector` no cambia.
- **Refactor de `ConnectionManager`**: extraer la lógica de `status()` (probar la sesión
  con `NOOP`, descartarla si está muerta, reconectar con backoff) a un `ensure_session`
  privado, y añadir `list_mailboxes()` / `select_mailbox(name)` que lo usan. `status()`
  pasa a delegar en `ensure_session`.
- **Sin reintento automático de comandos**: `ensure_session` garantiza sesión viva; el
  comando se ejecuta **una** vez. Si falla con un error de conexión (`Io`,
  `ConnectionLost`, `Tcp`), se **descarta** la sesión cacheada para que la siguiente
  petición reconecte. Reintentar comandos sería peligroso en operaciones futuras no
  idempotentes; se evita de raíz.
- **Tipos de dominio**:
  - `MailboxInfo { name: String, delimiter: Option<String>, attributes: Vec<String> }`.
  - `MailboxStatus { exists: u32, recent: u32, unseen: Option<u32>, uid_validity: Option<u32>, uid_next: Option<u32>, flags: Vec<String> }`.
  - La representación de `Flag`/`NameAttribute` se hace con funciones puras
    (`flag_label`/`attribute_label`) que mapean las variantes a `\Seen`, `\HasNoChildren`,
    … y los *keywords* personalizados tal cual.
- **Detección de «no existe»**: `select` mapea `async_imap::error::Error::No` a
  `ImapError::MailboxNotFound` (variante nueva) → `404 mailbox_not_found`; el resto de
  errores de conexión/protocolo → `503 imap_unavailable` con `public_message()`.
- **Endpoints**:
  - `GET /api/mailboxes` → `{"mailboxes":[{"name":..,"delimiter":..,"attributes":[..]}]}`.
  - `POST /api/mailboxes/select` `{"mailbox":"INBOX"}` → `{"mailbox":"INBOX","exists":..,"recent":..,"unseen":..,"uid_validity":..,"uid_next":..,"flags":[..]}`.
  - Ambos en el sub-router protegido; sin desactivar el body limit (payloads pequeños).
  - `mailbox` se valida no vacío (tras `trim`) → `400 invalid_request`.
- **Redacción**: no se añaden campos nuevos con secretos; el `Debug` sigue redactado.

## Risks / Trade-offs

- [No hay servidor IMAP en los tests] → Toda la superficie se cubre con sesiones falsas
  guionizadas (listado, selección, `NO` → 404, sesión muerta → reconexión). El camino
  real queda como verificación manual documentada.
- [`select` es estado de sesión compartida] → Es el modelo del PLAN («una tarea por
  conexión»); el buzón activo es el último seleccionado. Documentado.
- [`imap-proto` sin `Display`] → El mapeo de flags/atributos es explícito y unitario; si
  aparece una variante nueva, se añade al mapeo.

## Migration Plan

No hay despliegue en producción. El `README` documentará los dos endpoints nuevos.
