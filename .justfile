# apimail - Justfile

# ── Desarrollo (Podman por defecto) ─────────────────────────────
# El proyecto usa Podman para desarrollo local. Ajusta el binario
# en /usr/bin/podman si tu sistema lo tiene en otra ruta.

# Formatos de --format para podman. Se declaran como variables a propósito:
# en just, `{{{{` se colapsa a `{{` pero `}}}}` NO se colapsa, así que
# escribir '{{{{.Image}}}}' haría que podman recibiera '{{.Image}}}}' y
# añadiera dos llaves al valor. Con una variable el formato se escribe una
# sola vez, tal cual, y se interpola.
user                    := 'atareao'
name                    := `basename ${PWD}`
version                 := `vampus show`
registry                := 'ghcr.io'
podman_fmt_image        := '{{.Image}}'
podman_fmt_id           := '{{.Id}}'
podman_fmt_config_image := '{{.Config.Image}}'

# Podman construye por defecto en formato OCI, que no admite la instrucción
# HEALTHCHECK del Dockerfile y la descartaría en silencio. Forzar el formato
# `docker` conserva la sonda de /api/health en la imagen construida con Podman.
export BUILDAH_FORMAT := 'docker'

# Levanta el servidor con frontend embebido (Podman)
dev:
    podman compose up -d --build --force-recreate
    @echo "apimail: http://localhost:3000"

# Levanta el servidor con frontend embebido (Docker)
dev-docker:
    docker compose up -d
    @echo "apimail: http://localhost:3000"

# ── Imagen y despliegue (GHCR) ──────────────────────────────────
# La imagen publicada vive en ghcr.io/atareao/apimail. `deploy` la
# descarga y recrea el servicio SIN compilar; `deploy-local` compila
# desde el working tree. Ambos verifican /api/health y fallan con
# código distinto de cero si el servicio no queda sano.

push:
    @podman build \
        --tag {{registry}}/{{user}}/{{name}}:{{version}} \
        --tag {{registry}}/{{user}}/{{name}}:latest .
    @podman push {{registry}}/{{user}}/{{name}}:{{version}}
    @podman push {{registry}}/{{user}}/{{name}}:latest

# Construye la imagen local
build:
    podman compose build

# Despliega una imagen publicada de GHCR (tag: latest, sha-abc1234, ...)
deploy tag="latest":
    #!/usr/bin/env bash
    set -euo pipefail
    image="ghcr.io/atareao/apimail:{{tag}}"

    echo "▶ Descargando ${image} ..."
    podman pull "${image}"

    # --no-build garantiza que el deploy NUNCA compila: si la imagen no estuviera
    # ya descargada, fallará en vez de ponerse a construir; no dependemos del pull.
    # --force-recreate es imprescindible: podman-compose compara el hash de la
    # configuración del fichero compose, no el digest de la imagen. Como el
    # fichero no cambia, sin esta opción un `latest` nuevo nunca se aplica.
    echo "▶ Recreando el servicio desde la imagen publicada (sin compilar) ..."
    podman compose up -d --no-build --force-recreate

    # Obtén el contenedor de forma robusta desde compose (sin cablear su nombre).
    cid="$(podman compose ps -q | head -n1)"
    if [ -z "${cid}" ]; then
        echo "❌ No hay contenedor 'apimail' en ejecución tras el despliegue."
        exit 1
    fi

    # Garantiza que corre la imagen publicada (mismo ID) y no una local.
    running_id="$(podman inspect --format '{{ podman_fmt_image }}' "${cid}")"
    published_id="$(podman image inspect --format '{{ podman_fmt_id }}' "${image}")"
    echo "▶ Imagen en ejecución: ${running_id}"
    if [ "${running_id}" != "${published_id}" ]; then
        echo "❌ El contenedor NO usa la imagen publicada (${image})."
        echo "   esperada: ${published_id}"
        echo "   actual:   ${running_id}"
        exit 1
    fi
    echo "✅ El contenedor usa la imagen publicada ${image}."

    just _verify-health

# Compila en local y recrea el servicio, verificando salud
deploy-local:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "▶ Construyendo la imagen local ..."
    podman compose build

    echo "▶ Recreando el servicio ..."
    podman compose up -d --no-build --force-recreate

    cid="$(podman compose ps -q | head -n1)"
    if [ -z "${cid}" ]; then
        echo "❌ No hay contenedor 'apimail' en ejecución tras el despliegue."
        exit 1
    fi
    echo "▶ Imagen en ejecución: $(podman inspect --format '{{ podman_fmt_config_image }}' "${cid}")"

    just _verify-health

# Consulta puntual del endpoint de salud (muestra el JSON)
health:
    curl -fsS "http://127.0.0.1:${APIMAIL_PUBLISHED_PORT:-3000}/api/health"

# (privada) Espera acotada a que /api/health reporte status ok y name apimail
_verify-health:
    #!/usr/bin/env bash
    set -uo pipefail
    # Se usa 127.0.0.1 y no localhost: con el backend de red `passt` localhost
    # resuelve a ::1 y la conexión falla, mientras que 127.0.0.1 funciona tanto
    # con `passt` como con `rootlessport`.
    url="http://127.0.0.1:${APIMAIL_PUBLISHED_PORT:-3000}/api/health"
    attempts=30
    delay=2

    echo "▶ Esperando ${url} hasta $((attempts * delay))s ..."
    body=""
    for ((i = 1; i <= attempts; i++)); do
        if body="$(curl -fsS "${url}" 2>/dev/null)"; then
            if echo "${body}" | grep -Eq '"status"[[:space:]]*:[[:space:]]*"ok"' \
                && echo "${body}" | grep -Eq '"name"[[:space:]]*:[[:space:]]*"apimail"'; then
                echo "✅ Servicio sano tras ${i} intento(s)."
                echo "   ${body}"
                version="$(echo "${body}" | sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
                echo "   Versión: ${version:-desconocida}"
                exit 0
            fi
        fi
        sleep "${delay}"
    done

    echo "❌ El servicio NO está sano tras $((attempts * delay))s."
    echo "   Última respuesta: ${body:-<sin respuesta>}"
    exit 1

# ── Calidad ─────────────────────────────────────────────────────

# Ejecuta todos los checks (Rust + frontend)
check-all: fmt clippy test frontend-lint frontend-test frontend-check

# Tests Rust
test:
    cargo test

# Clippy (zero warnings, incluye targets de test para igualar el CI)
clippy:
    cargo clippy --all-targets -- -D warnings

# Formato
fmt:
    cargo fmt --check

# Frontend: chequeo de tipos + build (desarrollo standalone con Vite)
frontend-check:
    cd frontend && npx tsc --noEmit && npm run build

# Frontend: ESLint en modo CI
frontend-lint:
    cd frontend && npm run lint:ci

# Frontend: tests con Vitest (una sola pasada)
frontend-test:
    cd frontend && npx vitest run

# Check que existe un change proposal activo en openspec
check-spec:
    @ls openspec/changes/*/proposal.md 2>/dev/null || (echo "❌ No active change proposal found. Run: openspec new change <feature>" && exit 1)
    @echo "✅ Active change proposal found."

# Limpia todo
clean:
    cargo clean
    rm -rf frontend/dist frontend/node_modules

# Ayuda
help:
    @echo "Comandos disponibles:"
    @echo "  just dev           - Levanta apimail con Podman (reconstruye imagen)"
    @echo "  just dev-docker    - Levanta apimail con Docker"
    @echo "  just build         - Construye la imagen local"
    @echo "  just deploy [tag]  - Despliega la imagen de GHCR (por defecto latest)"
    @echo "  just deploy-local  - Construye en local y despliega"
    @echo "  just health        - Consulta el endpoint /api/health"
    @echo "  just check-all     - Ejecuta todos los checks (Rust + frontend)"
    @echo "  just test          - cargo test"
    @echo "  just clippy        - cargo clippy"
    @echo "  just fmt           - cargo fmt --check"
    @echo "  just frontend-lint - ESLint del frontend (modo CI)"
    @echo "  just frontend-test - Tests del frontend con Vitest"
    @echo "  just frontend-check- Tipos + build del frontend"
    @echo "  just check-spec    - Verifica change proposal activo"
