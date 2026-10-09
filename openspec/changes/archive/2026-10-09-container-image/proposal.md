# Proposal

## Why

apimail se distribuye hoy de dos formas —binario en GitHub Releases y crate en crates.io— pero
**no hay ninguna forma de ejecutarlo contenedorizado**. La brecha arrastra también la documentación
y el tooling:

1. **Sin imagen de contenedor**: las releases publican binarios para `x86_64-unknown-linux-gnu` y
   `aarch64-unknown-linux-gnu`, y crates.io sirve el crate `0.3.2`, pero no existe `Dockerfile`,
   ni `compose.yml`, ni imagen publicada en GHCR. Quien quiera desplegar apimail en un servidor
   tiene que compilarlo y montarle el *unit* de systemd a mano.
2. **README mudo sobre despliegue**: el `README.md` tiene **923 líneas**, documenta **18** rutas y
   unas **22** variables de entorno, y **no menciona ni una sola vez** Docker, Podman, compose ni
   despliegue (`rg -i 'docker|compose|podman|deploy|ghcr' README.md` → **0** resultados).
3. **Recetas de contenedor rotas**: el `.justfile` se copió de otro proyecto del mismo autor
   («Valet») y sus recetas de contenedor no aplican a apimail:
   - `deploy` descarga y verifica **`ghcr.io/atareao/valet-ai`**, que es la imagen de **otro**
     proyecto.
   - `_verify-health` exige en el JSON de salud `"db": "connected"`, pero `/api/health` de apimail
     devuelve `{"status","name","version"}` y **no tiene campo `db`**: la receta **falla siempre**.
   - `dev`, `dev-docker`, `push` y `help` nombran «Valet», y `push` publica en
     `docker.io/atareao/apimail` mientras la casa publica en GHCR.
   - `AGENTS.md` ya reconoce esta deuda («No las ejecutes hasta que se cree esa
     infraestructura»).
4. **Riesgo de versionar secretos**: `.gitignore` contiene **solo** `/target`, así que un `.env`
   con las credenciales de correo o la API key podría acabar *commiteado*.

## What Changes

- **`Dockerfile` multi-stage** al estilo de la casa: builder `rust:1.98.1-alpine3.21` (musl) +
  runtime `alpine:3.21`. El runtime va **sin** `ca-certificates`: la pila TLS es 100 % rustls con
  `webpki-roots`, que lleva las raíces empaquetadas en el binario y no consulta el almacén del
  sistema, así que instalarlo sería peso muerto. Usuario no root (uid 1000) y volumen con nombre
  montado en `/app/data`.
- **`.dockerignore`** que deja fuera del contexto, entre otros, `target/`, `.git/`, `.github/`,
  `openspec/`, `plans/`, `.opencode/`, `*.md` y **`.env*`**.
- **`compose.yml`** con el servicio `apimail` (`image: ghcr.io/atareao/apimail:latest` y `build:`),
  publicación de puerto vía `APIMAIL_PUBLISHED_PORT`, volumen **con nombre** en `/app/data`,
  `environment` con **todas** las variables de apimail y sus valores por defecto, y `healthcheck`
  sobre `/api/health`.
- **`.env.j2`** (plantilla Jinja2 con `{{ apimail_* }}`, comentada en español) y **`.env.example`**
  (equivalentes con valores vacíos). **`.gitignore`** gana `.env` y `.env.local` para que los
  secretos no se versionen.
- **`.github/workflows/image.yml`**: en push a `main`, en tags `v*` y a mano
  (`workflow_dispatch`). El workflow **SHALL** construir la imagen **sin** publicar, ejecutar un
  *smoke test* contra `/api/health` y **solo entonces** etiquetar y publicar en GHCR (`latest`,
  `sha-<7>` y, en tags, `vX.Y.Z`, `X.Y`, `latest`). **Solo `linux/amd64`**, como el resto de la casa.
- **`.justfile`**: se corrigen **solo** las recetas de contenedor y salud (`dev`, `dev-docker`,
  `push`, `build`, `deploy`, `deploy-local`, `health`, `_verify-health` y el texto de `help`). Las
  `frontend-*` y `check-all` quedan **intactas** (otra deuda, fuera de alcance).
- **`README.md`**: sección de despliegue con contenedor (construir, `compose up`, variables
  obligatorias, volumen, salud, tirar de GHCR y recetas de `just`), sin afirmaciones volátiles.

Sin cambios de comportamiento, de API ni de configuración **del crate**: solo infraestructura y
documentación. No hay **BREAKING**.

## Capabilities

### New Capabilities
<!-- Ninguna: cambio de infraestructura de distribución, sin comportamiento observable (§ skip_specs). -->

### Modified Capabilities
<!-- Ninguna. -->

## Impact

- Nuevos: `Dockerfile`, `.dockerignore`, `compose.yml`, `.env.j2`, `.env.example` y
  `.github/workflows/image.yml`.
- Modificados: `.gitignore` (añadir `.env`), `.justfile` (recetas de contenedor y salud) y
  `README.md` (sección de despliegue).
- Efecto: apimail se puede ejecutar en un contenedor con un comando, con la cola del webhook
  persistente **por defecto**, sin secretos versionados y con una receta de salud que **sí** refleja
  el contrato real de `/api/health`.
- `skip_specs: true` en el change: no cambia ninguna spec.
- Se publica como **minor** (`v0.4.0`): es una capacidad nueva y consumible (permite desplegar
  apimail en un contenedor) y, según la convención del repo (`✨ feat`), eso dispara un bump
  **minor**, no un *patch*.
