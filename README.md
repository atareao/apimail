# apimail

API HTTP escrita en Rust con [Axum](https://github.com/tokio-rs/axum) para
gestionar una cuenta de correo a través de IMAP y SMTP.

El objetivo del proyecto es exponer un CRUD HTTP sobre un buzón de correo:
leer, buscar, mover o copiar mensajes, gestionar banderas (`\Seen`, `\Flagged`,
`\Deleted`) y `EXPUNGE`, descargar cabeceras, cuerpo y adjuntos, enviar correo
saliente por SMTP, y recibir mensajes en tiempo real mediante la extensión
`IDLE` de IMAP con notificación a un endpoint configurable (webhook).

> Estado actual: esqueleto de la API. Ya funcionan el arranque configurable por
> variables de entorno y el endpoint de salud `GET /api/health`. Las operaciones
> IMAP/SMTP están en desarrollo.

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

| Variable        | Descripción                | Valor por defecto |
| --------------- | -------------------------- | ----------------- |
| `APIMAIL_HOST`  | Dirección de escucha       | `0.0.0.0`         |
| `APIMAIL_PORT`  | Puerto de escucha (`u16`)  | `3000`            |

Ejemplo:

```bash
APIMAIL_HOST=127.0.0.1 APIMAIL_PORT=8080 cargo run
```

Si `APIMAIL_PORT` no es un `u16` válido, el arranque falla con un error
descriptivo y un código de salida distinto de cero (no hay *fallback*
silencioso al valor por defecto).

## Endpoint de salud

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

Ejemplo con `curl`:

```bash
curl -fsS http://127.0.0.1:3000/api/health
```

## Licencia

Este proyecto se distribuye bajo la licencia **MIT**. Consulta el fichero
[LICENSE](LICENSE) para el texto completo.

Copyright (c) 2026 Lorenzo Carbonell.
