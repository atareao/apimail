# Tasks

> **RED ya capturado**: el no-determinismo (mismo commit `45c4355` ⇒ **digests distintos** pese a
> **contenido idéntico**) se midió antes de implementar (sección 1). El change está **aprobado e
> implementado** y su verificación empírica salió **verde**; solo quedan las tareas de **cierre** (§6).

## 1. RED — Baseline medido (evidencia)

- [x] 1.1 Confirmar que **`latest` ≠ `v0.4.1`** pese al mismo commit `45c4355`: dos builds en paralelo
  por `ref` produjeron dos manifests distintos. Anotar la evidencia.
  - Evidencia: run del **tag** `38026924672` → `v0.4.1`/`0.4` =
    `sha256:fa73b4cf7e3640ed3fc9e28a85062d83af17c9491e44a47f8da0faee4867834c`; run de **main**
    `38026923522` → `latest`/`sha-45c4355` =
    `sha256:258c5e8da99faa8dbf8cb28cba749e651ff879a88b640108454ee111d6a3c4b9`.
- [x] 1.2 Demostrar que el **contenido es idéntico**: comparar los dos rootfs y el binario. Anotar la
  evidencia.
  - Evidencia: `diff -r --no-dereference` de los dos rootfs → **sin diferencias**; `/app/apimail`
    idéntico byte a byte (`sha256=0849686e7defc91e7a80ecf436925563…`, **6.741.544 B** en los dos).
- [x] 1.3 Localizar el **único** no-determinismo: comparar entradas y mtimes de ambos árboles. Anotar
  la evidencia.
  - Evidencia: **528** entradas por imagen y **11** con **mtime** distinto — `.`, `./app`,
    `./app/apimail`, `./app/data`, `./etc`, `./etc/group`, `./etc/group-`, `./etc/passwd`,
    `./etc/shadow`, `./home`, `./home/apimail` (pasos `adduser`/`mkdir`+`chown`/`COPY` del `Dockerfile`).
- [x] 1.4 **Prueba decisiva + control del método**: normalizar **solo** las fechas y volver a comparar;
  comprobar que sin normalizar **sí** difieren, y descartar el falso positivo por symlinks. Anotar la
  evidencia.
  - Evidencia: con `tar --sort=name --mtime='@0' …` las dos huellas **coinciden**
    (`4f3694dd5f20c510d689a33be240061def0641a6…`); **sin** normalizar, **difieren**. Un `diff -r`
    inicial dio **305** «diferencias» que eran **falso positivo** por seguir **symlinks absolutos** de
    BusyBox (`--no-dereference` lo limpia). Conclusión: el único no-determinismo son los **mtimes**.
- [x] 1.5 Documentar el **mecanismo** (docs oficiales) y el **patrón**: `SOURCE_DATE_EPOCH` como
  variable de entorno; `rewrite-timestamp` como opción del exportador; cobertura por versión de
  BuildKit. Anotar la evidencia.
  - Evidencia: `SOURCE_DATE_EPOCH` es **variable de entorno** que buildx (**≥0.10**) propaga como
    build arg (la referencia de `docker buildx build` **no** la lista como bandera: menciona
    `provenance` 21×, `cache-to` 14×, `load` 28×). `rewrite-timestamp` es **opción del exportador**
    (`--output type=docker,rewrite-timestamp=true`, por defecto `false`). BuildKit **≥0.11** normaliza
    `created`/`history` y `org.opencontainers.image.created`; **≥0.13** cubre las **fechas de los
    ficheros de las capas**. Patrón oficial de Docker para GitHub Actions:
    `echo "TIMESTAMP=$(git log -1 --pretty=%ct)" >> $GITHUB_ENV` + `env: SOURCE_DATE_EPOCH: ${{ env.TIMESTAMP }}`.

## 2. GREEN — Implementar el arreglo (solo `.github/workflows/image.yml`)

- [x] 2.1 **Paso previo de `SOURCE_DATE_EPOCH`**: añadir, justo **antes** del build, un paso que hace
  `git log -1 --pretty=%ct` y escribe `SOURCE_DATE_EPOCH` **directamente** en `$GITHUB_ENV` (**sin**
  bloque `env:` en el paso de build: el snippet de los docs es un artefacto de
  `docker/build-push-action`). **Fail-closed**: si no hay timestamp, `exit 1`.
  - Evidencia: paso `Pin the build timestamp for reproducible layers` insertado como **3.er** paso,
    entre `Set up Docker Buildx` y `Build image without publishing` (9 pasos en total, en orden
    correcto). `md5sum .github/workflows/image.yml`: antes `34ec6440b43c3dcfabf610510b29474f` →
    después `330a5457af462ea5836c55c8034aaed2`.
- [x] 2.2 **Exportar con `rewrite-timestamp`**: en el paso de build, sustituir la bandera `--load` por
  `--output type=docker,rewrite-timestamp=true`; **sin** bloque `env:`. Conservar
  `--platform linux/amd64`, `--tag apimail:ci` y `--cache-from/to`.
  - Evidencia: `git diff --stat` → `1 file changed, 25 insertions(+), 1 deletion(-)`; la **única**
    línea borrada es la bandera `--load \`, ahora `--output type=docker,rewrite-timestamp=true \`.
    `--platform linux/amd64`, `--tag apimail:ci`, `--cache-from/to` intactos; el bucle (espera 10 s/30 s,
    `::warning::`, `::error::`, `exit 1`) intacto.
- [x] 2.3 **Línea de evidencia**: imprimir el id de la imagen construida para hacer la propiedad
  **auditable** por run.
  - Evidencia: en la rama de éxito del bucle (tras `Build succeeded on attempt ${attempt}`) se imprime
    `Image id: $(docker image inspect --format '{{.Id}}' apimail:ci)`; no cambia el control de flujo.
- [x] 2.4 Comprobar que **no** cambian los triggers, la semántica de tags, `platforms: linux/amd64`
  ni los secretos.
  - Evidencia: `git diff --name-only -- Dockerfile compose.yml src tests` → **vacío**;
    `md5sum Dockerfile compose.yml` = `cdbc362a4228499034d1c17a43592194` y
    `faf5c9b6a5cd31d0494b71331eb6a959`. El diff de `image.yml` solo toca el comentario/bandera del build
    y **añade** un paso: ni triggers, ni `permissions`, ni `concurrency`, ni la lógica de tags, ni
    `platforms` cambian. `actionlint` → `exit=0`.

## 3. Verificación estática del YAML

- [x] 3.1 Pasar `actionlint` (`/tmp/opencode/actionlint`, **1.7.12**) sobre el `image.yml` arreglado →
  `exit=0` (el baseline ya estaba verde, luego cualquier hallazgo es de este change).
  - Evidencia: `/tmp/opencode/actionlint .github/workflows/image.yml` → **`exit=0`**, sin hallazgos.
- [x] 3.2 Extraer los bloques `run:` y pasarles **`bash -n`** (`shellcheck` no está instalado en la
  máquina; se documenta).
  - Evidencia: **`shellcheck` no instalado** (`command -v shellcheck` → sin resultado); los **6**
    bloques `run:` extraídos a `/tmp/opencode/wf2/*.sh` pasan `bash -n` → **todos OK**.

## 4. Verificación empírica

- [x] 4.1 **Prueba fuerte**: dos `workflow_dispatch` sobre el **MISMO commit**; el tag `sha-<7>` debe
  resolver al **mismo digest**. Medirlo de forma **anónima** (`GHCR /v2/…/manifests/<tag>` con token de
  `ghcr.io/token`, cabecera `docker-content-digest`) **antes** del segundo dispatch y **después**; si
  cambia, **falla**. Adjuntar los digests.
  - Evidencia: commit `7f20753` (rama `feat/reproducible-image`), **dos dispatches** → en ambos
    `SOURCE_DATE_EPOCH=1791612217 (commit 7f20753)` y `Image id:` (digest del config) **idéntico**:
    `sha256:1360236702532ca10a598765b4fc69d8be1405ccfc3fd6edaf5a32508aa532b4` (runs `38029626408` y
    `38029820836`, ambos **success**). El digest del tag `sha-7f20753` en GHCR (consulta **anónima**,
    cabecera `docker-content-digest`) es **el mismo antes y después** del 2.º dispatch:
    `sha256:caa6591d7552a5ad58de6c0a0294a34937adcae6075569edd18c79c18bdfbdb7` → el tag **no se movió**;
    si el segundo build hubiese producido otra imagen, GHCR habría servido **otro** digest y la prueba
    habría **fallado**. **Contraste con el RED** (sección 1): sin el arreglo, dos builds del commit
    `45c4355` dieron `sha256:fa73b4cf…` (tag) ≠ `sha256:258c5e8d…` (main).
- [x] 4.2 **Inferencia `latest` ≡ `vX.Y.Z`** (documentada, sin forzar release): como `main` y el tag
  construyen el **mismo commit**, «un commit ⇒ un digest» **implica** que coinciden; **no** se
  re-verifica en una release real dentro de este change (no se fuerza una release para probarlo).
  - Evidencia: el **evento no afecta al build**: en `image.yml` el `case` de `${GITHUB_EVENT_NAME}`
    decide **solo los tags** publicados; **no** hay ningún paso de build dependiente del evento. Y
    `SOURCE_DATE_EPOCH` depende **solo** del commit (`git log -1 --pretty=%ct`): en los dos dispatches
    vale `1791612217` para `7f20753`, con independencia del evento. Por tanto, un push a `main` y un
    push del tag del **mismo** commit construyen la **misma** imagen. *(No se prueba contra una release
    real aquí.)*
- [x] 4.3 Confirmar que **todo lo demás sigue verde**: *smoke test*, verificación de uid 1000, push y
  verificación **post-push** del artefacto publicado.
  - Evidencia: `gh run view <id> --json jobs` en **ambos** runs → **todos** los pasos `success`
    (14 entradas: 10 del job + `Post …` + `Complete job`), incluidos `Pin the build timestamp for
    reproducible layers`, `Build image without publishing`, `Smoke test the built image`,
    `Verify unprivileged user and writable data volume`, `Log in to GitHub Container Registry`,
    `Tag and publish the verified image` y `Verify the published image is pullable and healthy`
    (lista **idéntica** en `38029626408` y `38029820836`).
- [x] 4.4 **(Opcional) verificación local cruzada**: `podman build` con las banderas equivalentes de
  buildah (`--source-date-epoch` / `--rewrite-timestamp`) para comprobar que el **`Dockerfile` no
  necesita cambios** y que dos builds locales coinciden; si la versión no lo soporta, documentarlo como
  **no verificable** por esa vía.
  - Evidencia: **sí** está soportado localmente — `buildah 1.45.1` expone `--rewrite-timestamp` y
    `--source-date-epoch` en `buildah build --help` —, pero **no** se ejecutó un build local completo
    del proyecto (≈5–10 min por build en frío). Motivo: la comparación de rootfs (sección 1) ya descartó
    otro no-determinismo, y la **prueba fuerte se ha hecho en CI**, con el **mismo toolchain** que
    produce el artefacto real. **El `Dockerfile` no necesitó cambios** (verificado **por inspección**:
    nada en él depende de fechas).

## 5. Revisión independiente

- [x] 5.1 Revisión **adversarial** del change (agente `general`) + `actionlint`; `rust-reviewer`
  **no procede** (el change no toca Rust). La verificación fuerte es la prueba empírica (sección 4).
  - Evidencia: revisión adversarial independiente (agente `general`) → **verdicto: mergeable tal
    cual**, **sin defectos bloqueantes**. Comprobado por la revisora: los dos runs fueron **en frío**
    (0 líneas `CACHED`) y coincidieron **config**, **capas** y **manifest**; el `HEALTHCHECK` **se
    preserva** con el exportador `type=docker`; **no** hay interpolación `${{ … }}` en bloques `run:`;
    el bucle de reintentos queda **intacto**; y el **alcance** se respetó (solo `image.yml`). Hallazgos
    **no bloqueantes**, ya incorporados aquí: (i) el workflow **audita pero no impone** la
    reproducibilidad → **seguimiento**; (ii) las **bases por tag** son mutables → limitación
    reconocida; (iii) precisión de la **anotación OCI** (schema2 sin anotaciones) y del **alcance del
    `Image id`**. **Dos observaciones no aceptadas**, con motivo: (1) «el `Image id` no discrimina los
    mtimes» → **medida en contra**: los `diff_ids` del config **sí** difieren entre los dos builds
    históricos (`5f023324…` vs `f21e7fc2…`), luego el id sí los distingue; (2) «código antes de la
    aprobación» → parte de un estado del fichero **ya superado**: el código se escribió **después**
    del «adelante» del usuario; queda aclarado.

## 6. Cierre

- [ ] 6.1 Marcar tareas y `openspec validate reproducible-image --strict`.
- [ ] 6.2 PR del change (`reproducible-image` → `development`; la release `development` → `main`
  arrastra el `.yml` y dispara la verificación empírica en CI).
