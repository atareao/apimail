# Proposal

## Why

`v0.4.2` está publicado y **`latest` = `0.4` = `v0.4.2` = `sha-7290c21` comparten un único digest**
(`sha256:12e895cd…`, medido **anónimamente**): el change archivado `reproducible-image` cerró la brecha.
Pero ese change **audita** la reproducibilidad, **no la impone**: no hay ningún paso que **falle** si
vuelve a romperse.

Y puede romperse por causas **ajenas a las banderas**: alguien quita `rewrite-timestamp`/`SOURCE_DATE_EPOCH`
en una edición, cambia la **versión de BuildKit** o se **mueve un tag base** (el `Dockerfile` referencia
las bases por **tag**, no por digest). Entonces el run de `main` y el del tag volverían a producir **dos
imágenes distintas**: `latest` se sobrescribiría con otro digest, la verificación **post-push** seguiría
**verde** (solo comprueba salud) y CI quedaría **verde mientras `latest` ≠ `vX.Y.Z`** — exactamente el
fallo que `reproducible-image` cerró. Hoy **nada** lo detectaría.

## What Changes

Todo el arreglo vive en **`.github/workflows/image.yml`**; el `Dockerfile`, `compose.yml` y la aplicación
**no se tocan**. Dos puntos:

- **Paso `Enforce reproducibility against the published reference`**, colocado **después del login** (hace
  falta credencial para leer el registro) y **antes de `Tag and publish`** (para **no publicar nada** si
  falla). Compara el **config digest** de la imagen **local** (`apimail:ci`) con el del tag **inmutable por
  commit** `sha-<7>` **ya publicado**, vía
  `docker buildx imagetools inspect "$reference" --raw` + `jq -r '.config.digest'`. Si el tag **no existe**
  (primer build de ese commit) → no hay nada que comparar: lo informa y **sale 0**; pero **antes** de
  concluir eso **reintenta la lectura 3 veces con 10 s** (máx ~30 s, y solo cuando el ancla falta) para dar
  tiempo al **run hermano** del release a publicar su ancla. Si existe y **difiere** → `::error::` con ambos
  digests y **`exit 1`**.
- **`sha-<7>` también en los eventos de tag**: hoy solo lo empujan `main` y `workflow_dispatch`; se añade a
  los **tags** para que **todo** build deje un **ancla inmutable por commit** y el enforcement tenga contra
  qué comparar en más casos. Cambio de semántica **deliberado**; beneficio lateral: se puede fijar
  `ghcr.io/atareao/apimail:sha-<7>` sin depender de `latest`.

## Límites (sin adornos)

- El enforcement **solo compara cuando ya existe una referencia de ese mismo commit**: valida el
  **segundo** build contra el **primero**. Un build **solitario** (el primero de un commit, o un
  `workflow_dispatch` aislado) **no se puede validar**; se dice tal cual. En el flujo de release, el caso
  que importa (**main + tag** sobre el mismo commit) **sí** queda cubierto.
- La comparación es de **config digest** (que incluye `rootfs.diff_ids`, las capas **descomprimidas**); **no**
  cubre la **compresión** de las capas. El digest autoritativo completo es el del **manifest**.
  Simplificación **deliberada** (la compresión la determina BuildKit y es estable); endurecimiento
  **opcional** (no implementado): exportar también a OCI (`--output type=oci,dest=…`) para comparar el
  digest del **manifest**.
- **No** se implementa borrado/rollback de tags ya publicados.

## Non-Goals (explícitos)

- **No** se prueban builds **locales** (`podman`/compose: otro motor; el `Dockerfile` no cambia).
- **No** se elimina el **build redundante** por release (~3 min; sigue igual).
- **No** se fija la versión de **BuildKit** ni los **digests de las bases** (ver límites).
- **No** se implementa el endurecimiento por manifest/OCI ni el borrado de tags.

## Capabilities

### New Capabilities
<!-- Ninguna: cambio de infraestructura de CI, sin comportamiento observable (§ skip_specs). -->

### Modified Capabilities
<!-- Ninguna. -->

## Impact

- Modificado: **solo `.github/workflows/image.yml`**.
- **No se tocan**: `Dockerfile`, `compose.yml`, la aplicación (`src/`) ni ninguna spec.
- **Sin secretos nuevos**: se reutiliza el login existente (`docker/login-action` con
  `secrets.GITHUB_TOKEN`); `imagetools inspect` usa esas credenciales.
- Efecto: una **regresión de reproducibilidad** hace **fallar el run** (antes de publicar) en vez de dejar
  `latest ≠ vX.Y.Z` en silencio con CI verde.
- `skip_specs: true` en el change: `image.yml` no tiene spec asociada.
- **Cambio de semántica reconocido**: se añade `sha-<7>` a los eventos de **tag** (antes solo `main` y
  `workflow_dispatch`): más tags publicados por release (uno más, inmutable).
