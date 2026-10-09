# Tasks

## 1. Reproducir el estado actual (evidencia)

- [ ] 1.1 `rg -i 'docker|compose|podman|deploy|ghcr' README.md` → **0** resultados (el README no
  menciona el despliegue contenedorizado); anotar la evidencia.
- [ ] 1.2 Confirmar la **ausencia** de `Dockerfile`, `compose.yml`, `.dockerignore` y de cualquier
  `.env*` en la raíz (`ls`/`git ls-files`), y que `git tag`/Releases solo ofrecen binarios.
- [ ] 1.3 `cat .gitignore` → **solo** `/target` (hoy un `.env` con secretos podría versionarse);
  anotar la evidencia.
- [ ] 1.4 Demostrar que `_verify-health` no puede pasar: `curl` a `/api/health` devuelve
  `{"status","name","version"}` **sin** campo `db`, mientras la receta exige
  `"db": "connected"`.

## 2. Dockerfile y contexto de build

- [ ] 2.1 `Dockerfile`: stage builder `rust:1.98.1-alpine3.21` con `build-base` y `musl-dev`;
  cachear dependencias construyendo primero **solo** la `--lib` (como Valet), copiar `src/` y
  compilar el binario; `strip` del binario final. **Sin** `CFLAGS` (no hay `aws-lc-sys`).
- [ ] 2.2 `Dockerfile`: stage runtime `alpine:3.21`; **sin** `ca-certificates` (pila rustls +
  `webpki-roots`); `WORKDIR /app`; usuario no root uid 1000 (`apimail`); `COPY --from` del binario;
  crear `/app/data` propiedad de ese usuario.
- [ ] 2.3 `Dockerfile`: `EXPOSE 3000`, `HEALTHCHECK` con `wget` contra
  `http://127.0.0.1:${APIMAIL_PORT:-3000}/api/health` y `CMD ["/app/apimail"]`.
- [ ] 2.4 Crear `.dockerignore` excluyendo, entre otros, `target/`, `.git/`, `.github/`,
  `openspec/`, `plans/`, `.opencode/`, `*.md` y **`.env*`** (los secretos no viajan al contexto de
  build).
- [ ] 2.5 Fijar el formato de imagen para conservar la sonda: `.justfile` **exporta
  `BUILDAH_FORMAT=docker`** (Podman construye en OCI por defecto y descartaría `HEALTHCHECK` en
  silencio); documentado en el README y en `design.md`.

## 3. compose.yml, plantilla de entorno y `.gitignore`

- [ ] 3.1 `compose.yml`: servicio `apimail` con `image: ghcr.io/atareao/apimail:latest` y `build:`;
  publicación `"${APIMAIL_PUBLISHED_PORT:-3000}:3000"`; volumen **con nombre** en `/app/data`;
  `environment` con **todas** las variables de apimail y sus valores por defecto;
  `APIMAIL_QUEUE_PATH=/app/data/queue.jsonl`; `restart: unless-stopped`; `healthcheck`.
- [ ] 3.2 Crear `.env.j2` (Jinja2, `{{ apimail_* }}`, comentado en español): variables
  obligatorias (`APIMAIL_API_KEY`, `APIMAIL_IMAP_HOST/USER/PASSWORD`,
  `APIMAIL_SMTP_HOST/USER/PASSWORD`) y opcionales documentadas (puerto, cola, webhook, etc.).
- [ ] 3.3 Crear `.env.example` con las mismas variables y valores vacíos (equivalente sin Jinja2).
- [ ] 3.4 `.gitignore`: añadir `.env` y `.env.local`.

## 4. Publicación en GHCR

- [ ] 4.1 Crear `.github/workflows/image.yml`: disparo en push a `main`, tags `v*` y
  `workflow_dispatch`; permisos `contents: read` y `packages: write`.
- [ ] 4.2 Build con Buildx **sin publicar** (`push: false`, `load: true`, tag `apimail:ci`,
  caché `gha`), **solo `linux/amd64`**.
- [ ] 4.3 *Smoke test*: arrancar el contenedor con las variables obligatorias ficticias y sondear
  `/api/health` comprobando `status == "ok"`, `name == "apimail"` y `version` **igual a la leída de
  `Cargo.toml`** (nunca cableada); **sin** comprobar `db`.
- [ ] 4.4 Solo tras pasar el *smoke test*: login en GHCR y etiquetar/publicar (`latest`, `sha-<7>`
  en push; `vX.Y.Z`, `X.Y`, `latest` en tags; `sha-<7>` en `workflow_dispatch`).

## 5. `.justfile`

- [ ] 5.1 Corregir `dev` y `dev-docker` para apimail (imagen y mensaje «apimail», puerto correcto).
- [ ] 5.2 Corregir `push` a `ghcr.io/atareao/apimail` (no `docker.io`) y `build` al servicio real.
- [ ] 5.3 Corregir `deploy` a `ghcr.io/atareao/apimail` (fuera `valet-ai`) y `deploy-local` al
  servicio `apimail`.
- [ ] 5.4 Corregir `health` y `_verify-health`: **quitar** la exigencia de `"db": "connected"`,
  validar `status == "ok"` (y mostrar `version`) y **seguir `APIMAIL_PUBLISHED_PORT`** al construir
  la URL (`http://127.0.0.1:${APIMAIL_PUBLISHED_PORT:-3000}/api/health`); sin la variable en el
  entorno, asumen el puerto 3000.
- [ ] 5.5 Actualizar el texto de `help` (nombres «Valet» fuera). **Dejar intactas** las recetas
  `frontend-*` y `check-all`.

## 6. README

- [ ] 6.1 Añadir una sección de despliegue con contenedor: construir, `compose up`, variables
  obligatorias, volumen de la cola, sonda de salud, tirar de la imagen de GHCR y las recetas de
  `just` (`dev`, `deploy`, `health`).
- [ ] 6.2 Sin afirmaciones volátiles: nada de «la última versión es X» ni recuentos cableados que
  envejezcan.

## 7. Verificar

- [ ] 7.1 `podman build -t apimail:ci .` termina en verde.
- [ ] 7.2 `podman run` con las variables ficticias y `curl` a `/api/health`: `status == "ok"`,
  `name == "apimail"` y `version` coincide con la de `Cargo.toml`.
- [ ] 7.3 Comprobar que el proceso corre como **uid 1000** y que `/app/data` es escribible.
- [ ] 7.4 Validar `compose.yml` con `podman compose config`.
- [ ] 7.5 Renderizar `.env.j2` a un `.env` temporal con Python `jinja2` y comprobar que el
  resultado es un entorno cargable y sin secretos reales.
- [ ] 7.6 `just test`, `just clippy` y `just fmt` sin regresiones (el change **no** toca Rust).
- [ ] 7.7 Revisar que ningún fichero versionado contiene secretos.
- [ ] 7.8 Pasar `actionlint` sobre `image.yml` **si estuviera disponible** en esta máquina **no**
  lo está: revisar el YAML a mano y dejarlo dicho, sin fingir que se ejecutó.
- [ ] 7.9 `podman build` terminó en verde (~56 s la primera vez); anotar el tiempo.
- [ ] 7.10 `/api/health` devolvió `status`, `name` y `version` coherentes con `Cargo.toml`.
- [ ] 7.11 El proceso corrió como **uid 1000**; `/app/data` fue escribible y `queue.jsonl` se creó
  con permisos `0600`.
- [ ] 7.12 Comparar **OCI vs formato `docker`** para el `HEALTHCHECK` (ver `design.md`): la imagen
  OCI por defecto queda sin sonda; la construida en formato `docker` queda **healthy**.
- [ ] 7.13 Comprobar que la receta arreglada **rechaza un servicio ajeno** (evidencia: un servicio
  escuchando en el 3000 respondía `{"db":"connected","status":"ok","version":"0.10.0"}`, que la
  comprobación antigua habría aceptado) y **acepta** el propio, usando el `name` como
  discriminante.

## 8. Cierre

- [ ] 8.1 Revisar con `docker-expert` (Dockerfile/compose/seguridad del workflow) y, si procede,
  con `rust-reviewer` (que no se toca Rust).
- [ ] 8.2 Marcar tareas y `openspec validate container-image --strict`.
- [ ] 8.3 PR `feature/container-image` → `development`.
