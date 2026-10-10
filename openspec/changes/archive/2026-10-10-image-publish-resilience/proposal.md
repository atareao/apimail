# Proposal

## Why

El release `v0.4.0` está **completo en crates.io, GitHub Releases y docs.rs**, pero la **imagen de
contenedor nunca se ha publicado**: el workflow `Image` ha fallado **4 veces seguidas** y el paquete
de GHCR **no existe**. La causa no es un fallo transitorio, sino el **límite anónimo de Docker Hub
por IP** amplificado por cómo se dispara el workflow:

1. **429 de Docker Hub al resolver los manifests base**: la resolución de
   `docker.io/library/alpine:3.21` (Dockerfile línea 34) y de
   `docker.io/library/rust:1.98.1-alpine3.21` devuelve
   `ERROR: failed to build: failed to solve: … unexpected status from HEAD request to
   https://registry-1.docker.io/v2/library/alpine/manifests/3.21: 429 Too Many Requests`.
   Los runners de GitHub **comparten IP** y agotan la cuota anónima del registro.
2. **Cuatro ejecuciones fallidas, incluido un reintento en otro runner**:
   `37990970041` (push del merge, 21:02:33), `37990994726` (push `chore: release v0.4.0`, 21:02:47),
   `37990997461` (tag `v0.4.0`, 21:02:47) y el **reintento** del último (`attempt=2`, 21:04:23)
   ejecutado en **otro runner** con el **mismo 429**. Que el reintento con ~90 s de diferencia y en
   otra máquina repita el 429 demuestra que **no es transitorio**: es un límite por IP.
3. **El paquete en GHCR no existe**: `GET /user/packages/container/apimail` responde
   **`404 Package not found`**. Es el único punto del release que quedó pendiente.
4. **Amplificador**: `image.yml` **no tiene `concurrency`**. Un release dispara **3 builds
   idénticos en paralelo** (push del merge → push del commit `chore: release` → push del tag), con lo
   que se triplica la presión anónima contra Docker Hub justo en el momento de publicar.
5. **Confirmación del diagnóstico**: desde la IP de desarrollo el mismo `HEAD` responde **`401`**
   (auth) con cabecera `docker-ratelimit-source: 139.47.26.33` y **sin** 429, luego el límite es
   **por IP**, no global.
6. **Trampa del reintento manual**: el tag `v0.4.0` apunta a `1266e3d`, cuyo árbol contiene el
   `image.yml` **viejo** (sin este arreglo). Por eso `gh run rerun` y
   `workflow_dispatch --ref v0.4.0` siguen usando el workflow antiguo y **volverían a fallar**.

## What Changes

Todo el arreglo vive en **`.github/workflows/image.yml`**; el `Dockerfile`, `compose.yml` y la
aplicación **no se tocan**. Cuatro puntos:

- **Espejo de registro para `docker.io`**: configurar buildx con `buildkitd-config-inline` en
  `docker/setup-buildx-action@v4` para que resuelva los manifests base a través de
  `mirror.gcr.io`. El `Dockerfile` **queda intacto**; si el espejo no tuviera un tag, buildkit
  recurre al registro original (degrada, no rompe). Verificado además que el espejo **sirve el
  artefacto completo** (manifest, config y capas) para las dos imágenes base en `linux/amd64`, no solo
  el manifest.
- **`concurrency` por ref** (`group: image-${{ github.ref }}`, `cancel-in-progress: true`): los dos
  pushes a `main` del release colapsan en **1** build y la build del **tag conserva su grupo**, de
  modo que ningún push a `main` puede cancelarla. Es una **adición deliberada** de este change
  (Valet, la referencia, **no** la tiene), no una convención de la casa.
- **Reintentos con espera creciente** alrededor del build: se **sustituye** el paso
  `docker/build-push-action@v7` por un `run:` con `docker buildx build` (mismas banderas) dentro de un
  bucle de **3 intentos** (10 s y 30 s, antes del 2.º y 3.er intento respectivamente), porque un
  `uses:` **no admite bucle**; sin
  dependencias nuevas. Absorbe 429/5xx transitorios y cortes de red de Docker Hub.
- **Auto-verificación del artefacto publicado**: hoy el *smoke test* valida solo la imagen **local**
  (`apimail:ci`) antes de publicar; nadie comprueba lo que queda en GHCR. Tras el push, cada tag
  publicado **SHALL** descargarse (`docker pull`) y la imagen descargada **SHALL** seguir pasando el
  *health check* (parando antes el contenedor del *smoke test*, que ocupa el puerto 3000).

## Capabilities

### New Capabilities
<!-- Ninguna: cambio de infraestructura de CI, sin comportamiento observable (§ skip_specs). -->

### Modified Capabilities
<!-- Ninguna. -->

## Impact

- Modificado: **solo `.github/workflows/image.yml`**.
- **No se tocan**: `Dockerfile`, `compose.yml`, la aplicación (`src/`) ni ninguna spec.
- **Sin secretos nuevos**: el espejo de buildkit no requiere autenticación (verificado: los `HEAD`
  a `mirror.gcr.io` responden **200** sin auth) y no se añaden credenciales de Docker Hub.
- Efecto: la imagen se publica en `ghcr.io/atareao/apimail` de forma resiliente (espejo + 1 build
  por release + reintentos) y **verificada tras el push**, cerrando el hueco de que `latest`
  apuntara a un artefacto no descargable.
- `skip_specs: true` en el change: `image.yml` no tiene spec asociada.
- Cierre: el **merge del arreglo a `main`** dispara `Image` (el diff incluye un `.yml`, que **no**
  está en `paths-ignore`) y recupera `latest`. Los alias `v0.4.0`/`0.4` **no se pueden recuperar
  nunca** (el workflow deriva el alias de la versión del tag empujado); la única vía de tener alias
  inmutables es una **release nueva** (`v0.4.1`), decisión que tomará el usuario.
