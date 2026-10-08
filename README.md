# apimail

API HTTP escrita en Rust con [Axum](https://github.com/tokio-rs/axum) para
gestionar una cuenta de correo a través de IMAP y SMTP.

El objetivo del proyecto es exponer un CRUD HTTP sobre un buzón de correo:
leer, buscar, mover o copiar mensajes, gestionar banderas (`\Seen`, `\Flagged`,
`\Deleted`) y `EXPUNGE`, descargar cabeceras, cuerpo y adjuntos, enviar correo
saliente por SMTP, y recibir mensajes en tiempo real mediante la extensión
`IDLE` de IMAP con notificación a un endpoint configurable (webhook).

> Estado actual: esqueleto de la API con **autenticación** y **cuenta de correo
> configurada**. Ya funcionan el arranque configurable por variables de entorno,
> el endpoint de salud público `GET /api/health`, la autenticación por API key y
> la inspección de la cuenta con `GET /api/account`. **Ya se puede enviar correo**
> con `POST /api/messages`. La **conexión de lectura a IMAP** ya está disponible
> mediante `GET /api/imap/status`, se pueden **listar y seleccionar buzones**
> con `GET /api/mailboxes` y `POST /api/mailboxes/select`, y **leer y buscar
> mensajes** con `GET /api/messages` y `GET /api/messages/{uid}`. El resto de
> operaciones de lectura (banderas, `IDLE`) siguen en desarrollo.

## Requisitos

- **Rust edition 2024** (se recomienda una toolchain reciente; el proyecto se
  compila con las versiones compatibles fijadas en `Cargo.toml`).
- Conexión a un servidor IMAP/SMTP para las funcionalidades de correo (envío
  SMTP y comprobación de la conexión IMAP; el resto de operaciones de lectura
  siguen en desarrollo).

## Construcción

```bash
cargo build
```

Para una build optimizada:

```bash
cargo build --release
```

## Ejecución

```bash
cargo run
```

El servidor se configura mediante variables de entorno.

### Servidor HTTP

| Variable           | Descripción                                     | Valor por defecto   |
| ------------------ | ----------------------------------------------- | ------------------- |
| `APIMAIL_HOST`     | Dirección de escucha                            | `0.0.0.0`           |
| `APIMAIL_PORT`     | Puerto de escucha (`u16`)                       | `3000`              |
| `APIMAIL_API_KEY`  | API key exigida para autenticar las peticiones  | — (**obligatoria**) |

### Cuenta de correo

Cada endpoint (IMAP y SMTP) requiere **host, usuario y secreto**. El puerto y el
modo TLS son opcionales.

| Variable                    | Descripción                             | Valor por defecto            |
| --------------------------- | --------------------------------------- | ---------------------------- |
| `APIMAIL_IMAP_HOST`         | Servidor IMAP                           | — (**obligatoria**)          |
| `APIMAIL_IMAP_USER`         | Usuario IMAP                            | — (**obligatoria**)          |
| `APIMAIL_IMAP_PASSWORD`     | Secreto IMAP (nunca se registra)        | — (**obligatoria**)          |
| `APIMAIL_IMAP_PORT`         | Puerto IMAP                             | según TLS (ver abajo)        |
| `APIMAIL_IMAP_TLS`          | Modo TLS (`implicit`/`starttls`/`none`) | `implicit`                   |
| `APIMAIL_SMTP_HOST`         | Servidor SMTP                           | — (**obligatoria**)          |
| `APIMAIL_SMTP_USER`         | Usuario SMTP                            | — (**obligatoria**)          |
| `APIMAIL_SMTP_PASSWORD`     | Secreto SMTP (nunca se registra)        | — (**obligatoria**)          |
| `APIMAIL_SMTP_PORT`         | Puerto SMTP                             | según TLS (ver abajo)        |
| `APIMAIL_SMTP_TLS`          | Modo TLS (`implicit`/`starttls`/`none`) | `implicit`                   |

### Conexión IMAP

| Variable                     | Descripción                                              | Valor por defecto |
| ---------------------------- | -------------------------------------------------------- | ----------------- |
| `APIMAIL_IMAP_TIMEOUT_SECS`  | Timeout (segundos) para TCP, handshake TLS y login IMAP  | `30`              |

`APIMAIL_IMAP_TIMEOUT_SECS` debe ser un entero **mayor que cero**; un valor `0`
o no numérico provoca un **error de arranque** (menciona la variable), sin
*fallback* silencioso.

### Envío

| Variable                        | Descripción                                     | Valor por defecto    |
| ------------------------------- | ----------------------------------------------- | -------------------- |
| `APIMAIL_MAX_ATTACHMENT_BYTES`  | Tamaño máximo total de los adjuntos (en bytes)  | `10485760` (10 MiB)  |

Puertos por defecto **derivados del modo TLS** (si no se fija `..._PORT`):

| Modo TLS   | IMAP  | SMTP  |
| ---------- | ----- | ----- |
| `implicit` | `993` | `465` |
| `starttls` | `143` | `587` |
| `none`     | `143` | `587` |

Ejemplo:

```bash
export APIMAIL_API_KEY=una-clave-secreta
export APIMAIL_HOST=127.0.0.1 APIMAIL_PORT=8080
export APIMAIL_IMAP_HOST=imap.example.com APIMAIL_IMAP_USER=me@example.com APIMAIL_IMAP_PASSWORD=secret
export APIMAIL_SMTP_HOST=smtp.example.com APIMAIL_SMTP_USER=me@example.com APIMAIL_SMTP_PASSWORD=secret
cargo run
```

El arranque es **fail-closed**: si `APIMAIL_API_KEY` falta o está vacía, o si
falta (o está en blanco) cualquiera de los valores obligatorios de la cuenta
(`..._HOST`, `..._USER`, `..._PASSWORD`), el servicio **no arranca** (error
descriptivo que nombra la variable y código de salida distinto de cero) en lugar
de quedar expuesto o a medias. Del mismo modo, un `APIMAIL_PORT` que no sea un
`u16`, un `..._PORT` de correo fuera de `1..=65535`, un `..._TLS` desconocido o
un `APIMAIL_IMAP_TIMEOUT_SECS` que no sea un entero mayor que cero provocan un
error de arranque, sin *fallback* silencioso.

> El modo TLS `none` se acepta (útil para servidores de prueba locales como
> MailHog o GreenMail) pero registra un **aviso**: las credenciales viajarían en
> claro.

## Autenticación

Todas las rutas quedan protegidas **salvo** `GET /api/health`. Las peticiones
deben presentar la API key en la cabecera `Authorization` con el esquema
`Bearer`:

```http
Authorization: Bearer <APIMAIL_API_KEY>
```

El esquema `Bearer` no distingue mayúsculas/minúsculas. La clave se compara en
**tiempo constante** (a partir de su digest SHA-256, de tamaño fijo), de modo que
no se filtra información por temporización.

Si la cabecera falta, usa otro esquema o la clave es incorrecta, la respuesta es
`401 Unauthorized` con cuerpo `application/json` y la cabecera
`WWW-Authenticate: Bearer`:

```json
{
  "error": "unauthorized",
  "message": "missing or invalid API key"
}
```

Ejemplo con `curl`:

```bash
# Ruta protegida con clave válida → 200
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/whoami

# Sin cabecera → 401
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:3000/api/whoami
```

## Endpoints

### Salud (público)

```http
GET /api/health
```

Respuesta `200 OK` con `Content-Type: application/json`:

```json
{
  "status": "ok",
  "name": "apimail",
  "version": "0.1.0"
}
```

```bash
curl -fsS http://127.0.0.1:3000/api/health
```

### Identidad autenticada (protegido)

```http
GET /api/whoami
```

Ruta de andamiaje que confirma que la autenticación ha tenido éxito. Requiere la
API key y devuelve `200 OK` con:

```json
{
  "authenticated": true
}
```

### Cuenta de correo (protegido)

```http
GET /api/account
```

Devuelve **solo el host y el puerto** de los endpoints IMAP y SMTP (nunca el
usuario ni el secreto). Requiere la API key:

```json
{
  "imap": { "host": "imap.example.com", "port": 993 },
  "smtp": { "host": "smtp.example.com", "port": 465 }
}
```

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/account
```

### Estado de la conexión IMAP (protegido)

```http
GET /api/imap/status
```

Asegura una sesión IMAP viva y reporta el resultado. Requiere la API key y
**solo** expone el host, el puerto y el modo TLS configurados; **nunca** el
usuario ni el secreto.

Respuesta `200 OK` con `Content-Type: application/json` cuando la conexión está
disponible:

```json
{
  "connected": true,
  "host": "imap.example.com",
  "port": 993,
  "tls": "implicit"
}
```

`tls` es una de las etiquetas del modo configurado: `implicit`, `starttls` o
`none`.

Si no se puede establecer la conexión, la respuesta es `503 Service Unavailable`
con `Content-Type: application/json`:

```json
{
  "connected": false,
  "error": "imap_unavailable",
  "message": "could not connect to the IMAP server"
}
```

El código `error` es estable y `message` no revela detalles internos (direcciones,
credenciales ni trazas).

> La conexión IMAP es **perezosa**: el servicio **arranca aunque el servidor IMAP
> no esté disponible** (no se abre red en el arranque). Este endpoint refleja el
> estado real en el momento de la consulta; cada petición intenta (re)conectar si
> no hay una sesión viva.

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/imap/status
```

### Buzones (protegido)

#### Listar buzones

```http
GET /api/mailboxes
```

Lista los buzones de la cuenta (`LIST`). Requiere la API key y **nunca** expone
credenciales. Respuesta `200 OK` con `Content-Type: application/json`:

```json
{
  "mailboxes": [
    { "name": "INBOX", "delimiter": "/", "attributes": ["\\HasNoChildren"] },
    { "name": "Sent", "delimiter": "/", "attributes": ["\\Sent"] }
  ]
}
```

`delimiter` es `null` cuando el servidor no define jerarquía y `attributes`
recoge los atributos del buzón en estilo IMAP (`\NoSelect`, `\HasNoChildren`,
`\Sent`…).

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  http://127.0.0.1:3000/api/mailboxes
```

Si el servidor o la sesión no están disponibles, la respuesta es
`503 Service Unavailable` con el modelo de error habitual
(`{"error":"imap_unavailable","message":"..."}`).

#### Seleccionar un buzón

```http
POST /api/mailboxes/select
```

Selecciona el buzón indicado (`SELECT`) y lo deja como buzón activo para las
operaciones siguientes. Requiere la API key y un cuerpo `application/json`:

```json
{ "mailbox": "INBOX" }
```

Respuesta `200 OK` con `Content-Type: application/json`:

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

`unseen`, `uid_validity` y `uid_next` son `null` cuando el servidor omite el
valor.

```bash
curl -fsS -X POST -H "Authorization: Bearer una-clave-secreta" \
  -H "Content-Type: application/json" \
  -d '{"mailbox":"INBOX"}' \
  http://127.0.0.1:3000/api/mailboxes/select
```

Códigos de error (mismo modelo `{"error":...,"message":...}`):

- `400` (`invalid_request`) — cuerpo JSON malformado o `mailbox` ausente o en
  blanco.
- `404` (`mailbox_not_found`) — el servidor responde `NO`: el buzón no existe.
- `503` (`imap_unavailable`) — el servidor o la sesión no están disponibles; el
  mensaje es estable y no filtra credenciales.
- `401` (`unauthorized`) — sin API key válida.

> Igual que `GET /api/imap/status`, ambas rutas reutilizan la sesión IMAP
> perezosa: no abren una conexión nueva mientras haya una viva.

### Lectura de mensajes (protegido)

#### Listar y buscar mensajes

```http
GET /api/messages?mailbox=INBOX
```

Selecciona el buzón indicado y devuelve los mensajes que cumplen los criterios
de búsqueda, **paginados** y **ordenados por `UID` descendente** (los más
recientes primero). Requiere la API key y **nunca** expone credenciales.

Parámetros de consulta:

| Parámetro | Descripción                                                                 | Por defecto |
| --------- | --------------------------------------------------------------------------- | ----------- |
| `mailbox` | Buzón a inspeccionar (**obligatorio**, no vacío)                            | —           |
| `from`    | Filtro `FROM` (remitente)                                                   | —           |
| `to`      | Filtro `TO` (destinatario)                                                  | —           |
| `subject` | Filtro `SUBJECT` (asunto)                                                   | —           |
| `text`    | Filtro `BODY` (contenido)                                                   | —           |
| `since`   | Mensajes recibidos en/después de `YYYY-MM-DD` (`SINCE`)                     | —           |
| `before`  | Mensajes recibidos antes de `YYYY-MM-DD` (`BEFORE`)                         | —           |
| `seen`    | `true` (`SEEN`) o `false` (`UNSEEN`)                                        | —           |
| `unseen`  | Alias de `seen`: `unseen=true` equivale a `seen=false` y viceversa          | —           |
| `flagged` | `true` (`FLAGGED`) o `false` (`UNFLAGGED`)                                  | —           |
| `limit`   | Tamaño de página (`1`..`200`)                                               | `50`        |
| `offset`  | Desplazamiento (`>= 0`)                                                     | `0`         |

Respuesta `200 OK` con `Content-Type: application/json`:

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

`total` cuenta **todos** los mensajes que cumplen la búsqueda; `messages` recoge
solo la ventana solicitada. `size`, `internal_date`, `envelope` y sus campos son
`null` cuando el servidor no los reporta.

```bash
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages?mailbox=INBOX&subject=Hola&limit=20&offset=0"
```

Códigos de error (modelo `{"error":...,"message":...}`):

- `400` (`invalid_request`) — `mailbox` ausente o en blanco, fecha que no es
  `YYYY-MM-DD`, booleano que no es `true`/`false`, `limit`/`offset` fuera de
  rango, `seen` y `unseen` contradictorios, o un carácter de control
  (`CR`/`LF`/`NUL`) en cualquier valor.
- `404` (`mailbox_not_found`) — el servidor responde `NO`: el buzón no existe.
- `503` (`imap_unavailable`) — el servidor o la sesión no están disponibles.
- `401` (`unauthorized`) — sin API key válida.

#### Descargar un mensaje

```http
GET /api/messages/{uid}?mailbox=INBOX
```

Selecciona el buzón y descarga el mensaje `uid` indicado. Requiere la API key.

| Parámetro | Descripción                                                             | Por defecto |
| --------- | ---------------------------------------------------------------------- | ----------- |
| `mailbox` | Buzón que contiene el mensaje (**obligatorio**, no vacío)              | —           |
| `format`  | `summary`, `headers` o `full`                                          | `summary`   |

Respuesta `200 OK` con `Content-Type: application/json`:

- `summary` — metadatos y envelope (mismos campos que un elemento de `messages`)
  más `"format":"summary"`.
- `headers` — igual que `summary`, más `"format":"headers"` y `headers_base64`,
  que es el **bloque crudo de cabeceras** codificado en base64.
- `full` — igual que `summary`, más `"format":"full"` y `raw_base64`, que es el
  **mensaje RFC822 completo** codificado en base64.

El contenido MIME crudo no es necesariamente UTF-8, por eso las cabeceras y el
mensaje completo se transportan en **base64** (un JSON válido debe ser UTF-8).

```bash
# Metadatos (por defecto)
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages/42?mailbox=INBOX"

# Mensaje completo en base64
curl -fsS -H "Authorization: Bearer una-clave-secreta" \
  "http://127.0.0.1:3000/api/messages/42?mailbox=INBOX&format=full"
```

Códigos de error (modelo `{"error":...,"message":...}`):

- `400` (`invalid_request`) — `uid` no numérico o `0`, `mailbox` ausente o en
  blanco, o `format` distinto de `summary`/`headers`/`full`.
- `404` (`mailbox_not_found`) — el servidor responde `NO`: el buzón no existe.
- `404` (`message_not_found`) — el `uid` indicado no existe en el buzón.
- `503` (`imap_unavailable`) — el servidor o la sesión no están disponibles.
- `401` (`unauthorized`) — sin API key válida.

> **Seguridad:** las dos rutas de lectura piden los cuerpos con `BODY.PEEK[...]`,
> de modo que **leer un mensaje nunca fija la bandera `\Seen`**. Además, tanto
> `GET /api/messages` como `GET /api/messages/{uid}` reutilizan la sesión IMAP
> perezosa (solo una conexión mientras esté viva) y los valores de búsqueda se
> citan y escapan, rechazando `CR`/`LF`/`NUL`, para que ninguna petición pueda
> inyectar comandos IMAP.

### Envío de correo (protegido)

```http
POST /api/messages
```

Envía un correo a través del SMTP configurado. Requiere la API key.

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

- `to` es obligatorio (al menos una dirección); el resto de campos son opcionales.
- Los adjuntos van en **base64** y su tamaño total ya decodificado no puede
  superar `APIMAIL_MAX_ATTACHMENT_BYTES`.
- Si se omite `from`, se usa el usuario SMTP configurado (`APIMAIL_SMTP_USER`).

Respuesta `200 OK` con `Content-Type: application/json`:

```json
{ "status": "sent" }
```

Códigos de error: `400` (petición inválida), `413` (adjuntos demasiado grandes),
`502` (fallo del servidor SMTP) y `401` (sin API key).

```bash
curl -fsS -X POST -H "Authorization: Bearer una-clave-secreta" \
  -H "Content-Type: application/json" \
  -d '{"to":["dest@example.com"],"subject":"Hola","text":"cuerpo"}' \
  http://127.0.0.1:3000/api/messages
```

## Licencia

Este proyecto se distribuye bajo la licencia **MIT**. Consulta el fichero
[LICENSE](LICENSE) para el texto completo.

Copyright (c) 2026 Lorenzo Carbonell.
