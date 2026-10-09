# Design

## Context

- **Distribución actual**: apimail se publica como **binario** (GitHub Releases para
  `x86_64-unknown-linux-gnu` y `aarch64-unknown-linux-gnu`) y como **crate** en crates.io
  (`0.3.2`); no existe ningún artefacto de contenedor.
- **Pila TLS 100 % rustls** (verificado): `cargo tree -i openssl-sys` y
  `cargo tree -i rustls-native-certs` **no encuentran el paquete**; `src/imap.rs:966` construye el
  almacén con `RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS…)`; `lettre` usa
  `tokio1-rustls-tls` y `reqwest` usa `rustls-tls-webpki-roots`. Como `webpki-roots` lleva las
  raíces **empaquetadas en el binario** y **no** consulta el almacén del sistema, la imagen de
  runtime **no** necesita `ca-certificates` (instalarlo sería peso muerto). Esto la diferencia del
  Dockerfile de Valet, que sí lo instala.
- **Backend criptográfico: solo `ring` 0.17.14** (verificado con `cargo tree`), **sin**
  `aws-lc-sys` ni `cmake` en el árbol. El build en musl solo necesita un compilador de C
  (`build-base`), **sin** los *hacks* de `CFLAGS` que Valet necesita para `aws-lc-sys`.
- **`GET /api/health` es público**: está exento del middleware `require_api_key` (doc de
  `src/http.rs`: «`GET /api/health` is **public**») y devuelve
  `{"status":"ok","name":"apimail","version":"<CARGO_PKG_VERSION>"}`. Sirve como sonda **sin**
  credenciales.
- **Arranque fail-closed**: sin `APIMAIL_API_KEY` no vacía y sin las 6 variables de cuenta
  (`APIMAIL_IMAP_HOST/USER/PASSWORD`, `APIMAIL_SMTP_HOST/USER/PASSWORD`) el proceso **aborta**. La
  conexión IMAP es **perezosa**, así que la sonda de salud no necesita que el servidor de correo
  exista.
- **Cola del webhook**: `APIMAIL_QUEUE_PATH` es *opt-in*; sin ella la cola vive en memoria y cada
  recreación del contenedor perdería notificaciones encoladas. Con ella, la entrega es
  *at-least-once* con reanudación y el fichero se crea con permisos `0600`.
- **Patrón de la casa** (`/data/rust/valet`, origen del `.justfile` copiado): Dockerfile
  multi-stage musl, `compose.yml` con `image: ghcr.io/atareao/<proyecto>`, `.env.j2` Jinja2,
  `.dockerignore` que excluye `.env*`, `.gitignore` que ignora `.env` y un `image.yml` con Buildx +
  *smoke test* contra `/api/health` leyendo la versión esperada de `Cargo.toml` (nunca cableada).
- **Herramientas locales**: hay `podman` 6.1.2, `buildah`, `jinja2`, `yq`, `jq` y `curl`; **no**
  hay `docker` ni `actionlint`.

## Goals / Non-Goals

**Goals**
- Poder construir y ejecutar apimail en un contenedor con un solo comando, con la cola del webhook
  persistente **por defecto**.
- Publicar una imagen en `ghcr.io/atareao/apimail` que solo se etiquete como publicable **después**
  de pasar un *smoke test* real contra `/api/health`.
- Dejar el `.justfile` con recetas de contenedor y salud que reflejen apimail (no Valet) y que
  **no** fallen por un campo `db` inexistente.
- Evitar que secretos de entorno entren al repositorio o al contexto de build.

**Non-Goals**
- No se cambia la API, la configuración del crate ni ningún fichero Rust.
- No se arreglan las recetas `frontend-*` ni `check-all` (son otra deuda, fuera de alcance).
- No se publica multi-arch: **solo `linux/amd64`**.
- No se añade `ca-certificates` ni `CFLAGS` «por si acaso».

## Decisions

1. **Imagen base `alpine` + musl** (patrón de la casa) frente a `debian-slim` y `distroless/cc`.
   **Veredicto: alpine.** Aporta una base de ~3,6 MB (orden de magnitud) y una imagen final del
   orden de **25-30 MB**; además `wget` de BusyBox ya está disponible para el `HEALTHCHECK` y queda
   un shell para depurar. Descartadas: `debian-slim` (imagen del orden de 55-60 MB y habría que
   instalar `curl` para la sonda) y `distroless/cc` (ahorra del orden de 10 MB pero **no tiene
   shell ni `curl`**: se quedaría sin `HEALTHCHECK` y sin `docker exec` para depurar). Las cifras
   son **órdenes de magnitud**, no medidas.
2. **Omitir `ca-certificates`** en el runtime. Es una desviación **deliberada y verificada**
   respecto a Valet, **no** un olvido: `webpki-roots` lleva las raíces empaquetadas en el binario y
   no consulta el almacén del sistema, así que el paquete no cambiaría nada (ver Context).
3. **Usuario no root** (uid 1000, p. ej. `apimail`) con volumen **con nombre** (no *bind mount*).
   El `chown` de `/app/data` se hace en la imagen, de modo que el propietario sobrevive a la
   creación del volumen; con un *bind mount* el uid debería alinearse en el host. Valet corre como
   root; aquí endurecerlo sale gratis.
4. **`HEALTHCHECK` sobre `/api/health`**, usando el puerto interno real vía `${APIMAIL_PORT:-3000}`
   en forma *shell*, para que no se rompa si alguien cambia el puerto de escucha. **Comprobación y
   mitigación (verificada en local)**: Podman construye **por defecto en formato OCI**, que **no
   admite** la instrucción `HEALTHCHECK` y la **descarta en silencio** (aviso `HEALTHCHECK is not
   supported for OCI image format and will be ignored`). Medido sobre la imagen real: la construida
   en OCI por defecto (`podman build`) deja el contenedor **sin** `.Config.Healthcheck` y **sin**
   `.State.Health` (sin sonda); las construidas con `--format docker` y con `BUILDAH_FORMAT=docker
   podman build` conservan la sonda (CMD-SHELL + tiempos) y el contenedor queda **healthy**. Por eso
   el `.justfile` **exporta `BUILDAH_FORMAT=docker`** (ver Risks / Trade-offs), mientras que el
   `healthcheck` declarado en `compose.yml` da la sonda con independencia del formato: `compose up`
   nunca se queda sin ella.
5. **Puerto**: el contenedor escucha en **3000** (`APIMAIL_PORT`); hacia fuera se publica con una
   variable de compose propia, **`APIMAIL_PUBLISHED_PORT`** (por defecto 3000), para no confundir
   «puerto del host» con el `APIMAIL_PORT` que entiende la aplicación.
6. **`image.yml`**: se dispara al hacer push a `main`, al empujar un tag `v*` y manualmente
   (`workflow_dispatch`); construye **sin** publicar, hace *smoke test* y **solo entonces** etiqueta
   y publica (`latest`, `sha-<7>` y, en tags, `vX.Y.Z`, `X.Y`, `latest`). El *smoke test* **MUST**
   aportar las variables obligatorias con valores ficticios (`APIMAIL_API_KEY`, `APIMAIL_IMAP_*` y
   `APIMAIL_SMTP_*`) y comprobar `status == "ok"`, `name == "apimail"` y `version` igual a la leída
   de `Cargo.toml` — **nunca** `db`, que no existe.
7. **Cola persistente por defecto**: volumen con nombre montado en `/app/data` y
   `APIMAIL_QUEUE_PATH=/app/data/queue.jsonl` por defecto (sobreescribible desde el entorno). Sin
   ella, cada recreación del contenedor perdería en silencio las notificaciones encoladas, que es
   justo el problema que resolvió el trabajo de `webhook-delivery`.

## Alternatives Considered

- **`debian-slim`** como runtime: descartado. Imagen más grande y requeriría instalar `curl` solo
  para la sonda de salud. No compra nada frente a alpine.
- **`distroless/cc`**: descartado. Ahorra unos MB, pero al no traer shell ni `curl` deja la imagen
  sin `HEALTHCHECK` y sin `docker exec` para depurar. Para un servicio que se depura en vivo, es un
  coste operativo mayor que el ahorro.
- **Publicar multi-arch (`amd64` + `arm64`)**: descartado. El resto de la casa publica solo
  `linux/amd64`; ampliar aquí sería una excepción sin necesidad verificada.
- **Instalar `ca-certificates` «por costumbre»** (como Valet): descartado por la Decisión 2.
- **Publicar la imagen directamente sin *smoke test***: descartado. Etiquetar solo lo verificado es
  la garantía de que `latest` nunca apunta a una imagen que no arranca.
- **Bind mount en vez de volumen con nombre**: descartado por la Decisión 3 (propietario y
  portabilidad).
- **Dejar la cola en memoria (sin volumen)**: descartado por la Decisión 7.

## Risks / Trade-offs

- *Riesgo*: el *smoke test* del workflow usa `docker` (en los runners de GitHub), mientras la
  verificación local de esta máquina se hace con `podman`. *Mitigación/aceptación*: se acepta; el
  test es el mismo comando (`podman`/`docker run`) contra el mismo endpoint, y en local se valida
  con `podman` (ver Verification).
- *Trade-off*: la imagen se construye con **musl** mientras las releases publican binarios
  **glibc**: son dos artefactos distintos del mismo tag. *Aceptación*: coste aceptado a cambio de
  coherencia con la casa y de una imagen más pequeña.
- *Riesgo*: `ring` compila en musl solo con `build-base` (sin `aws-lc-sys`). Si algún día entrara
  `aws-lc-sys` en el árbol, habría que revisar el stage de build (tal vez añadir `cmake` y los
  *hacks* de `CFLAGS` de Valet). *Detección*: el build de la imagen fallaría con un error de
  compilación nativo, visible en el stage del builder.
- *Riesgo*: la visibilidad del paquete en GHCR es **privada por defecto**: publicar la imagen no la
  hace descargable por terceros hasta que se cambie la visibilidad del paquete. *Seguimiento*:
  queda como tarea manual posterior; no bloquea el change.
- *Trade-off*: publicar `latest` en cada push a `main` mueve la etiqueta móvil; los tags inmutables
  (`sha-*`, `vX.Y.Z`) quedan para los despliegues fijados.
- *Riesgo*: Podman construye **por defecto en formato OCI**, que no admite `HEALTHCHECK`: una imagen
  construida «a pelo» con `podman build` saldría **sin** sonda y **en silencio**. *Mitigación
  (verificada en local con `podman`)*: el `.justfile` exporta `BUILDAH_FORMAT=docker`, así que
  `just build`/`just dev`/`just deploy-local` conservan la sonda, y el `healthcheck` de
  `compose.yml` la aporta con independencia del formato; documentado en el README.
  - **Verificado en local con Podman 6.1.2 (medición)**: la imagen en OCI por defecto deja el
    contenedor **sin** `.Config.Healthcheck` ni `.State.Health`; la imagen en formato `docker`
    (`--format docker` o `BUILDAH_FORMAT=docker`) mantiene la sonda presente y el contenedor
    **healthy**.
  - **Razonado, no verificable en esta máquina**: el workflow de GHCR usa `load: true` + `docker
    tag`/`docker push` sobre la imagen cargada por el exportador `docker` del daemon, por lo que la
    sonda viaja con ella. En esta máquina **no hay `docker`**, así que esto es **razonamiento, no
    medición**.
- *Riesgo*: las recetas `health` y `_verify-health` del `.justfile` fijan el puerto del host. Como
  el contenedor se publica con `APIMAIL_PUBLISHED_PORT` (por defecto 3000) pero el puerto interno
  es fijo (3000), una receta con el 3000 cableado apuntaría **en silencio** a un sitio equivocado
  en cuanto se publique en otro puerto. *Mitigación*: ambas recetas construyen la URL siguiendo la
  variable de entorno (`http://127.0.0.1:${APIMAIL_PUBLISHED_PORT:-3000}/api/health`); si la
  variable no está en el entorno, asumen el valor por defecto 3000. **Evidencia medida** (refuerza
  además el cambio de criterio de `_verify-health`): en una máquina con **otro servicio escuchando
  en el 3000**, ese servicio ajeno respondía exactamente
  `{"db":"connected","status":"ok","version":"0.10.0"}`, de modo que la comprobación **antigua**
  (que exigía `"status":"ok"` + `"db":"connected"`) habría dado por **sano un servicio
  equivocado**. La comprobación **nueva** exige `name == "apimail"` y por eso **rechaza** al
  ajeno, que no lo declara.

## Migration Plan

- Aditivo y de bajo riesgo: ficheros de infraestructura nuevos, un `.gitignore` ampliado, recetas
  de `just` corregidas y una sección de README. Sin migración de datos.
- Rollback: revertir la rama `feature/container-image`.

## Verification

- `podman build -t apimail:ci .` termina en verde.
- `podman run` con variables ficticias (obligatorias, ver Decisión 6) y `curl` a `/api/health`
  comprobando `status == "ok"`, `name == "apimail"` y `version` **igual a la de `Cargo.toml`**.
- El proceso corre como **uid 1000** y `/app/data` es escribible por él.
- `podman compose config` valida `compose.yml`.
- `.env.j2` renderiza con Python `jinja2` a un `.env` temporal, cargable y sin secretos reales.
- `just test`, `just clippy` y `just fmt` sin regresiones (el change **no** toca Rust).
- Revisar que ningún fichero versionado contiene secretos.
- `actionlint` sobre `image.yml` **si estuviera disponible** (en esta máquina no lo está: se revisa
  el YAML a mano).
