# Proposal

## Why

El release `v0.4.1` quedó completo y verificado (crates.io, GitHub Releases, docs.rs e imagen en
GHCR), pero **`latest` y `v0.4.1` no comparten digest aunque son el mismo commit** `45c4355`. Dos
builds **en paralelo** del mismo árbol —lo que permite a propósito el `concurrency` **por `ref`** del
change archivado `image-publish-resilience` (su decisión 2)— produjeron **dos imágenes distintas**:

1. **Run del tag** `38026924672` → `v0.4.1`, `0.4` = manifest
   `sha256:fa73b4cf7e3640ed3fc9e28a85062d83af17c9491e44a47f8da0faee4867834c`.
2. **Run de `main`** `38026923522` → `latest`, `sha-45c4355` = manifest
   `sha256:258c5e8da99faa8dbf8cb28cba749e651ff879a88b640108454ee111d6a3c4b9`.
3. Como el **último en empujar gana `latest`**, `latest` puede acabar apuntando a un digest **distinto**
   del de los alias inmutables (`vX.Y.Z`, `X.Y`) del **mismo** código.

El no-determinismo está **medido** (comparando con `podman` las dos imágenes publicadas, no inferido):

- **Contenido idéntico**: `diff -r --no-dereference` de los dos rootfs → **sin diferencias**; el
  binario `/app/apimail` es **idéntico byte a byte** en ambos (`sha256=0849686e7defc91e7a80ecf436925563…`,
  **6.741.544 B** en los dos).
- **528 entradas** por imagen; **11** cambian de **mtime** (precisión de segundos), **todas**
  consecuencia de los pasos `adduser`/`mkdir`+`chown`/`COPY` del `Dockerfile`: `.`, `./app`,
  `./app/apimail`, `./app/data`, `./etc`, `./etc/group`, `./etc/group-`, `./etc/passwd`, `./etc/shadow`,
  `./home`, `./home/apimail`.
- **Prueba decisiva**: normalizando **solo** las fechas (`tar --sort=name --mtime='@0' …`) los dos
  árboles dan la **misma huella** `4f3694dd5f20c510d689a33be240061def0641a6…`; **sin** normalizar, las
  huellas **difieren** (control del método: el método sí distingue).
- **Falso positivo descartado**: un `diff -r` inicial reportó 305 «diferencias» que eran consecuencia
  de seguir **symlinks absolutos** de BusyBox; con `--no-dereference` el árbol es limpio.
- **Conclusión**: el **único** no-determinismo son los **mtimes** ⇒ normalizarlos cierra la brecha.

## What Changes

Todo el arreglo vive en **`.github/workflows/image.yml`**; el `Dockerfile`, `compose.yml` y la
aplicación **no se tocan**. Tres puntos:

- **`SOURCE_DATE_EPOCH` = timestamp del commit `HEAD`**: un paso **previo** al build que hace
  `git log -1 --pretty=%ct` y lo vuelca a `$GITHUB_ENV`, siguiendo el **patrón oficial de Docker** para
  builds reproducibles en GitHub Actions.
- **Exportar con `rewrite-timestamp`**: sustituir `--load` por
  `--output type=docker,rewrite-timestamp=true` (`--load` **es exactamente** el exportador
  `type=docker`; se necesita además `rewrite-timestamp` porque `SOURCE_DATE_EPOCH` **solo**, en ese
  exportador, **no toca las fechas de las capas**).
- **Evidencia auditable por run**: imprimir el id (y tamaño) de la imagen construida
  (`docker image inspect --format '{{.Id}}' apimail:ci`) — **trazabilidad**, no lógica.

## Non-Goals (explícitos)

- **No** se hacen reproducibles los builds **locales** (`podman compose up --build`: es **otro motor**,
  buildah, y compose no pasa estas banderas).
- **No** se elimina el **build redundante** por release (~3 min: `main` y el tag construyen el mismo
  commit — sigue igual, ahora **inofensivo**).
- **No** se fija la versión de **BuildKit** ni `compatibility-version` (la asamblea afecta al digest
  **entre** versiones de BuildKit).
- **No** se fijan los **digests de las bases** (el `Dockerfile` las referencia por **tag** → el mismo
  commit daría otro digest si un tag base se mueve) ni se **impone** la reproducibilidad con un paso
  que falle al divergir. Ambas son limitaciones reconocidas (ver `design.md`); la segunda queda como
  **seguimiento**.

## Capabilities

### New Capabilities
<!-- Ninguna: cambio de infraestructura de CI, sin comportamiento observable (§ skip_specs). -->

### Modified Capabilities
<!-- Ninguna. -->

## Impact

- Modificado: **solo `.github/workflows/image.yml`**.
- **No se tocan**: `Dockerfile`, `compose.yml`, la aplicación (`src/`) ni ninguna spec.
- **Sin secretos nuevos**: `git log` y `docker image inspect` son locales; nada que autenticar.
- Efecto: el **mismo commit** produce **el mismo digest** en CI ⇒ `latest` y `vX.Y.Z` del mismo commit
  dejan de divergir, y el build redundante por release queda **inofensivo** (misma salida).
- `skip_specs: true` en el change: `image.yml` no tiene spec asociada.
- **Limitación reconocida**: la equivalencia `latest ≡ vX.Y.Z` tras una release **no se re-verifica
  hasta la próxima** (no se fuerza una release para probarlo). Lo que se prueba ahora es «un commit ⇒
  un digest» con **dos `workflow_dispatch`**, y la inferencia se documenta tal cual.
