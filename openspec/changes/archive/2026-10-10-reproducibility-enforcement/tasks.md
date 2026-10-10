# Tasks

> **RED ya capturado**: el estado **actual** (los cuatro tags comparten **un único digest**) y la
> **ausencia** de cualquier paso que imponga la reproducibilidad se midieron antes de escribir el diseño
> (sección 1). El resto de tareas quedan pendientes de aprobación e implementación.

## 1. RED — Diagnóstico y medición del estado actual (evidencia)

- [x] 1.1 Confirmar que **hoy la reproducibilidad se cumple**: `latest` = `0.4` = `v0.4.2` = `sha-7290c21`
  comparten **un único digest**, y que el paquete de GHCR es **público**. Anotar la evidencia.
  - Evidencia: consulta **anónima** (`ghcr.io/token` + `…/manifests/<ref>`, cabecera
    `docker-content-digest`) → los cuatro devuelven el **mismo**
    `sha256:12e895cdd260b8763177edcc892b1a82f8c8314f73535c571e55579bdc93a745`.
- [x] 1.2 Confirmar que **no existe** ningún paso que **imponga** la reproducibilidad: si una regresión la
  rompiera, CI seguiría **verde** con `latest` ≠ `vX.Y.Z`. Anotar la evidencia.
  - Evidencia: `grep -n "Enforce\|imagetools\|reference" .github/workflows/image.yml` → **sin
    coincidencias**: hoy no hay paso de enforcement, ni `imagetools inspect`, ni variable `reference`.
- [x] 1.3 Documentar el **hallazgo heredado** que este change aborda. Anotar la evidencia.
  - Evidencia: el `design.md` del change archivado `reproducible-image` (*Risks / Trade-offs*) lo dice
    literalmente: «el workflow **audita** la reproducibilidad, pero **no la impone**» (y describe el
    escenario `latest` ≠ `vX.Y.Z` con CI verde).

## 2. GREEN — Implementar el arreglo (solo `.github/workflows/image.yml`)

- [x] 2.1 **Paso `Enforce reproducibility against the published reference`**, **después del login** y
  **antes** de `Tag and publish`: `short_sha` en shell (sin `${{ }}`); `reference="${IMAGE}:sha-${short_sha}"`;
  `local_id` desde `docker image inspect`; `remote_id` desde `docker buildx imagetools inspect "$reference"
  --raw | jq -r '.config.digest'`, **con un bucle de hasta 3 intentos y 10 s de espera** (~30 s máx) que
  **solo** se recorre mientras el ancla **no existe** (motivo: dar tiempo al **run hermano** del release,
  casi simultáneo, a publicar su `sha-<7>`; en el caso normal la primera lectura ya vale). **Sin referencia
  tras los reintentos** → informar y **`exit 0`**; **difiere** → `::error::` + **`exit 1`**; **coincide** →
  imprimir confirmación y `exit 0`.
  - Evidencia: paso insertado como **8.º** de 10, entre `Log in to GitHub Container Registry` y
    `Tag and publish the verified image` (`python3 … [s.get('name') …]` → orden correcto; **7** bloques
    `run:`). `md5sum .github/workflows/image.yml`: antes `5564f7ccb91382b698622523b6685402` → después
    `ce5a7b704f25e58726740d7aad65b2d0`. `actionlint` → `exit=0`.
- [x] 2.2 **`sha-<7>` también en los eventos de tag**: en el `case` de `Tag and publish`, la rama de tag
  publica además `sha-${short_sha}` (además de `vX.Y.Z`, `X.Y`, `latest`).
  - Evidencia: `grep -n 'tags=(' …` → tag = `("${version}" "${minor#v}" "latest" "sha-${short_sha}")` (ahora
    con el ancla); `workflow_dispatch` sigue `("sha-${short_sha}")` y `main` sigue
    `("latest" "sha-${short_sha}")` **sin cambios**. `README.md` (que **sí** documenta la semántica de tags
    en «Imagen publicada en GHCR») actualizado para reflejar que `sha-<7>` se publica en **todos** los builds.
- [x] 2.3 Comprobar que **no** cambian triggers, `permissions`, `concurrency`, *smoke test*, uid 1000, login,
  reintentos ni verificación post-push (salvo el `sha-<7>` añadido a los eventos de tag).
  - Evidencia: `git diff --stat` → solo `.github/workflows/image.yml` (49 líneas; +47/−2) y `README.md`
    (+5/−3). `git diff --name-only -- Dockerfile compose.yml src tests` → **vacío**; `md5sum Dockerfile
    compose.yml` = `cdbc362a4228499034d1c17a43592194` y `faf5c9b6a5cd31d0494b71331eb6a959` (intactos). El
    diff de `image.yml` solo **añade** el paso y **cambia una línea** del `tags=` de tag: triggers,
    `paths-ignore`, `permissions`, `concurrency`, timestamp, build, smoke test, uid 1000, login y
    verificación post-push sin tocar.

## 3. Verificación estática del YAML

- [x] 3.1 Pasar `actionlint` (`/tmp/opencode/actionlint`, **1.7.12**) sobre el `image.yml` arreglado →
  `exit=0` (el baseline ya estaba verde, luego cualquier hallazgo es de este change).
  - Evidencia: `/tmp/opencode/actionlint .github/workflows/image.yml` → **`exit=0`**, sin hallazgos
    (re-ejecutado **tras** el refinamiento del punto 1).
- [x] 3.2 Extraer los bloques `run:` y pasarles **`bash -n`** (`shellcheck` no está instalado en la máquina;
  se documenta).
  - Evidencia: **`shellcheck` no instalado** (`command -v shellcheck` → sin resultado). Tras el cambio del
    punto 1 hay **7** bloques `run:` (se añadió el del enforcement) y **todos** pasan **`bash -n` → `rc=0`**.

## 4. Verificación empírica

- [x] 4.1 **Positiva (en CI)**: **dos `workflow_dispatch` sobre el MISMO commit** → el **segundo** pasa el
  enforcement e imprime `Reproducibility OK: …`, y el pipeline sigue **verde** (*smoke test*, uid 1000,
  publish, verificación **post-push**). Adjuntar el run y la línea de coincidencia.
  - Evidencia: commit `78003b0`. Dispatch **#1** `38032838386`: el ancla no existía → **reintentos reales**
    (`Reference … is not published yet (attempt 1/3)`, `2/3`) y luego «No published reference … nothing to
    compare» → **verde**. Dispatch **#2** `38033025358`:
    `Reproducibility OK: ghcr.io/atareao/apimail:sha-78003b0 matches the freshly built image
    (sha256:2f40532521ee74300d7987477c26aca3ba46c35b1892e8fbff05e0b873e12e84)` → **verde**. *Smoke test*, uid
    1000, publish y verificación **post-push** = `success` en ambos.
- [x] 4.2 **Negativa (en CI, la importante: ver la alarma sonar)**: en una **rama de usar y tirar** (nunca
  fusionada) apuntar el enforcement a una referencia **conocida y distinta** (p. ej. `reference` forzada a
  `sha-7f20753`, config `sha256:1360236702532ca10a598765b4fc69d8be1405ccfc3fd6edaf5a32508aa532b4`) y
  construir un commit nuevo → el paso **`exit 1`** y el job **rojo**, **sin publicar nada**. Adjuntar el run
  rojo. **Borrar** la rama después (es *scaffolding* de prueba, **no** se fusiona).
  - Evidencia: run **`38033290178`** (rama scratch `scratch/enforcement-negative`, **ya borrada**): el paso
    `Enforce reproducibility…` = **`failure`** con
    `##[error]Reproducibility broken: ghcr.io/atareao/apimail:sha-7f20753 is sha256:1360236702532ca10a598765b4fc69d8be1405ccfc3fd6edaf5a32508aa532b4 but this build produced sha256:ddc56f4a0e01620b8b71542ce25ab5c2458e9d7a9387463d8dc14653c021b0eb`;
    los pasos **`Tag and publish the verified image`** y **`Verify the published image is pullable and
    healthy`** quedaron **`skipped`**; el tag del commit scratch (`manifests/sha-9fc3aba`) → **HTTP 404** ⇒
    **no se publicó nada**. La rama scratch se **borró** (scaffolding, **no** se fusiona).
- [x] 4.3 **Regresión de alcance**: confirmar por `git diff` que solo cambia `image.yml` en lo previsto
  (el paso nuevo + el `sha-<7>` de los tags) y que triggers/`permissions`/`concurrency`/resto de pasos siguen
  igual; `git diff --name-only -- Dockerfile compose.yml src tests` → **vacío**.
  - Evidencia: `git diff --name-only` → solo `.github/workflows/image.yml`, `README.md` y los `.md` del
    change; `git diff --name-only -- Dockerfile compose.yml src tests` → **vacío**; `md5sum Dockerfile
    compose.yml` = `cdbc362a4228499034d1c17a43592194` y `faf5c9b6a5cd31d0494b71331eb6a959` (intactos).

## 5. Cierre

- [x] 5.1 Revisión **independiente** del change (agente `general`, adversarial) + `openspec validate
  reproducibility-enforcement --strict`; `rust-reviewer` **no procede** (el change no toca Rust).
  - Evidencia: verdicto **mergeable tal cual**, **sin defectos bloqueantes**. Comprobado por la revisora: el
    `local_id` **es** el `.config.digest` remoto (misma magnitud → la comparación es correcta); `actionlint=0`;
    `bash -n=0`; y el caso de **cancelación** cubierto **sin estado *torn***. Hallazgos y **destino**: el
    **falso verde del ancla-índice** → **corregido aquí** (**fail-closed**); el **blind spot de compresión** →
    **residual mayor** documentado (endurecimiento por manifest/OCI **pendiente de decisión**); **short-sha de
    28 bits**, «**ambos builds igual de mal**» y divergencia **entre commits** → **residuales documentados**;
    «**o se cancela**» y la **precisión config-vs-manifest** → **documentados**. `openspec validate
    reproducibility-enforcement --strict` → **válido** (`exit=0`).
- [x] 5.2 PR del change (`reproducibility-enforcement` → `development`; la release `development` → `main`
  arrastra el `.yml` y pone el enforcement en producción).
  - Evidencia: **PR #41** (`feat/reproducibility-enforcement` → `development`); el `.yml` viaja a `main` en
    la próxima release, donde el enforcement entra en **producción** y se publica además el ancla `sha-<7>`
    del release.
