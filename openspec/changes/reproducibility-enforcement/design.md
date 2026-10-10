# Design

## Context

- **Workflow `Image`** (`.github/workflows/image.yml`), tras `image-publish-resilience` y
  `reproducible-image`. Orden de pasos: checkout → `docker/setup-buildx-action@v4` (espejo
  `mirror.gcr.io` vía `buildkitd-config-inline`) → **`Pin the build timestamp for reproducible layers`**
  (`SOURCE_DATE_EPOCH` = `git log -1 --pretty=%ct`, **fail-closed**) → **`Build image without publishing`**
  (`docker buildx build --platform linux/amd64 --tag apimail:ci --output type=docker,rewrite-timestamp=true
  --cache-from/to type=gha`, **3 reintentos**, imprime `Image id:`) → *smoke test* → verificación de **uid
  1000** → **`docker/login-action@v4`** (`ghcr.io`) → **`Tag and publish the verified image`** (escribe
  `image-tags.txt`, etiqueta y empuja) → **`Verify the published image is pullable and healthy`**.
- **Semántica de tags hoy**: `main` → `latest` + `sha-<7>`; tag `vX.Y.Z` → `vX.Y.Z`, `X.Y`, `latest`;
  `workflow_dispatch` → **solo** `sha-<7>` (nunca mueve `latest`).
- **Estado verificado (medido anónimamente)**: `latest` = `0.4` = `v0.4.2` = `sha-7290c21` comparten **un
  único digest** `sha256:12e895cdd260b8763177edcc892b1a82f8c8314f73535c571e55579bdc93a745`. El paquete de
  GHCR es **público**.
- **Hallazgo heredado** (change archivado `reproducible-image`, *Risks / Trade-offs*): «el workflow
  **audita** la reproducibilidad, pero **no la impone**» — no hay ningún paso que **falle** si los digests
  divergen. Si un cambio futuro la rompiera, `latest` se sobrescribiría con otro digest, la verificación
  **post-push** seguiría **verde** (solo comprueba salud) y CI quedaría **verde mientras `latest` ≠
  `vX.Y.Z`**. Este change cierra ese hallazgo.
- **Herramientas disponibles en el runner**: `docker buildx` (con `buildx imagetools`) y `jq` (el workflow
  ya usa `jq` en el *smoke test*). No se añade ninguna dependencia.
- **`docker buildx imagetools inspect --raw`** devuelve el **manifest** del tag remoto y usa las
  credenciales del `login` previo; `.config.digest` de ese manifest es el digest del **config**, que
  coincide con el `Image id` de la imagen local.
- **Nota de alcance**: `docker image inspect --format '{{.Id}}' apimail:ci` imprime el digest del **config**
  (`sha256:…`), no el del manifest (ver *Decisión 1* y *Límites*).

## Goals / Non-Goals

**Goals**
- Que una **regresión de reproducibilidad** haga **fallar** el run, **antes** de publicar ningún tag.
- Que **todo** build deje un **ancla inmutable por commit** (`sha-<7>`) para poder comparar en más casos.

**Non-Goals**
- No se **prueba** el determinismo dentro de un mismo run (doble build).
- No se comparan digests de **manifest** (compresión); el config digest basta para el caso medido.
- No se borran/reeescriben tags ya publicados.
- No se fijan BuildKit ni los digests de las bases (heredado).

## Decisions

1. **Paso `Enforce reproducibility against the published reference`**: nuevo paso **después** del login y
   **antes** de `Tag and publish`. Lógica (ilustrativa):
   ```yaml
   - name: Enforce reproducibility against the published reference
     env:
       IMAGE: ghcr.io/atareao/apimail
     run: |
       set -euo pipefail
       short_sha="$(printf '%s' "${GITHUB_SHA}" | cut -c1-7)"   # sin ${{ }} en el run:
       reference="${IMAGE}:sha-${short_sha}"
       local_id="$(docker image inspect --format '{{.Id}}' apimail:ci)"

       # Lee el config digest ya publicado del mismo commit. Reintenta SOLO cuando el ancla
       # aún no está: en el flujo de release los runs de main y del tag arrancan casi a la vez
       # y el hermano puede estar empujando justo ahora.
       remote_id=""
       for attempt in 1 2 3; do
         remote_id="$(docker buildx imagetools inspect "${reference}" --raw 2>/dev/null \
           | jq -r '.config.digest' 2>/dev/null || true)"
         if [ -n "${remote_id}" ] && [ "${remote_id}" != "null" ]; then
           break
         fi
         if [ "${attempt}" -lt 3 ]; then
           echo "Reference ${reference} not published yet (attempt ${attempt}/3); retrying in 10s"
           sleep 10
         fi
       done

       if [ -z "${remote_id}" ] || [ "${remote_id}" = "null" ]; then
         echo "No published reference ${reference} after retries; nothing to compare (first build of this commit)."
         exit 0
       fi
       if [ "${local_id}" != "${remote_id}" ]; then
         echo "::error::Reproducibility broken: ${reference} is ${remote_id} but this build produced ${local_id}"
         exit 1
       fi
       echo "Reproducibility OK: ${reference} matches the freshly built image (${local_id})"
   ```
   **Colocación**: tras el `login` (necesita credenciales para leer GHCR) y antes de publicar (si falla,
   **no se empuja nada**).
   **Espera acotada cuando falta el ancla**: la lectura se **reintenta 3 veces con 10 s** (máx ~30 s) y
   **solo** mientras la referencia **no existe** — en el caso normal la primera lectura es válida y el
   bucle hace `break` (no se paga la espera). Motivo: en el flujo de release los runs de `main` (que publica
   `sha-<7>`) y del **tag** (que compara) arrancan con **segundos** de diferencia y tardan lo mismo (~3 min),
   así que sin la espera la comparación sería **una moneda al aire**. *Coste*: hasta ~30 s, y **solo** cuando
   el ancla falta.
   **Robustez del bucle**: si el registro devuelve error, la lectura sale vacía y **se reintenta** (no rompe
   el build); si el registro está inaccesible, el `push` posterior **fallará de todos modos**, así que **no
   se enmascara** nada.
   **Fail-open solo** cuando **no hay** referencia del commit tras los reintentos (primer build de ese
   commit: **no** hay nada que comparar). **Fail-closed** cuando existe y **difiere** (`exit 1`).
   `short_sha` se calcula en shell (nada de `${{ }}` dentro del `run:`); la comparación es de **config
   digest**, que el propio paso de build ya imprime como `Image id:`.
2. **`sha-<7>` también en los eventos de tag**: en el `case` de `Tag and publish`, la rama de tag pasa de
   `("${version}" "${minor#v}" "latest")` a `("${version}" "${minor#v}" "latest" "sha-${short_sha}")`. Así
   **todo** build (push a `main`, tag `vX.Y.Z` y `workflow_dispatch`) publica el ancla inmutable por commit,
   y el enforcement tiene contra qué comparar **siempre** que el mismo commit se construya **dos veces**
   (p. ej. el flujo de release `main` + tag). Es un **cambio de semántica deliberado**; beneficio lateral:
   se puede fijar `ghcr.io/atareao/apimail:sha-<7>`.

## Alternatives Considered

| Opción | Cambia | ¿Verifica el **resultado**? | Coste | Verdict |
|---|---|---|---|---|
| **A. Comparar contra el `sha-<7>` publicado (elegida)** | `image.yml` | sí (entre runners) | ~1 llamada a GHCR | detecta la deriva real, antes de publicar |
| B. Doble build dentro del run (`--no-cache`) | `image.yml` | sí, pero **mismo runner** | **+100 %** del tiempo de build por release (~3 min) | versión **más débil** de la promesa |
| C. Comparar el digest del **manifest** exportando a OCI | `image.yml` | sí, más completo | export + parseo | endurecimiento **opcional**, no implementado |
| D. Guard **estático** (lint que exija las banderas) | CI de lint | **no** | bajo | no ve la deriva del entorno (BuildKit, tag base) |

- **B descartada**: prueba el determinismo en el **mismo runner**, que es una versión **más débil** de la
  promesa (lo que importa es **entre runners**, que es donde falló históricamente), y cuesta el **100 %** del
  tiempo de build **en cada release**.
- **C descartada** (aquí): más completo (cubre la **compresión**), pero añade export a OCI + parseo; se deja
  como **endurecimiento opcional** documentado, no implementado.
- **D descartada**: previene el error de **edición** (que alguien quite una bandera) pero **no** la deriva
  del entorno (BuildKit, tag base); y no verifica el **resultado**, que es lo que de verdad importa.

## Risks / Trade-offs

- *Límite*: el enforcement **solo compara cuando ya existe** una referencia (`sha-<7>`) de **ese mismo
  commit** — valida el **segundo** build contra el **primero**. Un build **solitario** (el primero de un
  commit, o un `workflow_dispatch` aislado) **no se puede validar**. En el flujo de release el caso que
  importa (**main + tag**) **sí** queda cubierto.
- *Trade-off*: la comparación es de **config digest** (incluye `rootfs.diff_ids`, capas **descomprimidas**)
  y **no** cubre la **compresión**. *Aceptación*: la compresión la determina BuildKit y es estable; el
  endurecimiento por **manifest**/OCI queda documentado como opcional.
- *Riesgo*: **carrera** entre el run de `main` y el del tag (corren en paralelo por `concurrency`). Si el
  que va segundo no encuentra aún el `sha-<7>` del otro, **no compara** (fail-open). *Mitigación*: la
  **espera acotada** (3 reintentos × 10 s ≈ 30 s, **solo** cuando falta el ancla) da tiempo al run hermano a
  publicarla; con `sha-<7>` en **ambos** eventos, el que llega segundo **sí** compara en el caso normal.
- *Resto de incertidumbre (dicho tal cual)*: la red de seguridad **no es total**. Si el run hermano tarda
  **más** que esa ventana (~30 s) —o si su **build falla**—, la comparación **no ocurre** y el enforcement
  queda **fail-open en esa ejecución**. No se pretende cobertura completa: se pretende **no quedarse callado
  en el caso normal** (release `main` + tag, casi simultáneos).
- *Riesgo*: el paso añade una **llamada a GHCR** y, si el registro está caído, podría leer vacío y
  **fail-open** (no romper el build). *Aceptación*: se prefiere no bloquear un release legítimo por un
  fallo de lectura; el fallo **real** (digest distinto) sí rompe.
- *Non-goal*: **no** hay borrado/rollback de tags ya publicados.

## Migration Plan

- Aditivo y de bajo riesgo: cambios limitados a `.github/workflows/image.yml`. Sin migración de datos ni de
  la aplicación.
- Rollback: revertir la rama del change (quitar el paso y el `sha-<7>` de los eventos de tag).
- Los artefactos **ya publicados no se reescriben**.

## Verification

- **Positiva (en CI)**: **dos `workflow_dispatch` sobre el MISMO commit** → el **segundo** debe **pasar** el
  enforcement e imprimir `Reproducibility OK: …`; y el pipeline debe seguir **verde** (*smoke test*, uid 1000,
  publish y verificación **post-push**).
- **Negativa (en CI, la importante: hay que ver la alarma sonar)**: en una **rama de usar y tirar** (nunca
  fusionada) se apunta el enforcement a una **referencia conocida y distinta** (p. ej. `reference` forzada a
  `sha-7f20753`, cuyo config es
  `sha256:1360236702532ca10a598765b4fc69d8be1405ccfc3fd6edaf5a32508aa532b4`) mientras se construye un commit
  nuevo → el paso **SHALL** `exit 1` y el job **SHALL** quedar **rojo**, **sin publicar nada**. Después se
  borra la rama. Esa rama es **scaffolding de prueba** y **no** se fusiona.
- **Regresión de alcance**: triggers, `permissions`, `concurrency`, *smoke test*, uid 1000, login, semántica
  de tags (salvo el `sha-<7>` **añadido** a los eventos de tag), reintentos y verificación **post-push** sin
  más cambios.
- **Estática**: `actionlint` (`/tmp/opencode/actionlint`, 1.7.12) → `exit=0`; `bash -n` sobre los bloques
  `run:` (`shellcheck` no instalado; se documenta).
