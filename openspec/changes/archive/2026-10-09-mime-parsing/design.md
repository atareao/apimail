# Design

## Context

`imap-messages` ya sabe traer el mensaje crudo: `ConnectionManager::fetch_message(mailbox,
uid, FetchFormat::Full)` devuelve un `Message` cuyo campo `body: Option<Vec<u8>>` contiene el
RFC822 completo. `imap-flags` añadió las mutaciones. Lo que falta es **interpretar** ese
RFC822. `Config` ya tiene el patrón de límites numéricos (`max_attachment_bytes` para el
envío saliente, `imap_timeout`).

`mail-parser 0.11.9` (edition 2024, `#![forbid(unsafe_code)]`, fuzzed, MIT/Apache-2.0) cumple
RFC 5322 y MIME (2045–2049, 2047, 2231, 2183, 2557…) y expone una representación tipo JMAP
(RFC 8621 §4.1.4):

```rust
MessageParser::default().parse(&bytes)          -> Option<Message>;      // None = no parseable
Message::body_text(0) / body_html(0)            -> Option<Cow<str>>;     // deriva la alternativa que falte
Message::attachments()                          -> impl Iterator<Item = &MessagePart>; // orden posicional
MessagePart::contents() / len()                 -> &[u8] / usize;        // contenido descodificado
MimeHeaders::{content_type, attachment_name, content_id};
ContentType::{c_type, subtype(), is_inline()};
```

## Goals / Non-Goals

**Goals:**
- Exponer el cuerpo parseado (`/body`) y los adjuntos — metadatos (`/attachments`) y
  contenido (`/attachments/{id}`) — de un único mensaje por `{uid}`.
- Encapsular el parseo en un módulo **puro** (`src/mime.rs`), testeable sin red.
- Acotar el parseo con un límite configurable (control de DoS).
- Reutilizar la sesión existente y **no** tocar el trait `ImapSession`.
- Errores HTTP claros y estables, sin credenciales ni texto de terceros.

**Non-Goals:**
- Escribir adjuntos a disco, descarga en streaming o `cid:` inline reescrito.
- Sanear el HTML (se devuelve tal cual; el cliente decide cómo renderizarlo).
- Añadir `format=parsed` a `GET /api/messages/{uid}` (se añade `/body`).
- Construir MIME (`mail-builder`), `IDLE`/webhook, `SORT`/`THREAD`, UTF-7 de buzones.

## Decisions

- **Endpoints nuevos en vez de `format=parsed`** — puramente aditivo: el contrato de
  `imap-messages` (`format ∈ {summary, headers, full}`) queda intacto, así que este change no
  necesita ningún `MODIFIED Requirement`.
- **Módulo puro `src/mime.rs`** — `parse_message(raw: &[u8]) -> Result<ParsedMessage,
  MimeError>`; usa `MessageParser` y **convierte a tipos propios** (`String`/`Vec<u8>`) para
  no arrastrar el préstamo del buffer. Sin I/O, sin red, sin estado global.
- **Orquestación en un único método** — `ConnectionManager::fetch_parsed(mailbox, uid,
  max_bytes)`: `select` → `fetch([uid], Summary)` (existencia + `RFC822.SIZE`) → guarda de
  tamaño → `fetch([uid], Full)`, todo bajo la toma del lock de sesión, que devuelve **solo
  los bytes crudos** (`Vec<u8>`). Ya **fuera** del lock: segunda guarda de tamaño sobre
  `body.len()` y el parseo CPU-bound en `tokio::task::spawn_blocking`, de modo que no bloquea
  un worker del runtime ni mantiene el lock IMAP durante el parseo (tanto un `JoinError` —
  panic del task — como un `MimeError` se mapean a `Unparsable`). Reutiliza el `fetch`
  existente, por lo que **`ImapSession` no cambia**.
- **Modelo de error** — `ParsedMessageError { Imap(ImapError), TooLarge { size: usize,
  limit: usize }, Unparsable }`. En HTTP, `Imap` se mapea con el `From<ImapError>` existente;
  `TooLarge` → `413 message_too_large`; `Unparsable` → `422 message_not_parsable`. Nueva
  variante `MessageError::AttachmentNotFound` → `404 attachment_not_found`.
- **Semántica del cuerpo** — `text`/`html` desde `body_text(0)`/`body_html(0)`. `mail-parser`
  deriva la alternativa que falte (RFC 8621), comportamiento documentado en la spec; un
  mensaje sin parte visible devuelve `null` en ambas.
- **Adjuntos** — `id` = índice posicional en `message.attachments()`; metadatos `filename`
  (`attachment_name()`, ya descodifica RFC 2047/2231), `content_type` (`"{c_type}/{subtype}"`,
  `null` si no hay `Content-Type`), `size` (`len()`, longitud descodificada), `inline`
  (`content_disposition().is_inline()`), `content_id`.
- **Descarga como JSON + base64** — replica el criterio de `imap-messages`
  (`raw_base64`/`headers_base64`): mantiene la API solo-JSON y **elimina la superficie de
  inyección de cabeceras** (no se reflejan `filename`/`content_type` en
  `Content-Disposition`/`Content-Type`). Se descarta la variante «bytes crudos con cabeceras
  saneadas» por consistencia y seguridad.
- **Límite de tamaño** — nuevo `APIMAIL_MAX_MESSAGE_BYTES` (por defecto 25 MiB =
  `26214400`) en `Config`, validado como el resto de variables numéricas (ausente → defecto;
  presente pero no entero positivo → `ConfigError`, *fail-closed*). La pre-comprobación usa
  el `RFC822.SIZE` del `fetch(Summary)`; se añade una segunda comprobación sobre la longitud
  realmente obtenida como defensa en profundidad.
- **Dependencia** — `mail-parser = { version = "0.11", features = ["full_encoding"] }` (añade
  `encoding_rs`) para descodificar charsets multibyte legados (BIG5, ISO-2022-JP…).
- **HTML verbatim** — no se sanea en servidor; se documenta para que el cliente lo trate como
  contenido no confiable.

## Risks / Trade-offs

- [Sin servidor IMAP en tests] → una sesión falsa sirve un RFC822 fijo a `fetch`, y
  `src/mime.rs` se cubre con fixtures embebidos; nada toca la red.
- [MIME no confiable] → `mail-parser` prohíbe `unsafe` y está fuzzeado; el parseo está
  acotado por `APIMAIL_MAX_MESSAGE_BYTES`; no se escribe a disco, ni a cabeceras, ni a logs.
- [Cuerpos derivados] → `mail-parser` puede sintetizar `text` a partir del HTML o al revés;
  se documenta para no sorprender.
- [Memoria] → el mensaje completo y los adjuntos descodificados se tienen en memoria, acotado
  por el límite.
- [Cota de memoria del fetch] → la guarda previa se basa en el `RFC822.SIZE` del
  `fetch(Summary)`; si el servidor lo **omite o lo declara a la baja**, `async-imap`
  materializa el `BODY.PEEK[]` completo **antes** de la segunda comprobación, así que el
  límite acota el **parseo** y no estrictamente la memoria del fetch. La defensa real es la
  segunda comprobación por longitud real sobre `body.len()`, que rechaza el cuerpo antes de
  parsearlo y se ejecuta ya fuera del lock de la sesión.
- [Doble `fetch`] → `Summary` + `Full` son dos round-trips por petición; se acepta a cambio
  de comprobar existencia y tamaño antes de parsear.

## Migration Plan

No hay despliegue en producción. `README.md` documentará los tres endpoints y la variable
`APIMAIL_MAX_MESSAGE_BYTES`.
