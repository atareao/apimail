# Tasks

## 1. Reproducir el estado actual (evidencia)

- [x] 1.1 `rg -i 'docker|compose|podman|deploy|ghcr' README.md` → **0** resultados (el README no
  menciona el despliegue contenedorizado); anotar la evidencia.
  - Evidencia: `rg -i 'docker|compose|podman|deploy|ghcr' README.md` → **0** resultados (antes del change).
- [x] 1.2 Confirmar la **ausencia** de `Dockerfile`, `compose.yml`, `.dockerignore` y de cualquier
  `.env*` en la raíz (`ls`/`git ls-files`), y que `git tag`/Releases solo ofrecen binarios.
  - Evidencia: no existían `Dockerfile`, `compose.yml`, `.dockerignore` ni ningún `.env*`; las releases solo ofrecían binarios (glibc) y el crate en crates.io.
- [x] 1.3 `cat .gitignore` → **solo** `/target` (hoy un `.env` con secretos podría versionarse);
  anotar la evidencia.
  - Evidencia: `cat .gitignore` → **solo** `/target`.
- [x] 1.4 Demostrar que `_verify-health` no puede pasar: `curl` a `/api/health` devuelve
  `{"status","name","version"}` **sin** campo `db`, mientras la receta exige
  `"db": "connected"`.
  - Evidencia: `/api/health` devuelve `{"status","name","version"}` **sin** campo `db`; la receta antigua exigía `"db": "connected"` → fallaba siempre.

## 2. Dockerfile y contexto de build

- [x] 2.1 `Dockerfile`: stage builder `rust:1.98.1-alpine3.21` con `build-base` y `musl-dev`;
  cachear dependencias construyendo primero **solo** la `--lib` (como Valet), copiar `src/` y
  compilar el binario; `strip` del binario final. **Sin** `CFLAGS` (no hay `aws-lc-sys`).
  - Evidencia: `podman build -t apimail:ci .` en verde; el builder musl necesitó **solo** `build-base` + `musl-dev`, **sin** `CFLAGS` (el árbol solo trae `ring` 0.17.14, sin `aws-lc-sys` ni `cmake`).
- [x] 2.2 `Dockerfile`: stage runtime `alpine:3.21`; **sin** `ca-certificates` (pila rustls +
  `webpki-roots`); `WORKDIR /app`; usuario no root uid 1000 (`apimail`); `COPY --from` del binario;
  crear `/app/data` propiedad de ese usuario.
  - Evidencia: proceso como `uid=1000` (`whoami=apimail`); `/app/data` escribible y `queue.jsonl` creado con `-rw-------` (**0600**) dentro del volumen con nombre `apimail_apimail_data`.
- [x] 2.3 `Dockerfile`: `EXPOSE 3000`, `HEALTHCHECK` con `wget` contra
  `http://127.0.0.1:${APIMAIL_PORT:-3000}/api/health` y `CMD ["/app/apimail"]`.
  - Evidencia: log `INFO apimail listening on 0.0.0.0:3000`; con formato `docker` el contenedor queda `healthy` (sonda presente).
- [x] 2.4 Crear `.dockerignore` excluyendo, entre otros, `target/`, `.git/`, `.github/`,
  `openspec/`, `plans/`, `.opencode/`, `*.md` y **`.env*`** (los secretos no viajan al contexto de
  build).
  - Evidencia: el build **no** requirió `target/` (excluido por `.dockerignore`); el contexto de build se construyó sin él.
- [x] 2.5 Fijar el formato de imagen para conservar la sonda: `.justfile` **exporta
  `BUILDAH_FORMAT=docker`** (Podman construye en OCI por defecto y descartaría `HEALTHCHECK` en
  silencio); documentado en el README y en `design.md`.
  - Evidencia: `just --evaluate` muestra `BUILDAH_FORMAT := 'docker'`; la imagen OCI por defecto queda **sin** sonda y la de formato `docker` queda **healthy** (design.md, decisión 4).

## 3. compose.yml, plantilla de entorno y `.gitignore`

- [x] 3.1 `compose.yml`: servicio `apimail` con `image: ghcr.io/atareao/apimail:latest` y `build:`;
  publicación `"${APIMAIL_PUBLISHED_PORT:-3000}:3000"`; volumen **con nombre** en `/app/data`;
  `environment` con **todas** las variables de apimail y sus valores por defecto;
  `APIMAIL_QUEUE_PATH=/app/data/queue.jsonl`; `restart: unless-stopped`; `healthcheck`.
  - Evidencia: `podman compose config` valida e interpola `APIMAIL_QUEUE_PATH: /app/data/queue.jsonl`, `APIMAIL_IMAP_TLS: implicit` e `image: ghcr.io/atareao/apimail:latest`.
  - Evidencia: con `APIMAIL_QUEUE_PATH=` en blanco la interpolación da `''` → la cola se devuelve a memoria (el cambio de `${VAR:-…}` a `${VAR-…}` funciona en `podman compose`).
- [x] 3.2 Crear `.env.j2` (Jinja2, `{{ apimail_* }}`, comentado en español): variables
  obligatorias (`APIMAIL_API_KEY`, `APIMAIL_IMAP_HOST/USER/PASSWORD`,
  `APIMAIL_SMTP_HOST/USER/PASSWORD`) y opcionales documentadas (puerto, cola, webhook, etc.).
  - Evidencia: `.env.j2` renderiza con Python `jinja2` **3.1.6**: sin ningún `{{` pendiente y con los **7** obligatorios presentes.
- [x] 3.3 Crear `.env.example` con las mismas variables y valores vacíos (equivalente sin Jinja2).
  - Evidencia: `.env.example` reproduce las mismas variables con valores vacíos (equivalente sin Jinja2), sin secretos.
- [x] 3.4 `.gitignore`: añadir `.env` y `.env.local`.
  - Evidencia: `.gitignore` ignora `.env` y `.env.local`.

## 4. Publicación en GHCR

- [x] 4.1 Crear `.github/workflows/image.yml`: disparo en push a `main`, tags `v*` y
  `workflow_dispatch`; permisos `contents: read` y `packages: write`.
  - Evidencia: workflow revisado a mano; dispara en push a `main` (ignorando `**.md`/`openspec/**`), tags `v*` y `workflow_dispatch`, con `contents: read` y `packages: write`.
- [x] 4.2 Build con Buildx **sin publicar** (`push: false`, `load: true`, tag `apimail:ci`,
  caché `gha`), **solo `linux/amd64`**.
  - Evidencia: step de `docker/build-push-action` con `push: false`, `load: true`, `tags: apimail:ci`, `platforms: linux/amd64` y caché `gha`.
- [x] 4.3 *Smoke test*: arrancar el contenedor con las variables obligatorias ficticias y sondear
  `/api/health` comprobando `status == "ok"`, `name == "apimail"` y `version` **igual a la leída de
  `Cargo.toml`** (nunca cableada); **sin** comprobar `db`.
  - Evidencia: el smoke test lee la versión de `Cargo.toml` (nunca cableada) y exige `status == "ok"`, `name == "apimail"` y `version == expected`; no comprueba `db`.
- [x] 4.4 Solo tras pasar el *smoke test*: login en GHCR y etiquetar/publicar (`latest`, `sha-<7>`
  en push; `vX.Y.Z`, `X.Y`, `latest` en tags; `sha-<7>` en `workflow_dispatch`).
  - Evidencia: login en GHCR (`docker/login-action`) y publicación etiquetada según evento (`latest`/`sha-<7>` en push a `main`; `vX.Y.Z`/`X.Y`/`latest` en tags; `sha-<7>` en `workflow_dispatch`).

## 5. `.justfile`

- [x] 5.1 Corregir `dev` y `dev-docker` para apimail (imagen y mensaje «apimail», puerto correcto).
  - Evidencia: `just --list` sin errores de sintaxis; `dev`/`dev-docker` apuntan a apimail y al puerto 3000.
- [x] 5.2 Corregir `push` a `ghcr.io/atareao/apimail` (no `docker.io`) y `build` al servicio real.
  - Evidencia: `push` publica en `ghcr.io/atareao/apimail` (no `docker.io`); `build` usa `podman compose build`.
- [x] 5.3 Corregir `deploy` a `ghcr.io/atareao/apimail` (fuera `valet-ai`) y `deploy-local` al
  servicio `apimail`.
  - Evidencia: `deploy` usa `ghcr.io/atareao/apimail` (fuera `valet-ai`); `deploy-local` recrea el servicio `apimail`; `just --dry-run deploy` sin errores.
- [x] 5.4 Corregir `health` y `_verify-health`: **quitar** la exigencia de `"db": "connected"`,
  validar `status == "ok"` (y mostrar `version`) y **seguir `APIMAIL_PUBLISHED_PORT`** al construir
  la URL (`http://127.0.0.1:${APIMAIL_PUBLISHED_PORT:-3000}/api/health`); sin la variable en el
  entorno, asumen el puerto 3000.
  - Evidencia: `health`/`_verify-health` sin exigencia de `db`, validan `status == "ok"` + `name == "apimail"` y siguen `APIMAIL_PUBLISHED_PORT`; `APIMAIL_PUBLISHED_PORT=3978 just _verify-health` → `✅ Servicio sano tras 1 intento(s)`.
- [x] 5.5 Actualizar el texto de `help` (nombres «Valet» fuera). **Dejar intactas** las recetas
  `frontend-*` y `check-all`.
  - Evidencia: cero referencias a «Valet» en el `.justfile`; `help` con textos de apimail; `frontend-*` y `check-all` intactas; `just --dry-run health` sin errores.

## 6. README

- [x] 6.1 Añadir una sección de despliegue con contenedor: construir, `compose up`, variables
  obligatorias, volumen de la cola, sonda de salud, tirar de la imagen de GHCR y las recetas de
  `just` (`dev`, `deploy`, `health`).
  - Evidencia: sección de despliegue añadida al README; las dos anclas enlazadas (`## Ejecución`, `#### Cola de entrega duradera`) existen.
- [x] 6.2 Sin afirmaciones volátiles: nada de «la última versión es X» ni recuentos cableados que
  envejezcan.
  - Evidencia: sin afirmaciones volátiles (sin «la última versión es X» ni recuentos cableados).

## 7. Verificar

- [x] 7.1 `podman build -t apimail:ci .` termina en verde.
  - Evidencia: `podman build -t apimail:ci .` en verde.
- [x] 7.2 `podman run` con las variables ficticias y `curl` a `/api/health`: `status == "ok"`,
  `name == "apimail"` y `version` coincide con la de `Cargo.toml`.
  - Evidencia: `curl` a `/api/health` → `{"status":"ok","name":"apimail","version":"0.3.2"}`, coincidiendo con `Cargo.toml` (`version = "0.3.2"`).
- [x] 7.3 Comprobar que el proceso corre como **uid 1000** y que `/app/data` es escribible.
  - Evidencia: proceso como `uid=1000`; `/app/data` escribible.
- [x] 7.4 Validar `compose.yml` con `podman compose config`.
  - Evidencia: `podman compose config` valida el fichero.
  - Evidencia: camino documentado de punta a punta (`.env` sin puertos definidos, publicado en 3978 porque el 3000 estaba ocupado): `BUILDAH_FORMAT=docker podman compose up -d --build` → `Up 8 seconds (healthy)`, `RestartCount = 0`, `healthcheck: healthy`; log `INFO apimail listening on 0.0.0.0:3000`.
  - Evidencia (contraste, a propósito): con el defecto de puerto en blanco **antes** de `blank-port-as-absent`, el mismo `compose up` entraba en bucle de reinicio (`ERROR failed to start apimail: invalid APIMAIL_IMAP_PORT`).
- [x] 7.5 Renderizar `.env.j2` a un `.env` temporal con Python `jinja2` y comprobar que el
  resultado es un entorno cargable y sin secretos reales.
  - Evidencia: renderizado con `jinja2` 3.1.6 sin `{{` pendientes y con los 7 obligatorios; solo placeholders, sin secretos reales.
- [x] 7.6 `just test`, `just clippy` y `just fmt` sin regresiones (el change **no** toca Rust).
  - Evidencia: `just fmt` OK; `just clippy` cero warnings; `just test` → **313 tests, 0 failed**.
- [x] 7.7 Revisar que ningún fichero versionado contiene secretos.
  - Evidencia: `.env`/`.env.local` ignorados por `.gitignore`; `.env.example`/`.env.j2` solo con valores vacíos o `{{ … }}`.
- [x] 7.8 Pasar `actionlint` sobre `image.yml` **si estuviera disponible** en esta máquina **no**
  lo está: revisar el YAML a mano y dejarlo dicho, sin fingir que se ejecutó.
  - Evidencia: `actionlint` **no** disponible en esta máquina; YAML revisado a mano (sin fingir que se ejecutó).
- [x] 7.9 `podman build` terminó en verde (~56 s la primera vez); anotar el tiempo.
  - Evidencia: `podman build` en verde; **~56 s** la primera vez.
- [x] 7.10 `/api/health` devolvió `status`, `name` y `version` coherentes con `Cargo.toml`.
  - Evidencia: `/api/health` → `{"status":"ok","name":"apimail","version":"0.3.2"}`, coherente con `Cargo.toml`.
- [x] 7.11 El proceso corrió como **uid 1000**; `/app/data` fue escribible y `queue.jsonl` se creó
  con permisos `0600`.
  - Evidencia: `uid=1000`; `/app/data` escribible; `queue.jsonl` con permisos `0600` (`-rw-------`).
- [x] 7.12 Comparar **OCI vs formato `docker`** para el `HEALTHCHECK` (ver `design.md`): la imagen
  OCI por defecto queda sin sonda; la construida en formato `docker` queda **healthy**.
  - Evidencia: OCI (por defecto) → contenedor **sin** `.Config.Healthcheck` (`null`) y **sin** `.State.Health`; formato `docker` (`--format docker` y `BUILDAH_FORMAT=docker`) → `.Config.Healthcheck` presente y `State.Health = healthy`.
- [x] 7.13 Comprobar que la receta arreglada **rechaza un servicio ajeno** (evidencia: un servicio
  escuchando en el 3000 respondía `{"db":"connected","status":"ok","version":"0.10.0"}`, que la
  comprobación antigua habría aceptado) y **acepta** el propio, usando el `name` como
  discriminante.
  - Evidencia: en esta máquina el puerto 3000 lo ocupaba otro servicio que respondía `{"db":"connected","status":"ok","version":"0.10.0"}`; la comprobación antigua lo habría dado por sano; la nueva exige `name == "apimail"` y lo rechaza, aceptando el propio.

## 8. Cierre

- [x] 8.1 Revisar con `docker-expert` (Dockerfile/compose/seguridad del workflow) y, si procede,
  con `rust-reviewer` (que no se toca Rust).
  - Evidencia: revisión estática de Dockerfile/compose/seguridad del workflow; `rust-reviewer` no procede — el change **no** toca Rust (313 tests verdes).
- [x] 8.2 Marcar tareas y `openspec validate container-image --strict`.
  - Evidencia: tareas marcadas; `openspec validate container-image --strict` válido.
- [x] 8.3 PR `feature/container-image` → `development` (PR #34).
