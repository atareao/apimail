# Design

## Context

- **Estado del workflow `Image`** (`.github/workflows/image.yml`, 148 líneas): se dispara en
  `push` a `main` (con `paths-ignore: **.md` y `openspec/**`), en tags `v*` y a mano
  (`workflow_dispatch`). **No** tiene `concurrency` ni reintentos. Secuencia de pasos: checkout →
  `docker/setup-buildx-action@v4` → `docker/build-push-action@v7` (`push: false`, `load: true`,
  `tags: apimail:ci`, `platforms: linux/amd64`, `cache-from/to: type=gha`) → *smoke test* contra
  `/api/health` (asertando `status`/`name`/`version`, versión leída de `Cargo.toml`, nunca cableada)
  → verificación de uid 1000 y `/app/data` escribible → login en `ghcr.io` → tag + push.
- **Semántica de tags publicados**: en push a `main` → `latest` + `sha-<7>`; en tag `vX.Y.Z` →
  `vX.Y.Z`, `X.Y` y `latest`; en `workflow_dispatch` → **solo** `sha-<7>` (nunca mueve `latest`).
- **Causa raíz medida**: al resolver los manifests base, Docker Hub devuelve
  `429 Too Many Requests` desde `registry-1.docker.io`. Afecta a `docker.io/library/alpine:3.21`
  (Dockerfile línea 34) y a `docker.io/library/rust:1.98.1-alpine3.21` (línea 6).
- **No es transitorio**: el reintento `attempt=2` de `37990997461` corrió ~90 s después, en **otro
  runner**, y repitió el 429. La cuota es **anónima y por IP**; los runners de GitHub comparten IP.
- **Espejo verificado empíricamente desde esta máquina, más allá del `HEAD`** (sin autenticación):
  - `HEAD https://mirror.gcr.io/v2/library/alpine/manifests/3.21` → **HTTP 200**,
    `content-type: application/vnd.oci.image.index.v1+json`,
    `docker-content-digest: sha256:ce64758a109eb420d874a118f87920e625e12d3634e03b4a5573fd9f6e5d3507`;
    `HEAD https://mirror.gcr.io/v2/library/rust/manifests/1.98.1-alpine3.21` → **HTTP 200**.
  - **Build real desde el espejo**: `podman build` de un `FROM mirror.gcr.io/library/alpine:3.21`
    (con un `RUN`) **terminó correctamente** → el espejo sirve **manifest, config y capas**.
  - **`rust` vía espejo**: `podman manifest inspect
    mirror.gcr.io/library/rust:1.98.1-alpine3.21` → `mediaType=application/vnd.oci.image.index.v1+json`,
    **6 manifests**, plataformas `linux/amd64`, `linux/arm64`, `linux/ppc64le` → **`linux/amd64` está
    disponible**, que es justo la única plataforma que compila el workflow.
  - **Conclusión**: el espejo no solo resuelve el manifest, **sirve el artefacto completo** para las
    **dos** imágenes base que usa el `Dockerfile` en `linux/amd64`.
- **El límite es por IP, no global**: desde la IP de desarrollo el mismo `HEAD` a
  `registry-1.docker.io` responde **`401`** (auth) con `docker-ratelimit-source: 139.47.26.33` y
  **sin** 429.
- **El paquete GHCR no existe**: `GET /user/packages/container/apimail` → **404 Package not found**.
- **Estado del release**: `v0.4.0` está completo en crates.io, GitHub Releases y docs.rs; el tag
  `v0.4.0` existe apuntando a `1266e3d`, cuyo árbol contiene el **`image.yml` viejo**. Reejecutar el
  tag o `workflow_dispatch --ref v0.4.0` usa ese workflow antiguo y **seguiría fallando**.
- **Herramientas locales**: hay `gh`, `podman`, `skopeo`, `jq`, `curl` y **`actionlint` 1.7.12**
  (`/tmp/opencode/actionlint`); **no** hay `docker` ni `buildctl`.

## Goals / Non-Goals

**Goals**
- Que la publicación de la imagen `ghcr.io/atareao/apimail` deje de fallar por el límite anónimo de
  Docker Hub.
- Que el **mismo ref no se construya dos veces** (los dos pushes a `main` del release colapsan en 1
  build); refs distintos (`main` y el tag del release) sí corren en paralelo, cada uno una sola vez.
- Absorber 429/5xx transitorios con reintentos y espera creciente.
- Verificar **el artefacto publicado**, no solo la imagen local: cada tag debe poder descargarse y
  seguir pasando el *health check*.

**Non-Goals**
- No se cambian los **triggers** (push a `main`, tags `v*`, `workflow_dispatch`) ni la **semántica
  de tags** publicados.
- No se añaden **secretos** (ni cuenta/token de Docker Hub ni ninguna credencial nueva).
- No se pasa a **multi-arch**: se mantiene **solo `linux/amd64`**.
- No se toca el `Dockerfile` ni `compose.yml` ni la aplicación.

## Decisions

1. **Espejo de registro para `docker.io` en buildkit (opción A, elegida)**. Se configura
   `docker/setup-buildx-action@v4` con `buildkitd-config-inline`:
   ```yaml
   buildkitd-config-inline: |
     [registry."docker.io"]
       mirrors = ["mirror.gcr.io"]
   ```
   **Nota (nombre del input)**: el borrador del contrato llamaba a este input con el nombre antiguo
   de v3, pero ese nombre **no existe en `docker/setup-buildx-action@v4`**: v4 renombró los inputs de
   v3 (ya deprecados en esa versión) a **`buildkitd-config`** y **`buildkitd-config-inline`** (los
   inputs reales de v4 son `version, driver, driver-opts, buildkitd-flags, buildkitd-config,
   buildkitd-config-inline, use, name, endpoint, platforms, append, keep-state, cache-binary,
   cleanup`). Lo cazó **`actionlint`** (`exit=1`, señalando el input del borrador como *no definido*
   en esa action), que es justamente para lo que se instaló: con el nombre equivocado la action
   **ignoraría** el valor y el espejo **no se aplicaría**; con `buildkitd-config-inline` el lint da
   `exit=0` y el efecto es el correcto.
   Ventajas: el `Dockerfile` **queda intacto** (los builds locales no cambian), no requiere
   secretos y, si el espejo no tuviera un tag, buildkit **recurre al registro original** (degrada,
   no rompe). Encaja con el `Dockerfile` tal cual porque este usa `docker.io/library/...` sin
   cualificar del lado del cliente: la redirección es del **daemon/buildkit**, no del fichero. El
   espejo **sirve el artefacto completo** (manifest, config y capas) para `alpine:3.21` y
   `rust:1.98.1-alpine3.21` en `linux/amd64` (medido, ver Context).
2. **`concurrency` por ref**: `group: image-${{ github.ref }}` con `cancel-in-progress: true`. Es una
   **adición deliberada** de este change, **no** una convención de la casa: el `image.yml` de Valet
   (la referencia) **no tiene `concurrency`** (verificado: su workflow solo tiene `on:`/`push:` y el
   paso `docker/build-push-action@v7`). Un release dispara 3 eventos (`push` del merge, `push` del
   commit `chore: release vX.Y.Z`, `push` del tag `vX.Y.Z`). Se descarta un **grupo global**
   (`group: image`): un release empuja a `main` dos veces y **después** empuja el tag; con un grupo
   global, un push a `main` posterior **cancelaría una build del tag en vuelo**, y esa build es la
   única que publica los alias `vX.Y.Z` y `X.Y` → se perderían en silencio. Con
   `group: image-${{ github.ref }}`, los **dos** pushes al mismo `main` colapsan en **1** build (el
   segundo cancela al primero) y la build del **tag conserva su propio grupo**, así que **ningún
   push a `main` puede cancelarla**. Se pasa de **3 builds simultáneos a 2** (uno por ref); con el
   espejo, la presión sobre Docker Hub deja de ser el problema y el `concurrency` es solo una
   optimización de redundancia, **no** el arreglo.
3. **Reintentos con espera creciente alrededor del build**, implementados como script porque un paso
   `uses:` **no admite bucle**. Se **sustituye** el paso `uses: docker/build-push-action@v7` por un
   `run:` que invoca `docker buildx build` con **exactamente las mismas banderas** que la action
   (`--platform linux/amd64 --tag apimail:ci --load --cache-from type=gha
   --cache-to type=gha,mode=max .`) dentro de un bucle de **3 intentos con espera creciente**
   (10 s y 30 s, antes del 2.º y 3.er intento respectivamente), y que termina con `exit 1` si los 3
   fallan. `docker/setup-buildx-action@v4`
   **se mantiene**: crea el builder y lo deja como el activo (`--use`), así que `docker buildx build`
   a secas ya usa ese builder. Sin dependencias de terceros (nada de `nick-fields/retry`). No
   sustituye al espejo (ver opción D): absorbe 429/5xx y cortes de red **transitorios** una vez el
   espejo ya evita el límite por IP. *Trade-off*: se pierde el resumen/salida que aporta la action y
   el paso pasa de declarativo a script; a cambio se gana el reintento y **cero dependencias nuevas**.
   *Alternativa reversible*: si se prefiere menos superficie, **mantener la action y renunciar al
   reintento**, ya que el espejo ataca la causa raíz; es una elección que se puede revertir sin tocar
   nada más.
4. **Auto-verificación del artefacto publicado**: añadir un paso **tras** el push que, para **cada
   tag publicado**, ejecute `docker pull "${IMAGE}:${tag}"` y vuelva a correr el *health check*
   contra esa imagen, con las mismas variables ficticias del *smoke test*. Orden del paso: **parar y
   eliminar** el contenedor del *smoke test* (`apimail-ci`) → `docker pull` de cada tag → arrancar la
   imagen → *health check* → limpiar. El `stop`/`rm` es imprescindible porque ambos usan el puerto
   **3000** y, si no, el segundo `docker run` falla por puerto ocupado.

   **Alcance real de la prueba (sin sobreprometer)**: el `pull` corre en el **mismo runner** que
   acaba de construir y empujar esa imagen, así que las capas ya son locales y el log muestra
   `Status: Image is up to date for ghcr.io/atareao/apimail:<tag>`: **no descarga blobs**; lo que
   hace es **resolver el tag contra GHCR y comprobar que el digest servido coincide con el artefacto
   empujado**. Lo que **sí** detecta: que el push **no ocurrió**, que el digest servido **no
   coincide** con el construido, que el **tag no existe** en el registro y que el
   **registro/autenticación fallan**; además, arranca la imagen y comprueba `status == "ok"`,
   `name == "apimail"` y la `version` esperada. Lo que **no** ejercita: la **descarga de blobs desde
   un almacén vacío** (las capas ya están en ese runner). Forzar una descarga real exigiría un
   **runner limpio** — un segundo job con `needs:` —; se documenta como **limitación conocida** y
   **no** se implementa ahora, para no ampliar el alcance aprobado. La prueba **definitiva** de que
   un tercero puede descargarla es un `pull` **anónimo** desde la máquina de desarrollo con el
   paquete de GHCR **público** (hoy es privado por defecto); queda como **seguimiento** (tarea 5.4).

   El workflow **SHALL** fallar si algún tag no se puede resolver o si el digest servido no
   corresponde al artefacto publicado, y **SHALL** fallar si esa imagen no responde
   `status == "ok"`, `name == "apimail"` y la `version` esperada. Es el único punto que **no puede
   comprobarse desde fuera** cuando el paquete GHCR es privado (hoy: `404`), así que la verificación
   tiene que vivir dentro del propio workflow.

## Alternatives Considered

| Opción | Cambia | ¿Secretos? | Verdict |
|---|---|---|---|
| **A. Espejo en buildkit (elegida)** | `image.yml` | no | resuelve el 429; `Dockerfile` limpio; degrada a Docker Hub |
| B. `FROM mirror.gcr.io/...` en el `Dockerfile` | `Dockerfile` (afecta a builds locales) | no | funciona, pero ensucia el Dockerfile y ata los builds locales al espejo |
| C. `docker/login-action` a Docker Hub | `image.yml` + **2 secretos** | **sí** | el límite pasa a ser por cuenta, pero exige cuenta+token que hoy no existen |
| D. Solo reintentos | `image.yml` | no | **insuficiente**: ya falló 2 veces en runners distintos con 90 s de diferencia |

- **Opción B** descartada: mueve el problema al `Dockerfile` y ata **también los builds locales**
  al espejo; un `mirror.gcr.io` caído rompería el desarrollo, mientras que la opción A solo afecta
  a la resolución dentro de CI y degrada al registro original.
- **Opción C** descartada: convertiría el límite en «por cuenta», pero exige **crear una cuenta de
  Docker Hub y gestionar un token** (2 secretos nuevos) que hoy no existen; es más superficie
  operativa y de secretos para el mismo objetivo.
- **Opción D** descartada como solución única: el reintento ya se probó (dos runners, 90 s) y el 429
  se repitió, porque el límite por IP no se agota y desagota con el tiempo de espera de un bucle
  corto. Se conserva como **complemento** de A (Decisión 3), no como reemplazo.

## Risks / Trade-offs

- *Riesgo*: `mirror.gcr.io` podría no tener un tag concreto. *Mitigación*: buildkit **recurre al
  registro original** si el espejo no sirve el manifest; la degradación es a Docker Hub, no a fallo.
- *Trade-off*: `concurrency` con `cancel-in-progress: true` **cancela** builds previos **de la misma
  ref** (`group: image-${{ github.ref }}`). *Aceptación*: es deseable; los dos pushes a `main` del
  release publican el mismo árbol, así que solo interesa el último. El **riesgo de cancelar la build
  del tag** (el único que publica `vX.Y.Z`/`X.Y`) queda **eliminado por el diseño**: el tag vive en un
  grupo distinto y ningún push a `main` puede cancelarlo.
- *Trade-off*: el build pasa de la action declarativa `docker/build-push-action@v7` a un `run:` con
  `docker buildx build`; se pierde el resumen/salida que aporta la action a cambio del reintento y de
  **cero dependencias nuevas**. *Reversible*: se puede volver a la action renunciando al reintento si
  se prefiere (el espejo ya ataca la causa raíz).
- *Riesgo*: los reintentos pueden **alargar** el job si el 429 persiste. *Mitigación*: acotado a 3
  intentos con espera creciente; si el espejo funciona, el caso normal es un solo intento.
- *Riesgo*: la verificación post-push añade tiempo (`docker pull` de cada tag). *Aceptación*: es el
  precio de que `latest` no pueda apuntar a un artefacto no descargable; el número de tags es bajo
  (2 en push a `main`, 3 en tag, 1 en `workflow_dispatch`).
- *Riesgo*: si el paquete GHCR es **privado**, `docker pull` requiere el `GITHUB_TOKEN` ya usado en
  el login (presente en el workflow). *Seguimiento*: la visibilidad del paquete es una tarea manual
  aparte, fuera de alcance.
- *Hallazgo (bajo impacto, **consecuencia de la decisión 2**)*: `latest` y `v0.4.1` **no son el mismo
  digest** (`sha256:258c5e8d…` vs `sha256:fa73b4cf…`) aunque ambos son el commit `45c4355`: el grupo
  de `concurrency` **por `ref`** permite deliberadamente que la build de `main` y la del **tag** corran
  en **paralelo** (dos builds, ~20 s de diferencia) y **gana `latest` el último en empujar**. Las dos
  imágenes **no son reproducibles** (difieren en config, `created` y los **digests de las capas**, que
  embeben mtimes). Impacto **cosmético** (mismo código, ambas `version 0.4.1`) más un build redundante
  y almacenamiento duplicado. Se registra como **tarea 5.5**; si se aborda, va en un **change aparte**.

## Migration Plan

- Aditivo y de bajo riesgo: cambios limitados a `.github/workflows/image.yml`. Sin migración de
  datos ni de la aplicación.
- Rollback: revertir la rama del change.
- **Recuperación de artefactos**: `latest` se recupera con el **merge del arreglo a `main`** (que
  dispara `Image` porque el diff incluye un `.yml`, fuera de `paths-ignore`). Los alias
  `v0.4.0`/`0.4` **no se pueden recuperar nunca**: el workflow deriva el alias de **la versión del tag
  que se empuja** (`minor="${version%.*}"`), así que `v0.4.0` solo podría haberlo producido un run del
  tag `v0.4.0` con el workflow **viejo** (imposible). La única vía de tener alias inmutables es una
  **release nueva** (p. ej. `v0.4.1` → alias `v0.4.1`/`0.4`), decisión que tomará el usuario.

## Verification

- **Dry-run previo, sin liberar nada (la verificación más importante antes del merge)**: `image.yml`
  incluye `workflow_dispatch`, y en ese caso el `case` publica **solo** el tag inmutable `sha-<7>` y
  **nunca** mueve `latest`. Por eso, **antes** del merge a `main`, se puede lanzar
  `gh workflow run image.yml --ref <rama>` para probar el pipeline arreglado **completo** (espejo
  vía `buildkitd-config-inline`, reintentos, *smoke test*, uid 1000, push y verificación del
  publicado) sobre la rama, sin release y sin tocar ningún artefacto mutable. Es la verificación
  **previa** a la de `main`.
- `actionlint` **1.7.12** (`/tmp/opencode/actionlint`) sobre `image.yml`: el **baseline ya está
  verde** (`exit 0` con el workflow sin arreglar), así que cualquier hallazgo nuevo es atribuible al
  change. Es también el lint que **cazó** el nombre de input equivocado del borrador: el input del
  espejo en v4 es **`buildkitd-config-inline`** (v4 renombró los inputs de v3, ya deprecados):
  con el nombre real da `exit=0`.
- **End-to-end (merge a `main`)**: el merge del arreglo a `main` dispara `Image`; el run **SHALL**
  terminar **verde** publicando `latest`, y el contenedor de la imagen descargada **SHALL** quedar
  con `RestartCount 0`.
- El paso de verificación post-push **SHALL** resolver cada tag publicado contra GHCR y comprobar
  que el digest servido corresponde al artefacto empujado, y **SHALL** arrancar esa imagen y
  verificar `status`/`name`/`version`. **Alcance**: como corre en el **mismo runner**, las capas son
  locales (`Status: Image is up to date`) → **no** ejercita la descarga de blobs; forzarla requeriría
  un **runner limpio** (segundo job con `needs:`), que **no** se implementa ahora. La prueba
  definitiva de descarga por un tercero es un `pull` **anónimo** con el paquete GHCR **público**
  (seguimiento, tarea 5.4).
- Comprobación del espejo: `HEAD` a `mirror.gcr.io/v2/library/{alpine,rust}/manifests/...` → **200**,
  **build real** de `alpine:3.21` desde el espejo correcto y `rust:1.98.1-alpine3.21` con `linux/amd64`
  presente (ya medido, ver Context).
