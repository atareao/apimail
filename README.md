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
> la inspección de la cuenta con `GET /api/account`. La conexión real a IMAP/SMTP
> está en desarrollo.

## Requisitos

- **Rust edition 2024** (se recomienda una toolchain reciente; el proyecto se
  compila con las versiones compatibles fijadas en `Cargo.toml`).
- Conexión a un servidor IMAP/SMTP para las funcionalidades de correo (todavía
  no implementadas).

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
`u16`, un `..._PORT` de correo fuera de `1..=65535` o un `..._TLS` desconocido
provocan un error de arranque, sin *fallback* silencioso.

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

## Licencia

Este proyecto se distribuye bajo la licencia **MIT**. Consulta el fichero
[LICENSE](LICENSE) para el texto completo.

Copyright (c) 2026 Lorenzo Carbonell.
