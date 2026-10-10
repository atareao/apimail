# Design

## Context

- **Estado del workflow `Image`** (`.github/workflows/image.yml`), tras el change archivado
  `image-publish-resilience`: `concurrency` por `ref` (`group: image-${{ github.ref }}`), buildx con
  espejo de `docker.io` (`buildkitd-config-inline` → `mirror.gcr.io`), el build como `run:` con **3
  reintentos** (`docker buildx build --platform linux/amd64 --tag apimail:ci --load --cache-from
  type=gha --cache-to type=gha,mode=max .`), *smoke test* + verificación de uid 1000, login en
  `ghcr.io`, tag+push y verificación **post-push** del artefacto publicado.
- **Semántica de tags** (sin cambios): `main` → `latest` + `sha-<7>`; tag `vX.Y.Z` → `vX.Y.Z`, `X.Y`,
  `latest`; `workflow_dispatch` → **solo** `sha-<7>` (nunca mueve `latest`).
- **Síntoma medido (release `v0.4.1`)**: dos builds del mismo commit `45c4355`, en paralelo por `ref`:
  el run del **tag** `38026924672` → `v0.4.1`/`0.4` = `sha256:fa73b4cf…`; el run de **main**
  `38026923522` → `latest`/`sha-45c4355` = `sha256:258c5e8d…`. Es decir, **`latest` ≠ `v0.4.1`** con
  el **mismo** código.
- **El contenido es idéntico**: `diff -r --no-dereference` de los dos rootfs → **sin diferencias**;
  `/app/apimail` idéntico byte a byte (`sha256=0849686e7defc91e7a80ecf436925563…`, **6.741.544 B**).
- **El único no-determinismo son los mtimes**: **528** entradas por imagen, **11** con mtime distinto
  (`.`, `./app`, `./app/apimail`, `./app/data`, `./etc`, `./etc/group`, `./etc/group-`, `./etc/passwd`,
  `./etc/shadow`, `./home`, `./home/apimail`). Normalizando **solo** fechas (`tar --sort=name
  --mtime='@0' …`) las dos huellas **coinciden** (`4f3694dd5f20c510d689a33be240061def0641a6…`); **sin**
  normalizar, **difieren** (control del método). Un `diff -r` inicial dio 305 «diferencias» que eran un
  **falso positivo** por seguir symlinks absolutos de BusyBox; con `--no-dereference` es limpio.
- **Mecanismo disponible (docs oficiales)**: `SOURCE_DATE_EPOCH` es una **variable de entorno** que
  buildx (**≥0.10**) propaga como build arg — la referencia de `docker buildx build` **no** la lista
  como bandera (control de parseo: la página menciona `provenance` 21×, `cache-to` 14×, `load` 28×).
  `rewrite-timestamp` es una **opción del exportador** (`--output type=docker,rewrite-timestamp=true`,
  por defecto `false`). BuildKit **≥0.11** normaliza `created`/`history` y la anotación
  `org.opencontainers.image.created`; **≥0.13** cubre **además las fechas de los ficheros de las
  capas** —justo lo que falta—.
- **Patrón oficial (referencia)** (docs.docker.com/build/ci/github-actions/reproducible-builds/): paso
  `Get Git commit timestamps` → `echo "TIMESTAMP=$(git log -1 --pretty=%ct)" >> $GITHUB_ENV`, y el
  build con `env: SOURCE_DATE_EPOCH: ${{ env.TIMESTAMP }}`. **Desviación** (ver Decisión 1): ese
  bloque `env:` es un artefacto de `docker/build-push-action`; con un paso `run:` se escribe
  `SOURCE_DATE_EPOCH` **directamente** en `$GITHUB_ENV` (**sin** bloque `env:`), y el paso es
  **fail-closed** si no hay timestamp.
- **Herramientas locales**: hay `podman`, `skopeo`, `jq`, `curl`, `gh` y **`actionlint` 1.7.12**
  (`/tmp/opencode/actionlint`).

## Goals / Non-Goals

**Goals**
- Que el **mismo commit** produzca **el mismo digest** en CI (build determinista frente a mtimes).
- Que `latest` y los alias inmutables (`vX.Y.Z`, `X.Y`) del **mismo commit** dejen de divergir.
- Que la propiedad quede **auditable** en cada run (una línea con el id de la imagen construida).

**Non-Goals**
- No se hacen reproducibles los builds **locales** (`podman compose up --build`: motor buildah y
  compose no pasa estas banderas).
- No se elimina el **build redundante** por release (~3 min; ahora inofensivo).
- No se fija la **versión de BuildKit** ni `compatibility-version` (la asamblea afecta al digest
  entre versiones).
- No se cambian los **triggers**, la **semántica de tags**, `platforms: linux/amd64` ni los secretos.

## Decisions

1. **`SOURCE_DATE_EPOCH` = timestamp del commit `HEAD`** (paso **previo** al build). El paso hace
   `git log -1 --pretty=%ct` y escribe el valor **directamente** en `$GITHUB_ENV`:
   ```yaml
   - name: Pin the build timestamp for reproducible layers
     run: |
       set -euo pipefail
       epoch="$(git log -1 --pretty=%ct)"
       if [ -z "${epoch}" ]; then
         echo "::error::no se pudo leer el timestamp del commit"; exit 1
       fi
       echo "SOURCE_DATE_EPOCH=${epoch}" >> "$GITHUB_ENV"
   ```
   Así `SOURCE_DATE_EPOCH` queda en el **entorno de los pasos siguientes**, que es donde buildx lo lee
   para propagarlo como build arg. Fija `created`/`history` y la anotación
   `org.opencontainers.image.created` (BuildKit ≥0.11), pero **por sí solo** —con el exportador
   `type=docker`— **no** toca las fechas de los ficheros de las capas; de eso se encarga la decisión 2.
   **Desviación deliberada del patrón oficial** (ver Context): los docs de Docker proponen una
   variable intermedia `TIMESTAMP` más `env: SOURCE_DATE_EPOCH: ${{ env.TIMESTAMP }}` en el paso de
   build, pero ese snippet es un **artefacto de `docker/build-push-action`**. Aquí el build es un paso
   `run:`, así que escribir la variable **directamente** en `$GITHUB_ENV` deja el valor en el entorno
   del propio proceso donde buildx lo lee: **una pieza móvil menos** y el mismo efecto. Por eso **no**
   se añade bloque `env:` al paso de build. El paso es además **fail-closed**: si
   `git log -1 --pretty=%ct` no devuelve nada, falla (`exit 1`) en vez de construir un artefacto **no**
   reproducible en silencio.
2. **Exportar con `rewrite-timestamp=true`**: sustituir `--load` por
   `--output type=docker,rewrite-timestamp=true`. `--load` **es exactamente** el exportador
   `type=docker`; la opción `rewrite-timestamp` (por defecto `false`) es la que normaliza las **fechas
   de las capas** (BuildKit ≥0.13). Se conservan `--platform linux/amd64`, `--tag apimail:ci` y
   `--cache-from/to`. La decisión 1 **sin** la 2 no bastaría; la 2 **sin** la 1 normalizaría a un
   epoch fijo en vez de al timestamp del commit.
3. **Una línea de evidencia por run**: tras el build, imprimir el id (y tamaño) de la imagen:
   `docker image inspect --format '{{.Id}}' apimail:ci`. **No es lógica, es trazabilidad**: deja el id
   en el log para poder comparar runs sin cambiar la semántica del workflow.

## Alternatives Considered

| Opción | Cambia | Reproducible | Verdict |
|---|---|---|---|
| **A. `SOURCE_DATE_EPOCH` + `rewrite-timestamp` (elegida)** | `image.yml` | sí | cierra el no-determinismo sin tocar el `Dockerfile` |
| B. Grupo `concurrency` **global** con `cancel-in-progress: false` | `image.yml` | **no** | ordena la carrera, pero siguen existiendo **dos imágenes** y el resultado depende del orden |
| C. No disparar `Image` en el push del commit de release | `image.yml` | **no** | abre una ventana en la que `latest` apunta a la **versión anterior** |
| D. Normalizar en el `Dockerfile` (`ARG SOURCE_DATE_EPOCH` + `find … -exec touch`) | `Dockerfile` (+ build local) | sí | toca el `Dockerfile` y el build local, y duplica una capacidad del motor |

- **B descartada**: no reduce a **una** imagen; solo hace determinista *quién* gana `latest`. Persisten
  dos digests distintos y el resultado sigue dependiendo del orden de llegada.
- **C descartada**: retrasar el push del commit de release deja `latest` apuntando a la release
  **anterior** durante esa ventana, que es justo el fallo que se quiere evitar.
- **D descartada**: obliga a tocar el `Dockerfile` y afecta **también** al build local, cuando
  `rewrite-timestamp` es una capacidad del **exportador** y no requiere cambios en el fichero.

## Risks / Trade-offs

- *Riesgo*: `SOURCE_DATE_EPOCH` **sin** `rewrite-timestamp` no bastaría (las capas conservan mtimes).
  *Mitigación*: se usan **ambos** (decisiones 1 y 2); es exactamente lo que mide el baseline.
- *Trade-off*: el digest pasa a depender del **timestamp del commit** (`git log -1 --pretty=%ct`), no
  del contenido del árbol. Dos commits con el **mismo árbol** y distinta fecha darán distinto digest.
  *Aceptación*: es lo que se pide («mismo **commit** ⇒ mismo digest»), y es el patrón oficial.
- *Riesgo*: la reproducibilidad puede romperse si cambia la **versión de BuildKit** o su
  `compatibility-version`. *Seguimiento*: fijarla queda **fuera de alcance**; se documenta.
- *Trade-off*: `--output` en lugar de `--load` cambia la forma de cargar la imagen en el daemon; el
  efecto observable es el mismo (imagen `apimail:ci` local) más la normalización de fechas.
- *Riesgo*: los builds **locales** (`podman`/`compose`) siguen **no** siendo reproducibles.
  *Aceptación*: non-goal explícito (otro motor; compose no pasa estas banderas).

## Migration Plan

- Aditivo y de bajo riesgo: cambios limitados a `.github/workflows/image.yml`. Sin migración de datos
  ni de la aplicación.
- Rollback: revertir la rama del change (volver a `--load`, sin `SOURCE_DATE_EPOCH` ni
  `rewrite-timestamp`).
- Los artefactos **ya publicados no se reescriben**: la reproducibilidad aplica a los builds
  **futuros**; `v0.4.1`/`latest` actuales conservan sus digests.

## Verification

- **Prueba empírica (la fuerte)**: **dos `workflow_dispatch` sobre el MISMO commit**; el tag `sha-<7>`
  **SHALL** resolver al **mismo digest** tras ambos. Medición **anónima**:
  `GET https://ghcr.io/v2/atareao/apimail/manifests/<tag>` con un token de
  `https://ghcr.io/token?scope=repository:atareao/apimail:pull&service=ghcr.io`, leyendo la cabecera
  `docker-content-digest` **antes** del segundo dispatch y **después**; si cambia, el segundo build
  produjo **otra** imagen ⇒ **falla**.
- **Inferencia honesta y explícita**: el run de `main` y el del **tag** construyen **el mismo
  commit**, así que probar «un commit ⇒ un digest» **implica** que sus salidas **coinciden** ⇒
  **`latest` ≡ `v0.4.1`**. Pero eso **no se podrá re-verificar hasta la próxima release** (no se
  fuerza una release para probarlo); se dice tal cual.
- Todo lo demás **SHALL** seguir verde: *smoke test*, verificación de uid 1000, push y verificación
  **post-push** del artefacto publicado.
- **Verificación local cruzada** (opcional, barata y disponible): `podman build` con las banderas
  equivalentes de buildah (`--source-date-epoch` / `--rewrite-timestamp`) para comprobar que el
  **`Dockerfile` no necesita cambios** y que dos builds locales coinciden. Si la versión instalada no
  lo soporta, se documenta como **no verificable** por esa vía.
