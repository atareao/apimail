# Tasks

> **RED ya capturado**: la evidencia de los 4 fallos y del paquete GHCR inexistente (sección 1) se
> recogió antes de implementar; el resto de tareas quedan pendientes de aprobación e implementación.

## 1. RED — Reproducir y documentar el fallo (evidencia)

- [x] 1.1 Confirmar que el workflow `Image` **ha fallado 4 veces seguidas** y que la imagen **nunca
  se ha publicado** (`gh run list --workflow=Image`): `37990970041` (push del merge, 21:02:33),
  `37990994726` (push `chore: release v0.4.0`, 21:02:47), `37990997461` (tag `v0.4.0`, 21:02:47) y el
  **reintento** del último (`attempt=2`, 21:04:23) en **otro runner**. Anotar la evidencia.
  - Evidencia: 4 runs en `failure`; la imagen nunca se publicó.
- [x] 1.2 Capturar el error exacto de Docker Hub: `429 Too Many Requests` al resolver los manifests
  base `docker.io/library/alpine:3.21` (Dockerfile línea 34) y
  `docker.io/library/rust:1.98.1-alpine3.21`:
  `ERROR: failed to build: failed to solve: … unexpected status from HEAD request to
  https://registry-1.docker.io/v2/library/alpine/manifests/3.21: 429 Too Many Requests`. Anotar la
  evidencia.
  - Evidencia: el reintento (`attempt=2`, en **otro** runner, ~90 s después) **repitió** el 429 → **no** es transitorio, es el límite anónimo por IP.
- [x] 1.3 Confirmar que el paquete en GHCR **no existe**:
  `GET /user/packages/container/apimail` → **`404 Package not found`** (`gh api
  /user/packages/container/apimail`). Anotar la evidencia.
  - Evidencia: `404 {"message":"Package not found.","status":"404"}`.
- [x] 1.4 Confirmar que el límite es **por IP** (no global): desde la IP de desarrollo el mismo
  `HEAD` a `registry-1.docker.io` responde **`401`** (auth) con
  `docker-ratelimit-source: 139.47.26.33` y **sin** 429. Anotar la evidencia.
  - Evidencia: `401` + `docker-ratelimit-source: 139.47.26.33`, sin 429.
- [x] 1.5 Verificar el **espejo** `mirror.gcr.io` desde esta máquina, sin autenticación y **más allá del manifest**:
  - `HEAD https://mirror.gcr.io/v2/library/alpine/manifests/3.21` → **HTTP 200**,
    `content-type: application/vnd.oci.image.index.v1+json`,
    `docker-content-digest: sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507`.
  - `HEAD https://mirror.gcr.io/v2/library/rust/manifests/1.98.1-alpine3.21` → **HTTP 200**.
  - **Build real**: `podman build` de un `FROM mirror.gcr.io/library/alpine:3.21` terminó correctamente → el espejo sirve **manifest, config y capas**.
  - **`rust` vía espejo**: `podman manifest inspect mirror.gcr.io/library/rust:1.98.1-alpine3.21` → índice OCI con **6 manifests** y plataformas `linux/amd64`, `linux/arm64`, `linux/ppc64le` → **`linux/amd64` disponible**, la única que compila el workflow.
  - Evidencia: `HEAD` **200** sin auth en ambas, `podman build` correcto y `linux/amd64` presente.
- [x] 1.6 Documentar la **trampa del reintento manual**: el tag `v0.4.0` apunta a `1266e3d`, cuyo
  árbol contiene el `image.yml` **viejo**; `gh run rerun` y `workflow_dispatch --ref v0.4.0` usan ese
  workflow antiguo y **seguirían fallando**. Anotar la evidencia.
  - Evidencia: `git show v0.4.0:.github/workflows/image.yml` es idéntico al actual (sin el arreglo).

## 2. GREEN — Implementar el arreglo (solo `.github/workflows/image.yml`)

- [x] 2.1 **Espejo de registro en buildkit**: en `docker/setup-buildx-action@v4`, añadir
  `buildkitd-config-inline` con `[registry."docker.io"] mirrors = ["mirror.gcr.io"]`. **No** tocar el
  `Dockerfile`.
  - Evidencia: `image.yml` (paso `Set up Docker Buildx`) tiene
    `buildkitd-config-inline: |` → `[registry."docker.io"]` / `mirrors = ["mirror.gcr.io"]`.
    **Corrección de input**: el borrador del contrato usaba el nombre antiguo de v3, que **no existe
    en `setup-buildx-action@v4`** (v4 renombró los inputs deprecados a `buildkitd-config` /
    `buildkitd-config-inline`; ambos verificados en su `action.yml`). Lo cazó **`actionlint`**
    (`exit=1`, señalando el input del borrador como *no definido* en esa action); con
    `buildkitd-config-inline` actionlint da `exit=0`. Con el nombre viejo la action **ignoraría** el
    valor y el espejo **no se aplicaría**. `Dockerfile` intacto:
    `md5sum Dockerfile` = `cdbc362a4228499034d1c17a43592194`.
- [x] 2.2 **`concurrency` por ref**: `group: image-${{ github.ref }}` con `cancel-in-progress: true`.
  Los dos pushes al mismo `main` (merge y commit `chore: release`) colapsan en **1** build; la build
  del **tag** conserva su propio grupo y **ningún push a `main` puede cancelarla** (un grupo global lo
  haría y se perderían los alias `vX.Y.Z`/`X.Y`). Es una **adición deliberada** de este change: Valet
  (la referencia de la casa) **no** tiene `concurrency`.
  - Evidencia: `concurrency` a nivel de workflow, tras `permissions`.
    `python3 -c "import yaml; d=yaml.safe_load(open('.github/workflows/image.yml')); print(d.get('concurrency'))"`
    → `{'group': 'image-${{ github.ref }}', 'cancel-in-progress': True}`.
- [x] 2.3 **Reintentos con espera creciente**: un paso `uses:` **no** admite bucle, así que
  **sustituir** el paso de `docker/build-push-action@v7` por un `run:` que invoque
  `docker buildx build` con **las mismas banderas** (`--platform linux/amd64 --tag apimail:ci --load
  --cache-from type=gha --cache-to type=gha,mode=max .`) en un bucle de **3 intentos** con espera
  creciente (10 s y 30 s, antes del 2.º y 3.er intento respectivamente) y `exit 1` si los 3 fallan.
  `docker/setup-buildx-action@v4` **se mantiene** (crea el builder y lo deja activo). Sin
  dependencias de terceros.
  - Evidencia: el paso `Build image without publishing` es ahora `run:` con `for attempt in 1 2 3`,
    `wait_s=10` y `wait_s=$(( wait_s * 3 ))` (esperas 10 s/30 s) y `exit 1` tras el 3.º fallo,
    invocando `docker buildx build --platform linux/amd64 --tag apimail:ci --load --cache-from
    type=gha --cache-to type=gha,mode=max .` (mismas banderas que la action). `actionlint` → `exit=0`;
    `bash -n` del bloque `run:` extraído → OK.
- [x] 2.4 **Auto-verificación del artefacto publicado**: tras el push, **parar y eliminar antes** el
  contenedor del *smoke test* (`apimail-ci`, que ocupa el puerto 3000), y para **cada tag publicado**
  (`latest`, `sha-<7>` / `vX.Y.Z`, `X.Y` / `sha-<7>`) hacer `docker pull "${IMAGE}:${tag}"` y repetir
  el *health check* contra la imagen **descargada** con las mismas variables ficticias; **fallar** si
  algún tag no se descarga o no responde `status == "ok"`, `name == "apimail"` y la `version` esperada.
  - Evidencia: el workflow tiene un 8.º y último paso, `Verify the published image is pullable and
    healthy`, que (1) libera el puerto con `docker rm -f apimail-ci` **antes** del `docker pull`, (2)
    itera los tags desde `image-tags.txt` (fichero que escribe el paso previo `Tag and publish the
    verified image`, de modo que **no** se duplica la lógica del `case`), (3) hace
    `docker pull "${IMAGE}:${tag}"`, `docker run -d --name apimail-published -p 3000:3000 …` y valida
    `status == "ok"` / `name == "apimail"` / `version == $v` (versión leída de `Cargo.toml`), y (4)
    falla con `exit 1` si algún tag no descarga o no pasa el *health check*. Comprobado con
    `python3 -c "…print([s.get('name') for s in d['jobs']['image']['steps']])"`:
    `['Checkout repository', 'Set up Docker Buildx', 'Build image without publishing', 'Smoke test the
    built image', 'Verify unprivileged user and writable data volume', 'Log in to GitHub Container
    Registry', 'Tag and publish the verified image', 'Verify the published image is pullable and
    healthy']`.
- [x] 2.5 Comprobar que **no** se modifican los triggers ni la semántica de tags, **ni** se añaden
  secretos, **ni** se cambia a multi-arch (`linux/amd64` intacto).
  - Evidencia: `git diff --stat` → solo `.github/workflows/image.yml` (92 ins / 8 del);
    `git diff --name-only Dockerfile compose.yml .github/workflows/ci.yml .github/workflows/release.yml`
    → **vacío**. En el diff se conservan `on:` (push a `main` con `paths-ignore`, tags `v*`,
    `workflow_dispatch`), `permissions`, el `case` de tags (`latest`/`sha-<7>` en `main`;
    `vX.Y.Z`/`X.Y`/`latest` en tag; `sha-<7>` en `workflow_dispatch`) y `--platform linux/amd64`. No
    se añade ningún `secrets.*` nuevo (solo el `secrets.GITHUB_TOKEN` preexistente).

## 3. Verificación estática del YAML

- [x] 3.1 `actionlint` **ya disponible** (`/tmp/opencode/actionlint`, **1.7.12**).
- [x] 3.2 **Baseline ya verde**: `actionlint .github/workflows/image.yml` → `exit=0` con el workflow
  **sin** arreglar → cualquier hallazgo posterior es atribuible a este change.
- [x] 3.3 Pasar `actionlint` sobre el `image.yml` **ya arreglado** y dejar constancia del resultado.
  - Evidencia: `/tmp/opencode/actionlint .github/workflows/image.yml` → **sin hallazgos, `exit=0`**
    (el baseline ya era 0, luego **no hay ningún hallazgo atribuible al cambio**). **`shellcheck` no
    está disponible** en la máquina (`command -v shellcheck` → sin resultado); en su lugar se
    extrajeron los **5** bloques `run:` a `/tmp/opencode/wf-steps/*.sh` y se les pasó `bash -n`,
    todos **OK**.

## 4. Verificación end-to-end

- [x] 4.1 **Dry-run del pipeline arreglado con `workflow_dispatch` sobre la rama** (antes del merge a
  `main`): `gh workflow run image.yml --ref fix/image-publish-resilience`. En ese caso el `case`
  publica **solo** `sha-<7>` y **nunca** mueve `latest`, así que no hay release ni artefacto mutable.
  Debe terminar **verde**: prueba en un solo run el espejo (`buildkitd-config-inline`), los
  reintentos, el smoke test, la verificación de uid 1000, el push y la verificación del publicado.
  - Evidencia: run **38025457014** (`workflow_dispatch` sobre `fix/image-publish-resilience`),
    conclusión **success** y los **8 pasos en verde**. El log de `Set up Docker Buildx` muestra el
    buildkitd aplicado: `"buildkitd.toml": "[registry]\n[registry.'docker.io']\nmirrors = ['mirror.gcr.io']\n"`.
    El build **no** necesitó reintentos: `Build succeeded on attempt 1` (`attempt 1/3`), sin `429`.
    Smoke test: `version 0.4.0`; uid 1000 verificado. El paso nuevo verificó el **publicado**:
    `ghcr.io/atareao/apimail:sha-4a39141` descargado (digest
    `sha256:9e174dfd7c896b5b5ec76c8072c2f6a2a9bcc8c68dfeb70294348b0fa3a5ade5`) y
    «Published image ghcr.io/atareao/apimail:sha-4a39141 is pullable and healthy».
    **Se publicó solo `sha-4a39141`**: `latest` **no** se movió, como documenta el change.
- [ ] 4.2 **El propio merge del arreglo a `main` dispara `Image`** (el diff incluye un `.yml`, que
  **no** está en `paths-ignore`); adjuntar el run disparado.
- [ ] 4.3 El run termina **verde publicando `latest`** (y `sha-<7>`; `vX.Y.Z`/`X.Y` si el merge trae
  tag); adjuntar el run.
- [ ] 4.4 Comprobar que el paso de verificación post-push **descarga cada tag publicado** y repite
  el *health check* sobre la imagen descargada.
- [ ] 4.5 Arrancar la imagen descargada con las variables ficticias y confirmar **`RestartCount 0`**
  y `/api/health` sano.
- [ ] 4.6 **Nota (decisión del usuario)**: los alias `v0.4.0`/`0.4` **no pueden recuperarse nunca**:
  el workflow deriva los alias de la **versión del tag que se empuja** (`minor="${version%.*}"`), así
  que `v0.4.0` solo lo habría producido un run del tag `v0.4.0` con el workflow **viejo** (imposible:
  `gh run rerun` y `workflow_dispatch --ref v0.4.0` usan ese árbol). La única vía de tener alias
  inmutables es una **release nueva** (`v0.4.1`, que publicaría `latest`, `0.4` y `v0.4.1`). El
  `latest` se recupera con el merge del arreglo. **Condición**: si se corta `v0.4.1`, el commit del
  arreglo debe ser **`🐛 fix:`** para que el bump sea **patch** (no `feat`, que sería minor).

## 5. Cierre

- [x] 5.1 Revisión **independiente** del change (agente `general`, adversarial) + `actionlint`;
  `rust-reviewer` **no procede** (el change no toca Rust). La verificación fuerte es el run real
  end-to-end (sección 4).
  - Evidencia: verdicto de la revisión adversarial independiente → **mergeable tal cual**, sin bugs
    bloqueantes ni regresiones. Comprobado en el diff: las **8 líneas borradas** son exactamente el
    `with:` del paso `docker/build-push-action@v7`; el bucle de reintentos **no** tiene falso verde
    (el `if` exime de `set -e`, `exit 0` al éxito, **2** esperas exactas de 10 s y 30 s, `exit 1` tras
    3 fallos); el handoff por `image-tags.txt` es correcto; **no hay ninguna interpolación `${{ … }}`
    dentro de bloques `run:`** (todo va entre comillas vía env de shell); `workflow_dispatch` publica
    solo `sha-<7>` y **nunca** mueve `latest` (confirmado en el run: un único digest empujado);
    `permissions` intacto. Hallazgos de severidad **baja** (sin corrección de código): (a) la
    verificación post-push **no descarga blobs** — documentado en `design.md`; (b) no hay traza de que
    `--cache-to type=gha,mode=max` poblara la caché en ese run, pero las banderas son **idénticas** a
    las del paso anterior y no rompen el build (solo rendimiento). **No verificable** por la revisión
    (y por qué): que el espejo sirviera las base en ese run (el log imprime nombres lógicos, no el
    host resuelto; la prueba del espejo son las mediciones del `design.md`), la cancelación real de
    `concurrency` (no se pueden lanzar dos runs), la visibilidad del paquete por API (`403`, falta
    `read:packages`) y `shellcheck` (no instalado; se usó `bash -n`).
- [x] 5.2 Marcar tareas y `openspec validate image-publish-resilience --strict`.
  - Evidencia: `openspec validate image-publish-resilience --strict` → `Change 'image-publish-resilience'
    is valid` (`exit=0`), con `skip_specs` informado (`skip_specs is set in .openspec.yaml: change
    declares no spec-level behavior changes, zero deltas accepted`); **16/24** tareas marcadas en ese
    momento (las 4.2–4.6 y 5.4 quedan pendientes de la release).
- [x] 5.3 PR del change (`fix/image-publish-resilience` → `development`; la release
  `development` → `main` arrastra el `.yml` y dispara la verificación de 4.2).
  - Evidencia: **PR #36** (`fix/image-publish-resilience` → `development`), con **CI en verde**.
- [ ] 5.4 **Verificación externa definitiva**: con el paquete de GHCR **público**, `podman pull
  ghcr.io/atareao/apimail:latest` **anónimo** desde la máquina de desarrollo (con `docker logout`/
  sin credenciales) → es la prueba de que un tercero puede descargar la imagen, y cierra la
  limitación del paso de verificación (que no ejercita la descarga de blobs). Requiere que el usuario
  cambie la visibilidad del paquete (manual).
