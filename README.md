# apimail

API HTTP escrita en Rust con [Axum](https://github.com/tokio-rs/axum) para
gestionar una cuenta de correo a través de IMAP y SMTP.

El objetivo del proyecto es exponer un CRUD HTTP sobre un buzón de correo:
leer, buscar, mover o copiar mensajes, gestionar banderas (`\Seen`, `\Flagged`,
`\Deleted`) y `EXPUNGE`, descargar cabeceras, cuerpo y adjuntos, enviar correo
saliente por SMTP, y recibir mensajes en tiempo real mediante la extensión
`IDLE` de IMAP con notificación a un endpoint configurable (webhook).

> Estado actual: esqueleto de la API con **autenticación**. Ya funcionan el
> arranque configurable por variables de entorno, el endpoint de salud público
> `GET /api/health` y la autenticación por API key. Las operaciones IMAP/SMTP
> están en desarrollo.

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

El servidor se configura mediante variables de entorno:

| Variable           | Descripción                                   | Valor por defecto   |
| ------------------ | --------------------------------------------- | ------------------- |
| `APIMAIL_HOST`     | Dirección de escucha                          | `0.0.0.0`           |
| `APIMAIL_PORT`     | Puerto de escucha (`u16`)                     | `3000`              |
| `APIMAIL_API_KEY`  | API key exigida para autenticar las peticiones | — (**obligatoria**) |

Ejemplo:

```bash
APIMAIL_API_KEY=una-clave-secreta APIMAIL_HOST=127.0.0.1 APIMAIL_PORT=8080 cargo run
```

El arranque es **fail-closed**: si `APIMAIL_API_KEY` falta o está vacía, el
servicio **no arranca** (error descriptivo y código de salida distinto de cero)
en lugar de quedar expuesto sin protección. Del mismo modo, si `APIMAIL_PORT` no
es un `u16` válido el arranque falla, sin *fallback* silencioso al valor por
defecto.

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

## Licencia

Este proyecto se distribuye bajo la licencia **MIT**. Consulta el fichero
[LICENSE](LICENSE) para el texto completo.

Copyright (c) 2026 Lorenzo Carbonell.
