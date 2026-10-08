# Design

## Context

`imap-connection` provee la sesión autenticada perezosa: `ConnectionManager` con
`tokio::sync::Mutex<Option<Box<dyn ImapSession>>>`, verificación por `NOOP`, reconexión con
backoff y `Debug` redactado. `imap-mailboxes` añadió `list_mailboxes()`/`select()` al trait
inyectable:

```rust
pub trait ImapSession: Send {
    fn noop(&mut self) -> SendFuture<'_, Result<(), ImapError>>;
    fn list_mailboxes(&mut self) -> SendFuture<'_, Result<Vec<MailboxInfo>, ImapError>>;
    fn select(&mut self, mailbox: String) -> SendFuture<'_, Result<MailboxStatus, ImapError>>;
}
```

`async-imap 0.12` **no** ofrece un constructor tipado de consultas; expone:

```rust
Session::uid_search(query: impl AsRef<str>) -> Result<HashSet<u32>>;      // envía `UID SEARCH <query>`
Session::uid_fetch(uid_set: impl AsRef<str>, query: impl AsRef<str>)
    -> Result<impl Stream<Item = Result<Fetch>>>;                          // envía `UID FETCH <set> <query>`
```

**ambos interpolan el string tal cual**, sin pasar por `validate_str` (que sí usan `list` y
`select`). Como el comando se escribe seguido de `CRLF`, un valor con `CR`/`LF` permitiría
**inyectar comandos IMAP** en la sesión autenticada. Por eso la construcción de
`SEARCH`/`FETCH` se trata aquí como un límite de seguridad.

`Fetch` expone `message` (`Seq`), `uid`, `size`, `flags()`, `header()`, `body()`,
`envelope()` e `internal_date()` (→ `chrono::DateTime<FixedOffset>`, que se formatea con
`to_rfc3339()` sin declarar `chrono`). `imap-proto::Envelope` tiene
`date/subject/message_id: Option<Cow<[u8]>>` y `from/to/cc: Option<Vec<Address>>`; `Address`
tiene `name/adl/mailbox/host: Option<Cow<[u8]>>`; los campos `[u8]` se decodifican con
`String::from_utf8_lossy`. `Uid = u32` y `Seq = u32`.

## Goals / Non-Goals

**Goals:**
- Listar/buscar mensajes con criterios IMAP estándar, paginado y ordenado por UID descendente.
- Descargar metadatos + envelope, cabeceras crudas o el mensaje crudo completo (base64).
- Construir `SEARCH`/`FETCH` sin permitir inyección de comandos.
- Reutilizar la sesión existente y no fijar `\Seen`.
- Errores HTTP claros y estables, sin credenciales ni texto de terceros; todo testeable sin red.

**Non-Goals:**
- Modificar banderas, mover/copiar/borrar (`imap-flags`).
- Parseo MIME a texto/HTML/adjuntos y descarga de secciones MIME (`mime-parsing`).
- `SORT`/`THREAD`, IMAP modified UTF-7, `IDLE`/webhook.

## Decisions

- **`mailbox` en la query, no en el path**: `GET /api/messages` y `GET /api/messages/{uid}`.
  El nombre del buzón puede contener `/`, espacios o UTF-7; meterlo en un segmento de path
  obligaría a un URL-encoding frágil, mientras que como query se decodifica de forma segura.
- **Cada petición selecciona el buzón** reutilizando el `select` existente y ejecuta
  `UID SEARCH` + `UID FETCH` **bajo el mismo lock**, de modo que la operación es
  autocontenida y atómica respecto a otras peticiones. Los cuerpos se piden con
  `BODY.PEEK[...]` para no fijar `\Seen`. Alternativa considerada: `EXAMINE` (selección
  estrictamente de solo lectura); se descarta para no duplicar la superficie de selección
  del trait. Riesgo asumido y documentado: `SELECT` deja el buzón en modo lectura-escritura,
  pero sin `BODY` sin `PEEK` no hay cambio de flags.
- **Composición en `ConnectionManager`** (no en HTTP):
  `list_messages(mailbox, criteria, limit, offset)` = `select` → `search` (UIDs) → ordenar
  descendente → calcular `total` → recortar la ventana → `fetch(summary)` de esa página.
  `fetch_message(mailbox, uid, format)` = `select` → `fetch([uid], format)` →
  `MessageNotFound` si el stream viene vacío. Si la página está vacía **no se lanza**
  `UID FETCH` (un set vacío es un comando inválido).
- **Ampliar `ImapSession`** con:
  ```rust
  fn search(&mut self, criteria: SearchCriteria) -> SendFuture<'_, Result<Vec<u32>, ImapError>>;
  fn fetch(&mut self, uids: Vec<u32>, format: FetchFormat) -> SendFuture<'_, Result<Vec<Message>, ImapError>>;
  ```
  Los fakes existentes se amplían.
- **Construcción segura de comandos** (pura, en `src/imap.rs`, unitaria):
  - `SearchCriteria { from,to,subject,text: Option<String>, since,before: Option<SearchDate>, seen,flagged: Option<bool> }`
    con `imap_key() -> String`: claves `FROM "<v>"`, `TO`, `SUBJECT`, `BODY` (literal
    citado/escapado), `SINCE`/`BEFORE <DD-Mon-YYYY>`, `SEEN`/`UNSEEN`, `FLAGGED`/`UNFLAGGED`;
    sin filtros → `ALL`.
  - `quote_search_string(value)`: **descarta** `CR`/`LF`/`NUL` (defensa en profundidad) y
    escapa `\` y `"`; nunca puede romper la línea del comando.
  - `SearchDate::parse("YYYY-MM-DD")` valida el calendario; `to_imap()` rinde `DD-Mon-YYYY`.
  - `FetchFormat::{Summary,Headers,Full}::query()` devuelve **constantes**: `(UID FLAGS
    RFC822.SIZE INTERNALDATE ENVELOPE)`, la misma + `BODY.PEEK[HEADER]`, y + `BODY.PEEK[]`.
  - El *uid set* se compone solo de dígitos (`uids.join(",")`), nunca con texto libre.
- **Validación en el borde HTTP** (con envelope JSON `400`): `mailbox` no vacío tras `trim`;
  fechas `YYYY-MM-DD`; booleanos `true|false`; `limit` ∈ `1..=200` (def. 50) y `offset ≥ 0`
  (def. 0); `format` ∈ {summary, headers, full}; `unseen` es alias de `seen`
  (`unseen=true` ≡ `seen=false`; si ambos aparecen deben concordar, si no `400`). Se rechazan
  `CR`/`LF`/`NUL` en cualquier valor.
- **Parseo de la query con `RawQuery` + `serde_urlencoded`** para no depender del rejection
  en texto plano de `Query<T>`: todo `400` sale con el envelope JSON.
- **Errores**: `ImapError::MessageNotFound` (nuevo) → `404 message_not_found`;
  `MailboxNotFound` → `404 mailbox_not_found`; el resto → `503 imap_unavailable` con
  `public_message()`.
- **Respuestas**: metadatos + envelope en `summary`; `headers_base64`/`raw_base64` en
  `headers`/`full` (base64 porque el MIME crudo no es UTF-8 y JSON exige UTF-8).

## Risks / Trade-offs

- [Sin servidor IMAP en tests] → sesiones falsas guionizadas para búsqueda/fetch/paginación;
  el formato exacto de las claves `SEARCH`/`FETCH` y del envelope se cubre con tests
  unitarios de las funciones puras y con verificación manual documentada.
- [`async-imap` interpola strings sin validar] → mitigado en el borde (rechazo de
  `CR`/`LF`/`NUL`) y en la construcción (citado/escape). Es el riesgo central del change.
- [`SELECT` deja el buzón en lectura-escritura en la sesión compartida] → sin `BODY` (no
  `PEEK`) no hay cambio de flags; el futuro `imap-flags` volverá a seleccionar.
- [Paginación sobre `HashSet`] → `uid_search` devuelve un `HashSet` sin orden; se ordena
  explícitamente por UID descendente antes de paginar.

## Migration Plan

No hay despliegue en producción. El `README` documentará los dos endpoints nuevos.
