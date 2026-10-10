# Tasks

> **RED ya capturado**: el no-determinismo (mismo commit `45c4355` ⇒ **digests distintos** pese a
> **contenido idéntico**) se midió antes de implementar (sección 1); el resto de tareas quedan
> pendientes de aprobación e implementación.

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

- [ ] 3.1 Pasar `actionlint` (`/tmp/opencode/actionlint`, **1.7.12**) sobre el `image.yml` arreglado →
  `exit=0` (el baseline ya estaba verde, luego cualquier hallazgo es de este change).
- [ ] 3.2 Extraer los bloques `run:` y pasarles **`bash -n`** (`shellcheck` no está instalado en la
  máquina; se documenta).

## 4. Verificación empírica

- [ ] 4.1 **Prueba fuerte**: dos `workflow_dispatch` sobre el **MISMO commit**; el tag `sha-<7>` debe
  resolver al **mismo digest**. Medirlo de forma **anónima** (`GHCR /v2/…/manifests/<tag>` con token de
  `ghcr.io/token`, cabecera `docker-content-digest`) **antes** del segundo dispatch y **después**; si
  cambia, **falla**. Adjuntar los digests.
- [ ] 4.2 **Inferencia `latest` ≡ `vX.Y.Z`** (documentar, sin forzar release): como `main` y el tag
  construyen el **mismo commit**, «un commit ⇒ un digest» implica que coinciden; **no** se re-verifica
  hasta la próxima release.
- [ ] 4.3 Confirmar que **todo lo demás sigue verde**: *smoke test*, verificación de uid 1000, push y
  verificación **post-push** del artefacto publicado.
- [ ] 4.4 **(Opcional) verificación local cruzada**: `podman build` con las banderas equivalentes de
  buildah (`--source-date-epoch` / `--rewrite-timestamp`) para comprobar que el **`Dockerfile` no
  necesita cambios** y que dos builds locales coinciden; si la versión no lo soporta, documentarlo como
  **no verificable** por esa vía.

## 5. Revisión independiente

- [ ] 5.1 Revisión **adversarial** del change (agente `general`) + `actionlint`; `rust-reviewer`
  **no procede** (el change no toca Rust). La verificación fuerte es la prueba empírica (sección 4).

## 6. Cierre

- [ ] 6.1 Marcar tareas y `openspec validate reproducible-image --strict`.
- [ ] 6.2 PR del change (`reproducible-image` → `development`; la release `development` → `main`
  arrastra el `.yml` y dispara la verificación empírica en CI).
