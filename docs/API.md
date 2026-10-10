# apimail — Referencia de la API HTTP

Este documento es la **referencia exhaustiva** de la API HTTP de apimail: describe
cada ruta, sus parámetros, los cuerpos de petición y respuesta **campo a campo**,
y la matriz completa de códigos de estado y de códigos `error` estables.

> `README.md` es la **guía de uso** (arranque, variables de entorno, despliegue y
> ejemplos rápidos). Este documento es el **detalle campo a campo** derivado del
> código. Ante cualquier duda, **la fuente de verdad es el código** (`src/http.rs`,
> `src/config.rs`, `src/imap.rs`, `src/mime.rs`, `src/smtp.rs`, `src/idle.rs`,
> `src/queue.rs`).

---

## Convenciones generales

### URL base y formato

- La URL base es `http://<APIMAIL_HOST>:<APIMAIL_PORT>` (por defecto
  `http://0.0.0.0:3000`); todas las rutas cuelgan de `/api`.
- Las respuestas con cuerpo son `Content-Type: application/json`.
- Los identificadores, rutas, campos JSON y códigos `error` se escriben aquí en
  su forma original del código (inglés, `snake_case`).

### Autenticación

- Todas las rutas están protegidas **salvo** `GET /api/health`, que es **pública**.
- La petición debe llevar la API key en la cabecera `Authorization` con el
  esquema `Bearer`:

  ```http
  Authorization: Bearer <APIMAIL_API_KEY>
  ```

- El esquema `Bearer` **no distingue mayúsculas/minúsculas** (`bearer`, `BeArEr`…);
  entre el esquema y el token se acepta una o más espacios o tabuladores.
- La comparación se hace en **tiempo constante** sobre el digest SHA-256 (tamaño
  fijo) de la clave; nunca se compara ni se registra la clave en claro.
- Si la cabecera falta, usa otro esquema o la clave es incorrecta → `401` con
  `WWW-Authenticate: Bearer` y el envoltorio `{"error":"unauthorized","message":...}`.
- El arranque es **fail-closed**: sin `APIMAIL_API_KEY` no vacía el proceso no
  arranca.

### Envoltorio de error

Salvo las respuestas propias de axum para ruta desconocida (`404`) y método no
permitido (`405`), todos los errores usan el envoltorio JSON:

```json
{ "error": "<código estable>", "message": "<explicación legible>" }
```

`error` es un **código estable y máquina-filtrable**; `message` es un texto
legible que, en las rutas de correo, **nunca** refleja credenciales ni texto de
terceros (banners SMTP, trazas de rustls, direcciones…).

Códigos `error` estables existentes:

| Código `error` | HTTP | Origen |
| --- | --- | --- |
| `unauthorized` | 401 | middleware de autenticación |
| `invalid_request` | 400 | validación en el borde HTTP |
| `mailbox_not_found` | 404 | `SELECT` responde `NO` |
| `message_not_found` | 404 | el `uid` no existe en el buzón |
| `attachment_not_found` | 404 | el `id` no corresponde a ningún adjunto |
| `message_too_large` | 413 | mensaje por encima de `APIMAIL_MAX_MESSAGE_BYTES` (lectura/parseo) |
| `message_not_parsable` | 422 | el MIME del mensaje no puede parsearse |
| `payload_too_large` | 413 | cuerpo crudo o adjuntos decodificados por encima del límite (envío) |
| `smtp_error` | 502 | el servidor SMTP rechaza o falla la entrega |
| `capability_not_supported` | 501 | el servidor no anuncia `MOVE`/`UIDPLUS` |
| `idle_not_configured` | 501 | `start` sin `APIMAIL_WEBHOOK_URL` |
| `imap_unavailable` | 503 | la sesión o el servidor IMAP no están disponibles |

### Tabla global de códigos de estado

| HTTP | Código `error` | En qué rutas ocurre |
| --- | --- | --- |
| `200` | — (éxito) | todas las rutas |
| `400` | `invalid_request` | `POST /api/mailboxes/select`, `GET /api/messages`, `GET /api/messages/{uid}`, `PATCH /api/messages/{uid}/flags`, `POST /api/messages/{uid}/move`, `POST /api/messages/{uid}/copy`, `GET /api/messages/{uid}/body`, `GET /api/messages/{uid}/attachments`, `GET /api/messages/{uid}/attachments/{id}`, `POST /api/messages` |
| `401` | `unauthorized` | todas las rutas protegidas |
| `404` | `mailbox_not_found` | `POST /api/mailboxes/select`, `GET /api/messages`, `GET /api/messages/{uid}`, `PATCH .../flags`, `POST .../move`, `POST .../copy`, `GET .../body`, `GET .../attachments`, `GET .../attachments/{id}` |
| `404` | `message_not_found` | `GET /api/messages/{uid}`, `PATCH .../flags`, `POST .../move`, `POST .../copy`, `GET .../body`, `GET .../attachments`, `GET .../attachments/{id}`, `DELETE /api/messages/{uid}` |
| `404` | `attachment_not_found` | `GET /api/messages/{uid}/attachments/{id}` |
| `404` | — (sin envoltorio) | ruta desconocida (respuesta por defecto de axum) |
| `405` | — (sin envoltorio) | método no soportado en una ruta existente (axum) |
| `413` | `message_too_large` | `GET .../body`, `GET .../attachments`, `GET .../attachments/{id}` |
| `413` | `payload_too_large` | `POST /api/messages` |
| `422` | `message_not_parsable` | `GET .../body`, `GET .../attachments`, `GET .../attachments/{id}` |
| `501` | `capability_not_supported` | `POST .../move`, `DELETE /api/messages/{uid}` |
| `501` | `idle_not_configured` | `POST /api/idle/start` |
| `502` | `smtp_error` | `POST /api/messages` |
| `503` | `imap_unavailable` | `GET /api/imap/status`, `GET /api/mailboxes`, `POST /api/mailboxes/select`, `GET /api/messages`, `GET /api/messages/{uid}`, `PATCH .../flags`, `POST .../move`, `POST .../copy`, `GET .../body`, `GET .../attachments`, `GET .../attachments/{id}`, `DELETE /api/messages/{uid}` |

### Rutas y métodos no soportados

- Una **ruta desconocida** (p. ej. `GET /api/unknown`) → `404`, con la respuesta
  por defecto de axum. **No** usa el envoltorio `{"error":...,"message":...}`.
- Un **método no permitido** sobre una ruta existente (p. ej. `POST /api/health`)
  → `405`, respuesta por defecto de axum. **No** usa el envoltorio JSON.

> El middleware de autenticación se aplica **solo** sobre el sub-router protegido,
> por lo que `404`/`405` de rutas desconocidas o métodos no permitidos no exigen
> (ni comprueban) la API key.

### Límites y configuración

Valores por defecto reales tomados de `src/config.rs`.

| Variable | Descripción | Valor por defecto | Restricción |
| --- | --- | --- | --- |
| `APIMAIL_HOST` | Dirección de escucha | `0.0.0.0` | — |
| `APIMAIL_PORT` | Puerto de escucha (`u16`) | `3000` | debe ser un `u16` válido |
| `APIMAIL_API_KEY` | API key exigida | — (**obligatoria**) | no vacía (fail-closed) |
| `APIMAIL_IMAP_HOST` / `_USER` / `_PASSWORD` | Cuenta IMAP | — (**obligatorias**) | no vacías |
| `APIMAIL_IMAP_PORT` | Puerto IMAP | derivado del TLS (`993`/`143`) | `1..=65535`; en blanco = ausente |
| `APIMAIL_IMAP_TLS` | Modo TLS IMAP | `implicit` | `implicit`/`starttls`/`none` (sin distinguir mayúsculas) |
| `APIMAIL_SMTP_HOST` / `_USER` / `_PASSWORD` | Cuenta SMTP | — (**obligatorias**) | no vacías |
| `APIMAIL_SMTP_PORT` | Puerto SMTP | derivado del TLS (`465`/`587`) | `1..=65535`; en blanco = ausente |
| `APIMAIL_SMTP_TLS` | Modo TLS SMTP | `implicit` | `implicit`/`starttls`/`none` |
| `APIMAIL_MAX_ATTACHMENT_BYTES` | Máx. total de adjuntos **decodificados** (envío) | `10485760` (10 MiB) | entero `> 0` |
| `APIMAIL_MAX_MESSAGE_BYTES` | Máx. de mensaje a descargar/parsear (lectura) | `26214400` (25 MiB) | entero `> 0` |
| `APIMAIL_IMAP_TIMEOUT_SECS` | Timeout (TCP + TLS + login IMAP) | `30` | entero `> 0` |
| `APIMAIL_WEBHOOK_URL` | URL notificada al llegar correo | — (sin ella, IDLE no arranca) | URL absoluta `http`/`https` |
| `APIMAIL_IDLE_MAILBOX` | Buzón vigilado por IDLE | `INBOX` | en blanco → vuelve a `INBOX` |
| `APIMAIL_WEBHOOK_TIMEOUT_SECS` | Timeout por petición al webhook | `10` | entero `> 0` |
| `APIMAIL_QUEUE_PATH` | Fichero de la cola duradera | — (cola en memoria) | ruta usable; en blanco = ausente |
| `APIMAIL_QUEUE_MAX_ITEMS` | Máx. de notificaciones pendientes | `1000` | entero `> 0` |
| `APIMAIL_QUEUE_MAX_BYTES` | Máx. de bytes vivos en la cola | `67108864` (64 MiB) | entero `> 0` |

Notas de límites relevantes para la API:

- **Cuerpo de `POST /api/messages`**: el límite del cuerpo crudo es
  `APIMAIL_MAX_ATTACHMENT_BYTES * 4 / 3 + 65536` (expansión base64 + holgura
  `BODY_LIMIT_SLACK = 64 KiB`); la comprobación autoritativa sigue siendo el total
  **decodificado** de los adjuntos contra `APIMAIL_MAX_ATTACHMENT_BYTES`.
- **Lectura/parseo**: `APIMAIL_MAX_MESSAGE_BYTES` acota el mensaje en dos puntos:
  el `RFC822.SIZE` reportado por el servidor y, de forma autoritativa, la longitud
  realmente descargada antes de parsear.
- Un `0` o un valor no numérico en los enteros anteriores provoca un **error de
  arranque** (sin *fallback* silencioso).

### Notas transversales de seguridad

- **Sesión IMAP perezosa y compartida**: nada se abre en el arranque; la sesión se
  establece en el primer uso, se reutiliza mientras está viva y se reemplaza con
  *backoff* exponencial acotado cuando se detecta muerta (sonda `NOOP`). Las rutas
  compartidas usan esa única sesión.
- **`BODY.PEEK`**: los cuerpos se piden con `BODY.PEEK[...]`; **leer un mensaje
  nunca fija la bandera `\Seen`**.
- **Nada se escribe a disco** durante las operaciones de correo: el MIME se parsea
  en memoria. Lo único que puede escribir a disco es la cola durable, y solo si se
  define `APIMAIL_QUEUE_PATH`.
- **Las respuestas nunca exponen usuario ni secreto**: `GET /api/account` y
  `GET /api/imap/status` devuelven solo host/puerto/modo TLS.
- **Contenido no UTF-8 en base64**: las cabeceras crudas, el mensaje completo y el
  contenido de los adjuntos viajan en **base64** dentro del JSON (un JSON válido
  debe ser UTF-8); así un `filename`/`content_type` hostil nunca llega a una
  cabecera HTTP.
- **Validación en el borde HTTP**: los textos de búsqueda se citan y escapan, el
  `uid set` se rinde a partir de un `u32` y la *query* de `STORE` se compone solo
  con la allowlist; se rechazan `CR`/`LF`/`NUL` en los valores, de modo que ninguna
  petición puede inyectar comandos IMAP.
- **Códigos `error` estables**: los mensajes de error de correo son cadenas fijas
  o agrupadas por familia, nunca texto de terceros.

---

## Índice de rutas

| Método | Ruta | ¿Protegida? | Propósito |
| --- | --- | --- | --- |
| `GET` | `/api/health` | No (pública) | Sonda de salud del servicio |
| `GET` | `/api/whoami` | Sí | Confirma que la autenticación tuvo éxito |
| `GET` | `/api/account` | Sí | Host/puerto (no sensibles) de IMAP y SMTP |
| `GET` | `/api/imap/status` | Sí | Estado de la conexión IMAP |
| `GET` | `/api/mailboxes` | Sí | Lista de buzones (`LIST`) |
| `POST` | `/api/mailboxes/select` | Sí | Selecciona un buzón (`SELECT`) |
| `GET` | `/api/messages` | Sí | Lista y busca mensajes (paginado) |
| `POST` | `/api/messages` | Sí | Envía correo saliente (SMTP) |
| `GET` | `/api/messages/{uid}` | Sí | Descarga metadatos/headers/full de un mensaje |
| `DELETE` | `/api/messages/{uid}` | Sí | Marca `\Deleted` y expurga solo ese mensaje |
| `PATCH` | `/api/messages/{uid}/flags` | Sí | Añade y/o quita banderas (`UID STORE`) |
| `POST` | `/api/messages/{uid}/move` | Sí | Mueve el mensaje a otro buzón |
| `POST` | `/api/messages/{uid}/copy` | Sí | Copia el mensaje a otro buzón |
| `GET` | `/api/messages/{uid}/body` | Sí | Cuerpo en texto plano y HTML |
| `GET` | `/api/messages/{uid}/attachments` | Sí | Lista los adjuntos (sin contenido) |
| `GET` | `/api/messages/{uid}/attachments/{id}` | Sí | Contenido de un adjunto (base64) |
| `POST` | `/api/idle/start` | Sí | Arranca la suscripción IDLE |
| `POST` | `/api/idle/stop` | Sí | Detiene la suscripción IDLE |
| `GET` | `/api/idle/status` | Sí | Estado de la suscripción IDLE |
| `GET` | `/api/idle/queue` | Sí | Estado observable de la cola de entrega |

---

## Endpoints

### `GET /api/health`

- **Autenticación**: pública (no requiere API key).
- **Descripción**: sonda de salud. No toca IMAP ni SMTP, no tiene *side-effects*.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `status` | string | siempre | siempre `"ok"` |
| `name` | string | siempre | nombre del crate (`"apimail"`, `CARGO_PKG_NAME`) |
| `version` | string | siempre | versión del crate en ejecución (`CARGO_PKG_VERSION`) |

```json
{ "status": "ok", "name": "apimail", "version": "0.4.2" }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | siempre que la petición es `GET /api/health` |
| `404` | — | ruta distinta (axum) |
| `405` | — | método distinto de `GET` (axum) |

**Ejemplo `curl`**

```bash
curl -fsS http://127.0.0.1:3000/api/health
```

---

### `GET /api/whoami`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: ruta de andamiaje; llega al handler solo tras autenticación
  correcta. Sin *side-effects*.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `authenticated` | bool | siempre | siempre `true` |

```json
{ "authenticated": true }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | clave válida |
| `401` | `unauthorized` | sin cabecera, esquema distinto de `Bearer`, o clave incorrecta (incluye token vacío o cabecera no ASCII) |
| `405` | — | método distinto de `GET` (axum) |

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/whoami
```

---

### `GET /api/account`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: devuelve **solo el host y el puerto** de los endpoints IMAP y
  SMTP (nunca usuario ni secreto). Sin *side-effects*.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `imap` | object | siempre | endpoint de entrada |
| `imap.host` | string | siempre | host IMAP |
| `imap.port` | number | siempre | puerto IMAP (`u16`) |
| `smtp` | object | siempre | endpoint de salida |
| `smtp.host` | string | siempre | host SMTP |
| `smtp.port` | number | siempre | puerto SMTP (`u16`) |

```json
{
  "imap": { "host": "imap.example.com", "port": 993 },
  "smtp": { "host": "smtp.example.com", "port": 465 }
}
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | clave válida |
| `401` | `unauthorized` | clave ausente o inválida |
| `405` | — | método distinto de `GET` (axum) |

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/account
```

---

### `GET /api/imap/status`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: asegura una sesión IMAP viva (conectando de forma perezosa si
  no la hay) y reporta el resultado. **Side-effect**: puede abrir/reutilizar la
  conexión IMAP compartida.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `connected` | bool | siempre | `true` cuando hay sesión viva |
| `host` | string | siempre | host IMAP configurado |
| `port` | number | siempre | puerto IMAP (`u16`) |
| `tls` | string | siempre | modo TLS: `implicit`, `starttls` o `none` |

```json
{ "connected": true, "host": "imap.example.com", "port": 993, "tls": "implicit" }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | sesión asegurada |
| `401` | `unauthorized` | clave ausente o inválida |
| `503` | `imap_unavailable` | no se pudo (re)conectar |

**Cuerpo de la respuesta 503** (envoltorio extendido con `connected`):

```json
{
  "connected": false,
  "error": "imap_unavailable",
  "message": "could not connect to the IMAP server"
}
```

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/imap/status
```

---

### `GET /api/mailboxes`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: lista los buzones de la cuenta con `LIST "" "*"`. Reutiliza la
  sesión IMAP compartida. No expone credenciales.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailboxes` | array | siempre | lista de buzones |
| `mailboxes[].name` | string | siempre | nombre del buzón |
| `mailboxes[].delimiter` | string \| null | siempre | delimitador de jerarquía; `null` si el servidor reporta espacio plano |
| `mailboxes[].attributes` | array de string | siempre | atributos IMAP (`\NoSelect`, `\HasNoChildren`, `\Sent`…) |

```json
{
  "mailboxes": [
    { "name": "INBOX", "delimiter": "/", "attributes": ["\\HasNoChildren"] },
    { "name": "Sent", "delimiter": "/", "attributes": ["\\Sent"] }
  ]
}
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | listado correcto |
| `401` | `unauthorized` | clave ausente o inválida |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

> `LIST` no produce `NO` de buzón inexistente, por lo que `404 mailbox_not_found`
> no ocurre en la práctica en esta ruta (el mapeo existe, pero `LIST` no lo emite).

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/mailboxes
```

---

### `POST /api/mailboxes/select`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: selecciona el buzón indicado (`SELECT`) y lo deja como buzón
  activo para las operaciones siguientes. Reutiliza la sesión IMAP compartida.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: `application/json`:

| Campo | Tipo | Obligatorio | Restricciones |
| --- | --- | --- | --- |
| `mailbox` | string | Sí | no vacío tras `trim`; sin `CR`/`LF`/`NUL` |

```json
{ "mailbox": "INBOX" }
```

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón seleccionado |
| `exists` | number | siempre | nº de mensajes (`EXISTS`) |
| `recent` | number | siempre | mensajes con `\Recent` (`RECENT`) |
| `unseen` | number \| null | siempre | primer mensaje no visto (`UNSEEN`); `null` si el servidor lo omite |
| `uid_validity` | number \| null | siempre | `UIDVALIDITY`; `null` si se omite |
| `uid_next` | number \| null | siempre | `UIDNEXT`; `null` si se omite |
| `flags` | array de string | siempre | banderas definidas en IMAP (`\Seen`, …) |

```json
{
  "mailbox": "INBOX",
  "exists": 42,
  "recent": 0,
  "unseen": 3,
  "uid_validity": 7,
  "uid_next": 100,
  "flags": ["\\Seen", "\\Flagged"]
}
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | buzón seleccionado |
| `400` | `invalid_request` | cuerpo JSON malformado o `mailbox` ausente/blank |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` (buzón inexistente) |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
curl -fsS -X POST -H "Authorization: Bearer una-clave-secreta" \
  -H "Content-Type: application/json" \
  -d '{"mailbox":"INBOX"}' \
  http://127.0.0.1:3000/api/mailboxes/select
```

---

### `GET /api/messages`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: selecciona `mailbox`, ejecuta `UID SEARCH` con los filtros y
  devuelve una **página** ordenada por `UID` **descendente** (los más recientes
  primero). `total` cuenta **todas** las coincidencias; `messages` es solo la
  ventana pedida. Los cuerpos se piden con `BODY.PEEK` (no fija `\Seen`).
  **Side-effect**: `SELECT` del buzón.

**Parámetros (query)**

| Nombre | Tipo | Obligatorio | Restricciones / límites | Por defecto |
| --- | --- | --- | --- | --- |
| `mailbox` | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |
| `from` | string | No | filtro `FROM`; sin caracteres de control | — |
| `to` | string | No | filtro `TO`; sin caracteres de control | — |
| `subject` | string | No | filtro `SUBJECT`; sin caracteres de control | — |
| `text` | string | No | filtro `BODY`; sin caracteres de control | — |
| `since` | string | No | `SINCE`, fecha `YYYY-MM-DD` válida | — |
| `before` | string | No | `BEFORE`, fecha `YYYY-MM-DD` válida | — |
| `seen` | `true`/`false` | No | `true`→`SEEN`, `false`→`UNSEEN` | — |
| `unseen` | `true`/`false` | No | alias de `seen` (`unseen=true` ⇒ `seen=false`); si ambos se dan, deben concordar | — |
| `flagged` | `true`/`false` | No | `true`→`FLAGGED`, `false`→`UNFLAGGED` | — |
| `limit` | integer | No | `1..=200` | `50` |
| `offset` | integer | No | `>= 0` | `0` |

**Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón consultado |
| `total` | number | siempre | total de coincidencias de la búsqueda |
| `limit` | number | siempre | tamaño de página aplicado |
| `offset` | number | siempre | desplazamiento aplicado |
| `messages` | array | siempre | ventana de mensajes |
| `messages[].uid` | number | siempre | UID del mensaje |
| `messages[].seq` | number | siempre | número de secuencia |
| `messages[].flags` | array de string | siempre | banderas IMAP |
| `messages[].size` | number \| null | siempre | `RFC822.SIZE`; `null` si se omite |
| `messages[].internal_date` | string \| null | siempre | `INTERNALDATE` en RFC 3339; `null` si se omite |
| `messages[].envelope` | object \| null | siempre | envelope; `null` si el servidor no lo reporta |
| `messages[].envelope.from` | array | si hay envelope | `{name, address}` |
| `messages[].envelope.to` | array | si hay envelope | `{name, address}` |
| `messages[].envelope.cc` | array | si hay envelope | `{name, address}` |
| `messages[].envelope.subject` | string \| null | si hay envelope | cabecera `Subject` cruda |
| `messages[].envelope.date` | string \| null | si hay envelope | cabecera `Date` cruda |
| `messages[].envelope.message_id` | string \| null | si hay envelope | `Message-ID` |

```json
{
  "mailbox": "INBOX",
  "total": 123,
  "limit": 50,
  "offset": 0,
  "messages": [
    {
      "uid": 42,
      "seq": 42,
      "flags": ["\\Seen"],
      "size": 2048,
      "internal_date": "2026-10-08T12:00:00+00:00",
      "envelope": {
        "from": [{ "name": "Alice", "address": "alice@example.com" }],
        "to": [{ "name": null, "address": "bob@example.com" }],
        "cc": [],
        "subject": "hi",
        "date": "Wed, 08 Oct 2026 11:00:00 +0000",
        "message_id": "<id@example.com>"
      }
    }
  ]
}
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | búsqueda correcta |
| `400` | `invalid_request` | `mailbox` ausente/blank; fecha que no es `YYYY-MM-DD`; booleano distinto de `true`/`false`; `limit` fuera de `1..=200`; `offset` no numérico; `seen`/`unseen` contradictorios; caracteres de control; *query string* malformada |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages?mailbox=INBOX&subject=Hola&limit=20&offset=0"
```

---

### `POST /api/messages`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: envía un correo por el SMTP configurado. Los adjuntos en base64
  se decodifican **aquí**, en el borde HTTP. **Side-effect**: entrega SMTP.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: `application/json`:

| Campo | Tipo | Obligatorio | Restricciones / notas |
| --- | --- | --- | --- |
| `from` | string | No | si se omite, se usa el usuario SMTP (`APIMAIL_SMTP_USER`); debe ser una dirección válida |
| `to` | array de string | Sí | al menos una dirección; cada una debe ser una dirección válida |
| `cc` | array de string | No | direcciones válidas (por defecto `[]`) |
| `bcc` | array de string | No | direcciones válidas (por defecto `[]`) |
| `subject` | string | No | asunto (por defecto `""`) |
| `text` | string | No | cuerpo en texto plano |
| `html` | string | No | cuerpo en HTML |
| `attachments` | array | No | adjuntos (por defecto `[]`) |
| `attachments[].filename` | string | Sí (por elemento) | nombre presentado al destinatario |
| `attachments[].content_type` | string | No | MIME; por defecto `application/octet-stream` |
| `attachments[].data_base64` | string | Sí (por elemento) | contenido en base64 |

```json
{
  "from": "Yo <me@example.com>",
  "to": ["dest@example.com"],
  "cc": [],
  "bcc": [],
  "subject": "Hola",
  "text": "cuerpo en texto",
  "html": "<p>cuerpo en HTML</p>",
  "attachments": [
    { "filename": "nota.txt", "content_type": "text/plain", "data_base64": "aGVsbGE=" }
  ]
}
```

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `status` | string | siempre | siempre `"sent"` |

```json
{ "status": "sent" }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | el transporte aceptó el mensaje |
| `400` | `invalid_request` | JSON malformado; sin destinatarios; dirección inválida; base64 inválido |
| `401` | `unauthorized` | clave ausente o inválida |
| `413` | `payload_too_large` | el cuerpo crudo supera el límite, o los adjuntos **decodificados** superan `APIMAIL_MAX_ATTACHMENT_BYTES` |
| `502` | `smtp_error` | el servidor SMTP rechaza o falla la entrega |

> El límite de cuerpo se aplica en el propio handler (con `DefaultBodyLimit`
> desactivado para esta ruta), por lo que un cuerpo excesivo también recibe el
> envoltorio JSON (`413 payload_too_large`) en lugar del rechazo en texto plano de
> axum. Los `message` de `payload_too_large` pueden incluir los totales implicados
> (números), no secretos.

**Ejemplo `curl`**

```bash
curl -fsS -X POST -H "Authorization: Bearer una-clave-secreta" \
  -H "Content-Type: application/json" \
  -d '{"to":["dest@example.com"],"subject":"Hola","text":"cuerpo"}' \
  http://127.0.0.1:3000/api/messages
```

**Seguridad**: los adjuntos solo viajan dentro del cuerpo JSON; su tamaño se mide
ya decodificado contra el límite configurado.

---

### `GET /api/messages/{uid}`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: selecciona `mailbox` y descarga el mensaje `uid` en el formato
  pedido, con `BODY.PEEK` (no fija `\Seen`). **Side-effect**: `SELECT`.

**Parámetros**

| Nombre | Ubicación | Tipo | Obligatorio | Restricciones | Por defecto |
| --- | --- | --- | --- | --- | --- |
| `uid` | path | integer | Sí | entero positivo (`> 0`) | — |
| `mailbox` | query | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |
| `format` | query | string | No | `summary`/`headers`/`full` | `summary` |

**Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón consultado |
| `uid` | number | siempre | UID del mensaje |
| `seq` | number | siempre | número de secuencia |
| `flags` | array de string | siempre | banderas IMAP |
| `size` | number \| null | siempre | `RFC822.SIZE` |
| `internal_date` | string \| null | siempre | `INTERNALDATE` (RFC 3339) |
| `envelope` | object \| null | siempre | envelope (misma forma que en el listado) |
| `format` | string | siempre | `summary`, `headers` o `full` |
| `headers_base64` | string | **solo** `format=headers` | bloque crudo de cabeceras en base64 |
| `raw_base64` | string | **solo** `format=full` | mensaje RFC822 completo en base64 |

```json
{
  "mailbox": "INBOX",
  "uid": 42,
  "seq": 42,
  "flags": ["\\Seen"],
  "size": 2048,
  "internal_date": "2026-10-08T12:00:00+00:00",
  "envelope": {
    "from": [{ "name": "Alice", "address": "alice@example.com" }],
    "to": [{ "name": null, "address": "bob@example.com" }],
    "cc": [],
    "subject": "hi",
    "date": "Wed, 08 Oct 2026 11:00:00 +0000",
    "message_id": "<id@example.com>"
  },
  "format": "summary"
}
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | mensaje descargado |
| `400` | `invalid_request` | `uid` no numérico o `0`; `mailbox` ausente/blank o con caracteres de control; `format` distinto de `summary`/`headers`/`full`; *query string* malformada |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `404` | `message_not_found` | el `uid` no existe en el buzón |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
# Metadatos (por defecto)
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages/42?mailbox=INBOX"

# Mensaje completo en base64
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages/42?mailbox=INBOX&format=full"
```

**Seguridad**: el contenido MIME crudo no es necesariamente UTF-8; por eso
`headers_base64`/`raw_base64` viajan en base64.

---

### `DELETE /api/messages/{uid}`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: marca el mensaje como `\Deleted` con `UID STORE` y lo purga
  **solo a él** con `UID EXPUNGE`. Exige que el servidor anuncie `UIDPLUS`; si no
  lo hace, responde `501 capability_not_supported` y **nunca** ejecuta un
  `EXPUNGE` global. La existencia del mensaje se comprueba **antes** que la
  capacidad. **No** es idempotente (borra).
- **Parámetros**

| Nombre | Ubicación | Tipo | Obligatorio | Restricciones | Por defecto |
| --- | --- | --- | --- | --- | --- |
| `uid` | path | integer | Sí | entero positivo (`> 0`) | — |
| `mailbox` | query | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |

**Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón |
| `uid` | number | siempre | UID borrado |
| `status` | string | siempre | siempre `"deleted"` |

```json
{ "mailbox": "INBOX", "uid": 42, "status": "deleted" }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | marcado y expurgado |
| `400` | `invalid_request` | `uid` no numérico o `0`; `mailbox` ausente/blank o con caracteres de control |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `404` | `message_not_found` | el `uid` no existe (comprobado **antes** que la capacidad) |
| `501` | `capability_not_supported` | el servidor no anuncia `UIDPLUS` |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
curl -fsS -X DELETE -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages/42?mailbox=INBOX"
```

---

### `PATCH /api/messages/{uid}/flags`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: añade y/o quita banderas con `UID STORE`; las banderas
  resultantes se leen con un `UID FETCH`. La existencia del mensaje se comprueba
  antes de enviar el `STORE`. Idempotente respecto al estado final de las banderas.
- **Parámetros**

| Nombre | Ubicación | Tipo | Obligatorio | Restricciones | Por defecto |
| --- | --- | --- | --- | --- | --- |
| `uid` | path | integer | Sí | entero positivo (`> 0`) | — |
| `mailbox` | query | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |

**Cuerpo de la petición**: `application/json`:

| Campo | Tipo | Obligatorio | Restricciones |
| --- | --- | --- | --- |
| `add` | array de string | No (por defecto `[]`) | cada bandera debe estar en la allowlist |
| `remove` | array de string | No (por defecto `[]`) | cada bandera debe estar en la allowlist |

- Al menos uno de `add`/`remove` debe ser no vacío.
- Ninguna bandera puede aparecer a la vez en `add` y `remove`.
- Allowlist (se acepta sin distinguir mayúsculas en el nombre, pero el `\` inicial
  es **obligatorio**): `\Seen`, `\Answered`, `\Flagged`, `\Draft`, `\Deleted`.
- Las banderas se canonicalizan y deduplican.

```json
{ "add": ["\\Seen", "\\Flagged"], "remove": ["\\Deleted"] }
```

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón |
| `uid` | number | siempre | UID afectado |
| `flags` | array de string | siempre | banderas resultantes tras el `STORE` |

```json
{ "mailbox": "INBOX", "uid": 42, "flags": ["\\Seen", "\\Flagged"] }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | banderas aplicadas |
| `400` | `invalid_request` | `uid` no numérico o `0`; `mailbox` ausente/blank o con caracteres de control; JSON malformado; actualización vacía; bandera desconocida; misma bandera en `add` y `remove` |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `404` | `message_not_found` | el `uid` no existe |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
curl -fsS -X PATCH -H "Authorization: Bearer una-clave-secreta" \
  -H "Content-Type: application/json" \
  -d '{"add":["\\Seen","\\Flagged"]}' \
  "http://127.0.0.1:3000/api/messages/42/flags?mailbox=INBOX"
```

**Seguridad**: la *query* del `STORE` se compone **solo** con la allowlist y los
items fijos (`+FLAGS.SILENT`/`-FLAGS.SILENT`); el `uid set` se rinde desde un
`u32`, así que no hay inyección de comandos IMAP.

---

### `POST /api/messages/{uid}/move`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: mueve el mensaje `uid` al buzón destino. Usa `UID MOVE` cuando
  el servidor anuncia `MOVE`; si no, cuando anuncia `UIDPLUS`, emula el movimiento
  con `UID COPY` + `UID STORE +FLAGS.SILENT (\Deleted)` + `UID EXPUNGE` (primero
  copia, después marca y por último expurga, para no perder correo). La existencia
  del mensaje se comprueba **antes** que la capacidad. **No** es idempotente.
- **Parámetros**

| Nombre | Ubicación | Tipo | Obligatorio | Restricciones | Por defecto |
| --- | --- | --- | --- | --- | --- |
| `uid` | path | integer | Sí | entero positivo (`> 0`) | — |
| `mailbox` | query | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |

**Cuerpo de la petición**: `application/json`:

| Campo | Tipo | Obligatorio | Restricciones |
| --- | --- | --- | --- |
| `to` | string | Sí | buzón destino no vacío tras `trim`; sin `CR`/`LF`/`NUL` |

```json
{ "to": "Archive" }
```

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón origen |
| `uid` | number | siempre | UID movido |
| `to` | string | siempre | buzón destino |
| `status` | string | siempre | siempre `"moved"` |

```json
{ "mailbox": "INBOX", "uid": 42, "to": "Archive", "status": "moved" }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | movimiento realizado |
| `400` | `invalid_request` | `uid` no numérico o `0`; `mailbox` o `to` ausentes/blank o con caracteres de control; JSON malformado |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `404` | `message_not_found` | el `uid` no existe |
| `501` | `capability_not_supported` | el servidor no anuncia ni `MOVE` ni `UIDPLUS` |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
curl -fsS -X POST -H "Authorization: Bearer una-clave-secreta" \
  -H "Content-Type: application/json" \
  -d '{"to":"Archive"}' \
  "http://127.0.0.1:3000/api/messages/42/move?mailbox=INBOX"
```

---

### `POST /api/messages/{uid}/copy`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: copia el mensaje `uid` al buzón destino con `UID COPY`
  (parte de IMAP4rev1; **no requiere extensión**). La existencia del mensaje se
  comprueba antes de la copia.
- **Parámetros**

| Nombre | Ubicación | Tipo | Obligatorio | Restricciones | Por defecto |
| --- | --- | --- | --- | --- | --- |
| `uid` | path | integer | Sí | entero positivo (`> 0`) | — |
| `mailbox` | query | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |

**Cuerpo de la petición**: `application/json`:

| Campo | Tipo | Obligatorio | Restricciones |
| --- | --- | --- | --- |
| `to` | string | Sí | buzón destino no vacío tras `trim`; sin `CR`/`LF`/`NUL` |

```json
{ "to": "Archive" }
```

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón origen |
| `uid` | number | siempre | UID copiado |
| `to` | string | siempre | buzón destino |
| `status` | string | siempre | siempre `"copied"` |

```json
{ "mailbox": "INBOX", "uid": 42, "to": "Archive", "status": "copied" }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | copia realizada |
| `400` | `invalid_request` | `uid` no numérico o `0`; `mailbox` o `to` ausentes/blank o con caracteres de control; JSON malformado |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `404` | `message_not_found` | el `uid` no existe |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

> `copy` **no** produce `501 capability_not_supported`: `UID COPY` no depende de
> ninguna extensión.

**Ejemplo `curl`**

```bash
curl -fsS -X POST -H "Authorization: Bearer una-clave-secreta" \
  -H "Content-Type: application/json" \
  -d '{"to":"Archive"}' \
  "http://127.0.0.1:3000/api/messages/42/copy?mailbox=INBOX"
```

---

### `GET /api/messages/{uid}/body`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: trae el mensaje con `BODY.PEEK[]` (sin fijar `\Seen`), parsea su
  MIME en memoria y devuelve texto plano y HTML **descodificados**. El tamaño está
  acotado por `APIMAIL_MAX_MESSAGE_BYTES`.
- **Parámetros**

| Nombre | Ubicación | Tipo | Obligatorio | Restricciones | Por defecto |
| --- | --- | --- | --- | --- | --- |
| `uid` | path | integer | Sí | entero positivo (`> 0`) | — |
| `mailbox` | query | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |

**Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón |
| `uid` | number | siempre | UID |
| `text` | string \| null | siempre | texto plano; `null` si no hay parte mostrable ni derivable |
| `html` | string \| null | siempre | HTML; `null` si no hay parte mostrable ni derivable |

```json
{ "mailbox": "INBOX", "uid": 42, "text": "Hola mundo", "html": "<p>Hola</p>" }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | cuerpo devuelto |
| `400` | `invalid_request` | `uid` no numérico o `0`; `mailbox` ausente/blank o con caracteres de control; **no se envía ningún comando IMAP** |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `404` | `message_not_found` | el `uid` no existe |
| `413` | `message_too_large` | el mensaje supera `APIMAIL_MAX_MESSAGE_BYTES` (antes de parsear) |
| `422` | `message_not_parsable` | el mensaje existe pero su MIME no puede parsearse |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages/42/body?mailbox=INBOX"
```

**Seguridad**: el HTML se devuelve **sin sanear**; trátalo como contenido no
confiable.

---

### `GET /api/messages/{uid}/attachments`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: trae y parsea el mensaje y lista los adjuntos con sus metadatos
  y **sin** el contenido. Cada adjunto lleva un `id` posicional (0-based) estable.
- **Parámetros**

| Nombre | Ubicación | Tipo | Obligatorio | Restricciones | Por defecto |
| --- | --- | --- | --- | --- | --- |
| `uid` | path | integer | Sí | entero positivo (`> 0`) | — |
| `mailbox` | query | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |

**Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón |
| `uid` | number | siempre | UID |
| `attachments` | array | siempre | adjuntos (sin contenido) |
| `attachments[].id` | number | siempre | identificador posicional (0-based) |
| `attachments[].filename` | string \| null | siempre | nombre; `null` si no se reporta |
| `attachments[].content_type` | string \| null | siempre | MIME; `null` si no se reporta |
| `attachments[].size` | number | siempre | longitud del contenido decodificado (bytes) |
| `attachments[].inline` | bool | siempre | marcado `inline` |
| `attachments[].content_id` | string \| null | siempre | `Content-ID`; `null` si no se reporta |

```json
{
  "mailbox": "INBOX",
  "uid": 42,
  "attachments": [
    {
      "id": 0,
      "filename": "informe.pdf",
      "content_type": "application/pdf",
      "size": 13,
      "inline": false,
      "content_id": null
    }
  ]
}
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | listado devuelto |
| `400` | `invalid_request` | `uid` no numérico o `0`; `mailbox` ausente/blank o con caracteres de control; no se envía comando IMAP |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `404` | `message_not_found` | el `uid` no existe |
| `413` | `message_too_large` | el mensaje supera `APIMAIL_MAX_MESSAGE_BYTES` |
| `422` | `message_not_parsable` | el MIME no puede parsearse |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages/42/attachments?mailbox=INBOX"
```

---

### `GET /api/messages/{uid}/attachments/{id}`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: trae y parsea el mensaje y devuelve el contenido **decodificado**
  del adjunto `id`, codificado en **base64** dentro del JSON.
- **Parámetros**

| Nombre | Ubicación | Tipo | Obligatorio | Restricciones | Por defecto |
| --- | --- | --- | --- | --- | --- |
| `uid` | path | integer | Sí | entero positivo (`> 0`) | — |
| `id` | path | integer | Sí | entero no negativo (`0` es válido) | — |
| `mailbox` | query | string | Sí | no vacío; sin `CR`/`LF`/`NUL` | — |

**Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)**

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `mailbox` | string | siempre | buzón |
| `uid` | number | siempre | UID |
| `id` | number | siempre | identificador posicional del adjunto |
| `filename` | string \| null | siempre | nombre; `null` si no se reporta |
| `content_type` | string \| null | siempre | MIME; `null` si no se reporta |
| `size` | number | siempre | longitud del contenido decodificado (bytes) |
| `content_base64` | string | siempre | contenido decodificado en base64 |

```json
{
  "mailbox": "INBOX",
  "uid": 42,
  "id": 0,
  "filename": "informe.pdf",
  "content_type": "application/pdf",
  "size": 13,
  "content_base64": "UERGIGNvbnRlbnQhCg=="
}
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | adjunto devuelto |
| `400` | `invalid_request` | `uid` no numérico o `0`; `id` no numérico; `mailbox` ausente/blank o con caracteres de control; no se envía comando IMAP |
| `401` | `unauthorized` | clave ausente o inválida |
| `404` | `mailbox_not_found` | el servidor responde `NO` |
| `404` | `message_not_found` | el `uid` no existe |
| `404` | `attachment_not_found` | el `id` no corresponde a ningún adjunto |
| `413` | `message_too_large` | el mensaje supera `APIMAIL_MAX_MESSAGE_BYTES` |
| `422` | `message_not_parsable` | el MIME no puede parsearse |
| `503` | `imap_unavailable` | sesión o servidor no disponibles |

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages/42/attachments/0?mailbox=INBOX"
```

**Seguridad**: el contenido del adjunto solo viaja dentro del cuerpo JSON, nunca
en una cabecera HTTP.

---

### `POST /api/idle/start`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: arranca la suscripción IDLE (buzón `APIMAIL_IDLE_MAILBOX`), que
  usa su **propia** conexión IMAP dedicada y entrega al webhook a través de la cola
  durable. Es **idempotente**: si ya está en marcha devuelve el mismo cuerpo sin
  abrir una segunda conexión. Un arranque nuevo limpia cualquier `last_error`
  previo.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)** (objeto `IdleStatus`)

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `status` | string | siempre | `"running"` |
| `mailbox` | string | siempre | buzón vigilado |
| `last_error` | string \| null | siempre | código estable del último fallo o `null` |

```json
{ "status": "running", "mailbox": "INBOX", "last_error": null }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | suscripción arrancada (o ya en marcha) |
| `401` | `unauthorized` | clave ausente o inválida |
| `501` | `idle_not_configured` | no hay `APIMAIL_WEBHOOK_URL`; no se abre conexión |

**Ejemplo `curl`**

```bash
curl -fsS -X POST -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/idle/start
```

---

### `POST /api/idle/stop`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: detiene la suscripción IDLE y espera (acotado por un timeout
  interno) a sus tareas. Es **idempotente**: parar una suscripción que no está en
  marcha no hace nada.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)** (objeto `IdleStatus`)

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `status` | string | siempre | `"stopped"` |
| `mailbox` | string | siempre | buzón vigilado |
| `last_error` | string \| null | siempre | código estable del último fallo o `null` |

```json
{ "status": "stopped", "mailbox": "INBOX", "last_error": null }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | detenida (o ya detenida) |
| `401` | `unauthorized` | clave ausente o inválida |

**Ejemplo `curl`**

```bash
curl -fsS -X POST -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/idle/stop
```

---

### `GET /api/idle/status`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: informa del estado actual de la suscripción. Una tarea cuyo
  handle ya terminó se reporta como `stopped`.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)** (objeto `IdleStatus`)

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `status` | string | siempre | `"running"` o `"stopped"` |
| `mailbox` | string | siempre | buzón vigilado |
| `last_error` | string \| null | siempre | código estable del último fallo o `null` |

Valores de `last_error`:

- `null` — la última operación tuvo éxito.
- `"imap_unavailable"` — la conexión IMAP cayó; se reintenta con *backoff*.
- `"webhook_failed"` — la entrega al webhook falló; se reintenta con *backoff*.
- `"queue_unavailable"` — la cola no se pudo leer/escribir; la ingesta no reconoce el mensaje.

```json
{ "status": "running", "mailbox": "INBOX", "last_error": null }
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | siempre (con clave válida) |
| `401` | `unauthorized` | clave ausente o inválida |

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/idle/status
```

> Los fallos de IMAP y de webhook **no** devuelven código HTTP: la suscripción
> sigue viva y reconecta/reintenta; se reflejan solo en `last_error`.

---

### `GET /api/idle/queue`

- **Autenticación**: requiere `Authorization: Bearer <clave>`.
- **Descripción**: informa del estado observable de la cola de entrega. El
  snapshot ya deriva `Serialize` y se devuelve tal cual. Sin *side-effects*.
- **Parámetros**: ninguno.
- **Cuerpo de la petición**: ninguno.

**Respuesta 2xx (200)** (snapshot `QueueStats`)

| Campo | Tipo | Presencia | Significado |
| --- | --- | --- | --- |
| `persistent` | bool | siempre | `true` si la cola está respaldada por `APIMAIL_QUEUE_PATH`; `false` si vive en memoria |
| `pending` | number | siempre | notificaciones aún no entregadas |
| `delivered` | number | siempre | notificaciones entregadas al webhook |
| `dropped` | number | siempre | descartadas por superar `APIMAIL_QUEUE_MAX_ITEMS`/`APIMAIL_QUEUE_MAX_BYTES` |
| `failed` | number | siempre | descartadas tras agotar los intentos acotados (fallo no reintentable) |
| `oldest_pending_secs` | number \| null | siempre | antigüedad en segundos de la notificación pendiente más antigua; `null` si no hay ninguna |

```json
{
  "persistent": true,
  "pending": 0,
  "delivered": 42,
  "dropped": 0,
  "failed": 0,
  "oldest_pending_secs": null
}
```

**Códigos de estado**

| HTTP | error | Cuándo |
| --- | --- | --- |
| `200` | — | siempre (con clave válida) |
| `401` | `unauthorized` | clave ausente o inválida |

**Ejemplo `curl`**

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/idle/queue
```

---

## Apéndice: códigos `error` → HTTP → significado

| Código `error` | HTTP | Significado |
| --- | --- | --- |
| `unauthorized` | `401` | Falta la API key, el esquema no es `Bearer` o la clave es incorrecta. Incluye `WWW-Authenticate: Bearer`. |
| `invalid_request` | `400` | Petición malformada o fuera de límites: JSON/QUERY inválido, campo obligatorio ausente o en blanco, entero fuera de rango, fecha/booleano/`format` inválidos, bandera desconocida o contradictoria, carácter de control (`CR`/`LF`/`NUL`). Validado en el borde HTTP, antes de enviar comandos IMAP. |
| `mailbox_not_found` | `404` | El servidor respondió `NO` al `SELECT`: el buzón no existe. |
| `message_not_found` | `404` | El `uid` indicado no existe en el buzón (se comprueba antes que la capacidad del servidor). |
| `attachment_not_found` | `404` | El `id` indicado no corresponde a ningún adjunto del mensaje. |
| `message_too_large` | `413` | El mensaje supera `APIMAIL_MAX_MESSAGE_BYTES` y se rechaza antes de parsear su MIME. |
| `message_not_parsable` | `422` | El mensaje existe pero su contenido MIME no puede parsearse. |
| `payload_too_large` | `413` | En `POST /api/messages`: el cuerpo crudo supera el límite del cuerpo o los adjuntos decodificados superan `APIMAIL_MAX_ATTACHMENT_BYTES`. |
| `smtp_error` | `502` | El servidor SMTP rechaza o falla la entrega del mensaje. |
| `capability_not_supported` | `501` | El servidor no anuncia la extensión que la operación necesita (`MOVE`/`UIDPLUS`). |
| `idle_not_configured` | `501` | `POST /api/idle/start` sin `APIMAIL_WEBHOOK_URL`; no se abre conexión. |
| `imap_unavailable` | `503` | La sesión IMAP o el servidor no están disponibles; el mensaje es estable y no filtra credenciales. |

> `GET /api/health` (pública) no emite códigos `error`. Las respuestas `404`
> (ruta desconocida) y `405` (método no permitido) las produce axum con su cuerpo
> por defecto y **no** incluyen el envoltorio `{"error":...,"message":...}`.
