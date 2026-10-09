# ═══════════════════════════════════════════════════════════════
# Stage 1: Builder (Rust sobre musl)
# ═══════════════════════════════════════════════════════════════
# El prefijo `docker.io/library/` es deliberado: evita depender de la
# configuración de registries sin cualificar de la máquina que construye.
FROM docker.io/library/rust:1.98.1-alpine3.21 AS builder

# `ring` es el único backend criptográfico del árbol (no hay `aws-lc-sys`),
# así que un compilador de C (`build-base`) más `musl-dev` bastan: no hacen
# falta los `CFLAGS` que otros proyectos necesitan.
RUN apk add --no-cache build-base musl-dev

WORKDIR /build

# Caché de dependencias: se construye solo la librería, que arrastra todo el
# grafo, mientras las fuentes reales todavía no están. Un `cargo build` a
# secas exigiría que existieran todas las fuentes de los `[[bin]]` en este
# punto. Se copia el manifiesto real después del `cargo init` de relleno.
RUN cargo init --bin --name apimail . && \
    echo "pub fn dummy() {}" > src/lib.rs

COPY Cargo.toml Cargo.lock ./
RUN cargo build --release --lib && \
    rm -rf src

COPY src ./src
RUN touch src/*.rs && \
    cargo build --release && \
    strip target/release/apimail

# ═══════════════════════════════════════════════════════════════
# Stage 2: Runtime
# ═══════════════════════════════════════════════════════════════
FROM alpine:3.21

# Sin `ca-certificates` a propósito: la pila TLS es 100 % rustls con
# `webpki-roots`, que lleva las raíces de confianza empaquetadas en el propio
# binario y nunca consulta el almacén del sistema. Instalarlo sería peso muerto.

# Usuario no root (uid 1000).
RUN addgroup -g 1000 apimail && \
    adduser -D -u 1000 -G apimail apimail

WORKDIR /app

# `/app/data` es el punto de montaje del volumen con nombre de la cola. El
# propietario se fija en la imagen para que un volumen con nombre recién creado
# herede ese uid; con un bind mount habría que alinearlo en el host.
RUN mkdir -p /app/data && \
    chown apimail:apimail /app/data

COPY --from=builder --chown=apimail:apimail /build/target/release/apimail /app/apimail

USER apimail

EXPOSE 3000

# Forma shell: `${APIMAIL_PORT:-3000}` lo expande el shell del contenedor en
# tiempo de ejecución, así que la sonda sigue al puerto de escucha real.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD wget -qO- "http://127.0.0.1:${APIMAIL_PORT:-3000}/api/health" || exit 1

CMD ["/app/apimail"]
