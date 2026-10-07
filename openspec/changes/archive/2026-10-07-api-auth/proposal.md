# Proposal

## Why

Desde v0.1.0 la API arranca y expone `GET /api/health`, pero **no tiene ninguna
protección**: cualquiera con acceso de red puede invocarla. Como el roadmap va a
añadir operaciones con credenciales de correo (leer, borrar, enviar, adjuntos),
necesitamos cerrar el acceso **antes** de seguir: si dejamos el CRUD abierto
mientras se construye, cada endpoint nuevo nace sin autenticar.

## What Changes

- Nueva capability `api-auth`: **API key estática** configurada por entorno
  (`APIMAIL_API_KEY`), presentada en la cabecera `Authorization: Bearer <key>`.
- El arranque **falla** si `APIMAIL_API_KEY` falta o está vacía (*fail-closed*):
  el servicio nunca corre sin protección.
- Todas las rutas quedan protegidas **salvo** `GET /api/health`, que sigue
  siendo público para sondas de liveness/CI.
- Nuevo endpoint protegido `GET /api/whoami` (andamiaje) que devuelve
  `{"authenticated":true}`: da una ruta real sobre la que verificar el middleware
  hasta que existan los endpoints de correo.
- Respuestas de rechazo: `401` con cuerpo `application/json` y cabecera
  `WWW-Authenticate: Bearer`.
- Comparación de la clave en **tiempo constante** para no filtrar información
  por temporización.

BREAKING interno (no publicado como contrato estable): `build_router()` pasa a
recibir el estado (`AppState`), y `Config`/`ConfigError` ganan el campo y la
variante de la API key.

## Capabilities

### New Capabilities
- `api-auth`: API key estática, middleware de autorización, endpoint
  `GET /api/whoami` y respuestas `401`.

### Modified Capabilities
- `http-api`: el requisito *Server startup and configuration* pasa a exigir
  también una API key válida; `GET /api/health` se declara explícitamente público.

## Impact

- `Cargo.toml`: añadir `subtle` (comparación en tiempo constante) y `sha2`
  (digest SHA-256 de tamaño fijo para comparar sin filtrar la longitud de la clave).
- `src/config.rs`: `Config.api_key`, `ConfigError::MissingApiKey` y su validación.
- `src/http.rs`: `AppState` con la clave, middleware `require_api_key`, handler
  `whoami`, y `build_router(state)`.
- `src/main.rs`: construir el estado desde `Config`.
- Tests: nuevo `tests/auth.rs`; `tests/health.rs` y `tests/startup.rs` actualizados.
- Habilita que el resto del roadmap (IMAP/SMTP/MIME/IDLE) se construya ya protegido.
- Fuera de alcance: multi-usuario, JWT/OIDC, rotación de claves, rate limiting,
  HTTPS/TLS de terminación y gestión de credenciales de correo.
