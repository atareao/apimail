# Design

## Context

Ver `proposal.md` — Why. Estado de partida (v0.1.0): `build_router()` no recibe
argumentos, `Config` solo tiene `host`/`port`, y el único endpoint es
`GET /api/health`. El proyecto es un crate **lib + bin** con tests que ejercitan
el `Router` vía `tower::ServiceExt::oneshot`, sin abrir sockets.

## Goals / Non-Goals

**Goals:**
- Toda la API protegida salvo `GET /api/health`.
- Configuración *fail-closed*: sin clave, el servicio no arranca.
- Lógica **testeable sin mutar el entorno** (reutiliza `Config::from_lookup`).
- Contrato de error `401` estable y consumible (JSON + `WWW-Authenticate`).
- Dejar la puerta abierta a los endpoints de correo sin tocar el middleware.

**Non-Goals:**
- Multi-usuario, JWT/OIDC, roles/scopes, rotación de claves.
- Rate limiting, CORS, TLS de terminación.
- Credenciales de correo (llegan con `mail-account`).

## Decisions

- **API key estática en `Authorization: Bearer <key>`**, no `X-API-Key`:
  reutiliza un esquema estándar, ya entendido por clientes y proxies, y encaja
  con `WWW-Authenticate: Bearer`.
  *Alternativas descartadas:* `X-API-Key` (menos convencional); query param
  (se filtra en logs); basic auth (pensado para usuario/contraseña).
- ***Fail-closed*: `APIMAIL_API_KEY` obligatoria y no vacía.** Si falta, el
  arranque falla. Un valor por defecto o un modo "sin auth" convertirían el
  olvido de configuración en una API pública.
- **Comparación en tiempo constante** con el crate `subtle`
  (`ConstantTimeEq`). Es barato y evita filtrar por temporización. La `length`
  se normaliza comparando siempre cadenas del mismo tamaño lógico mediante el
  propio `subtle` sobre los bytes.
- **Middleware por capa** (`axum::middleware::from_fn_with_state`) montado con
  `route_layer` sobre el sub-router protegido. Así el `404` de rutas
  desconocidas y el `405` de métodos incorrectos se mantienen intactos, y las
  rutas nuevas se añaden al router protegido sin tocar el middleware.
  *Alternativa descartada:* envolver todo el router y excluir `/api/health` con
  un *matcher* (más frágil).
- **`build_router(state: AppState)`**: el router deja de construir su propio
  estado y lo recibe. Hace explícita la dependencia (la clave) y mantiene los
  tests inyectables sin variables de entorno.
- **`AppState { name, version, api_key }`**, construido con
  `AppState::from_config(&Config)`.
- **Error `401`**: `{"error":"unauthorized","message":"..."}` con
  `Content-Type: application/json` y `WWW-Authenticate: Bearer`. Se implementa
  como un tipo de error propio que implementa `IntoResponse`, embrión del
  modelo de errores unificado del roadmap.
- **`GET /api/whoami`** como endpoint de andamiaje: sin dependencias de correo,
  permite verificar el middleware con una ruta real protegida.

*Orden de validación en arranque:* host → port → api key (declaración del
`struct`), de modo que un `APIMAIL_PORT` inválido sigue reportándose como tal.

## Risks / Trade-offs

- [Una clave estática es un único secreto compartido] → Aceptable para un
  servicio interno; JWT/OIDC queda para más adelante si aparecen varios usuarios.
- [La clave viaja en claro si no hay TLS] → El despliegue debe terminar TLS por
  delante (proxy/ingress); fuera de alcance aquí. No registrar la cabecera.
- [El endpoint `whoami` es artificial y podría quedar huérfano] → Su requisito
  queda definido en `api-auth`; se puede retirar o reubicar cuando existan
  endpoints reales, vía un change posterior.
- [`build_router()` cambia de firma] → Es API interna, pero obliga a tocar los
  tests existentes; se hace dentro de este change (RED primero).

## Migration Plan

No hay despliegue en producción todavía. Quien ejecute `cargo run` en local
deberá exportar `APIMAIL_API_KEY` (p. ej. `APIMAIL_API_KEY=dev-key`). El
`README`/`AGENTS.md` pueden documentarlo en un change de documentación.
