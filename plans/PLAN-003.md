# PLAN-003.md — apimail

Trabajo planificado a partir de **`v0.3.0`** (+ `container-image`), ítem **#9 `openapi`** de
`plans/PLAN-001.md` §2.

Este documento **no sustituye a las specs de OpenSpec**: es planificación, no una propuesta
aprobada. **No se escribe ni una línea de código** (ni de tests, ni de `Cargo.toml`, ni de
`docs/API.md`) hasta que exista un change aprobado en `openspec/changes/openapi/` y el usuario lo
apruebe explícitamente (ver `AGENTS.md`).

---

## 1. Objetivo

Publicar una **especificación OpenAPI 3.1** de la API de apimail, generada desde el propio código
(sin duplicar rutas), y servir una **UI de documentación** interactiva para poder explorar y probar
los endpoints.

## 2. Alcance

- **Especificación OpenAPI 3.1** (y, si el soporte lo permite, 3.2) generada desde los *handlers*.
- **Sin duplicar rutas**: el registro de rutas y la documentación comparten el mismo punto de
  definición (`OpenApiRouter` + macro de path).
- **UI de documentación** servida por el propio axum (Scalar por defecto; alternativas evaluadas).
- **Seguridad documentada**: esquema `HTTP Bearer` para la API key.
- **Respuestas de error** documentadas por código de estado (contrato `{error,message}`).
- **Endpoint de especificación** (p. ej. `/api-docs/openapi.json`) consumible por herramientas.
- **Guardarraíles**: test de *snapshot* de la spec y lint con Spectral en CI.

## 3. Fuera de alcance

- **Cambiar el comportamiento** de la API: esto documenta lo que ya existe; cualquier cambio de
  contrato va en su propio change.
- **Reescribir `docs/API.md`**: en este ítem no se toca (lo prohíbe el encargo); una eventual
  convergencia con la spec generada se decide en otro momento.
- Generación de **clientes SDK** en varios lenguajes (posible ítem futuro).
- Versionado de la API distinto del *tag* del proyecto.

## 4. Decisiones técnicas

- **Stack recomendado — `utoipa` 6.0.0** + `utoipa-gen` 6.0.1 (publicado **2026-09-22**):
  - licencia **MIT OR Apache-2.0**; **MSRV 1.88**, que coincide con el `rust-version = "1.88"`;
  - soporta **OpenAPI 3.1** (y **3.2** en la 6.0);
  - repo `juhaku/utoipa` (**~4,1k ★**, push **2026-10-05**).
- **Integración axum sin duplicar rutas — `utoipa-axum` 0.3.0** (2026-09-22, depende de
  `axum ^0.8.4`): `OpenApiRouter` + `routes!(handler)`; `.split_for_parts()` devuelve
  `(Router, OpenApi)`. **El `path` vive en la macro `#[utoipa::path]`**, no en el `Router`.
- **UIs** (todas del mismo workspace, `axum ^0.8.4`):
  - **`utoipa-scalar` 0.4.0** — **recomendada** por moderna;
  - **`utoipa-swagger-ui` 10.0.1** — ⚠️ su `build.rs` **descarga** los assets en tiempo de
    compilación ⇒ en CI/contenedor usar el feature **`vendored`** o fijar
    `SWAGGER_UI_DOWNLOAD_URL`;
  - `utoipa-redoc` 7.0.0;
  - `utoipa-rapidoc` 7.0.0.
- **Descartados y por qué**:
  - `aide` 0.16.0-alpha.4 (estable 0.15.1) — válido, pero el canal estable va por detrás y usa
    `schemars`;
  - `okapi` 0.7.0 — **sin release desde 2024-01**;
  - `paperclip` 0.9.7 y `apistos` 0.9.0 — **solo actix-web**.
- **Coste de integración**: añadir `#[derive(ToSchema)]` a los **DTOs** y a los **enums de error**
  (`{error,message}`); declarar `SecurityScheme::Http{ scheme: Bearer }`; documentar las respuestas
  de error por *status*.
- **Mantener sincronizado**: **test de snapshot** del `/api-docs/openapi.json` (p. ej. con
  `insta`) y lint con **Spectral** en CI.
- **Re-verificación obligatoria antes de implementar**: versiones de `utoipa`/`utoipa-axum`, de las
  UIs y el soporte de la versión de OpenAPI cambian con rapidez; la spec debe fijar versiones
  exactas.

## 5. Requisitos candidatos para la spec

A redactar en `openspec/changes/openapi/specs/<capability>/spec.md` como `## Requirement` /
`#### Scenario` (redacción definitiva en el change, no aquí):

- **`openapi-spec`**: el servidor SHALL exponer una especificación OpenAPI válida (documento 3.1)
  que cubra todas las rutas de la API; escenarios de cobertura total, validez del documento y
  presencia de `securitySchemes`.
- **`openapi-route-registration`**: documentación y registro de rutas SHALL compartir la misma
  fuente (`OpenApiRouter`), de modo que una ruta nueva no pueda quedar sin documentar; escenario de
  no-duplicación.
- **`openapi-ui`**: el servidor SHALL servir una UI de documentación navegable; escenario de carga
  de la UI y de resolución de la spec asociada.
- **`openapi-error-responses`**: cada ruta SHALL documentar sus respuestas de error con el formato
  `{error,message}`; escenarios de los códigos que **realmente** devuelve cada ruta
  (`400`, `401`, `404`, `413`, `422`, `501`, `502`, `503`), sin inventar `403`/`500` que la API no
  produce.
- **`openapi-snapshot`**: un test SHALL fallar si la especificación generada cambia sin
  actualización explícita del snapshot; escenario de detección de deriva.
- **`openapi-build-time`**: la generación SHALL no degradar la compilación más allá de lo aceptado
  (sin assets descargados en tiempo de compilación por defecto); escenario en CI/contenedor.

## 6. Riesgos

- **Doble sistema de esquemas**: si además se hace `plans/PLAN-002.md` (MCP), se convive con
  `ToSchema` de `utoipa` y `schemars::JsonSchema` (MCP) para los **mismos** tipos ⇒ dos
  derivaciones a mantener sincronizadas.
- **`utoipa-swagger-ui` descarga assets en `build.rs`** ⇒ build frágil/offline-hostil en CI y en el
  contenedor salvo que se use `vendored` o se fije `SWAGGER_UI_DOWNLOAD_URL`.
- **Tamaño del binario**: la UI `vendored` añade varios MB; servir por CDN o con Scalar lo deja casi
  despreciable.
- **Tiempo de compilación**: las macros proc de `utoipa` lo incrementan (a vigilar).
- **Visibilidad de tipos**: documentar puede obligar a **hacer públicos** tipos hoy privados ⇒
  ruido en la API interna.
- **Deriva de la spec**: sin snapshot + Spectral, la documentación se queda obsoleta en silencio.

## 7. Impacto

- **Documentación/distribución**; sin cambio de contrato de los endpoints existentes.
- **`build_router(state) -> Router`** puede pasar a devolver **`(Router, OpenApi)`** usando
  `.split_for_parts()`; los *call sites* (bin, tests) se adaptan.
- **`Cargo.toml`**: dependencias nuevas (`utoipa`, `utoipa-axum`, UI elegida, `insta` en dev); **no
  se toca antes del change aprobado**.
- **DTOs y errores**: `#[derive(ToSchema)]` en tipos de respuesta y en los enums `{error,message}`;
  puede requerir ajustar visibilidad.
- **CI**: pasos nuevos de snapshot y Spectral; `clippy --all-targets -- -D warnings` (estándar del
  proyecto) debe seguir en cero.
- **`docs/API.md`** queda **intacto** en este ítem (no se modifica).

## 8. Secuencia propuesta

1. Redactar el change `openspec/changes/openapi/` (proposal + specs + design + tasks) y **esperar
   aprobación explícita**. Nada de código antes.
2. Fijar versiones (`utoipa`/`utoipa-axum`/UI) y migrar `build_router` a `OpenApiRouter` con **una**
   ruta de ejemplo para validar `split_for_parts()`.
3. RED→GREEN→REFACTOR: `ToSchema` en DTOs y errores, `#[utoipa::path]` por handler, seguridad Bearer.
4. Añadir la UI (Scalar por defecto; Swagger UI con `vendored` si se elige).
5. Añadir test de snapshot del `openapi.json` y lint Spectral en CI.
6. `openspec archive openapi` y PR a `development`; release a `main` solo con confirmación.

## 9. Preguntas abiertas

- ¿Qué UI por defecto: **Scalar** (moderna) o **Swagger UI** (familiar, pero con descarga en build)?
- ¿La spec/UI se sirven **siempre** o tras *feature flag* / variable de entorno?
- ¿Se comparten tipos con el servidor MCP (PLAN-002) o se asumen dos sistemas de esquema?
- ¿Se versiona el documento OpenAPI (p. ej. `info.version` ligado al *tag*)?
- ¿`/api-docs/openapi.json` entra en el contrato público (protegido por auth) o es abierto?
- ¿Se generan clientes SDK más adelante a partir de la spec, o queda fuera?

---

## Definition of Done

La misma que `plans/PLAN.md` §7 y `plans/PLAN-001.md` §4: spec aprobada → TDD (RED/GREEN/REFACTOR)
verificado por CLI (`just test`, `just clippy`, `just fmt`) → revisión → `openspec archive
<openapi>` → PR a `development`. La release a `main` **solo** con confirmación explícita del
usuario.

**Requisito previo ineludible**: sin un change aprobado en `openspec/changes/openapi/` **no se
escribe código**. Este documento es planificación, no una propuesta aprobada.

---

## Fuera de alcance vigente

- Reescribir o sustituir `docs/API.md` (permanece intacto en este ítem).
- Cambios de contrato en los endpoints existentes.
- Generación multi-lenguaje de SDKs.
- Documentación de un transporte MCP (eso vive en `plans/PLAN-002.md`).
