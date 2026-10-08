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
> mediante `GET /api/imap/status`. El resto de operaciones de lectura (búsqueda,
> banderas, `IDLE`) siguen en desarrollo.

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
