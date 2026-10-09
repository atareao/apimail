# Design

## Context

- `release-prepare.yml` (push a `main`) calcula el bump desde los conventional commits, ejecuta
  `vampus upgrade --minor|--patch|--major` (que solo reemplaza patrones en `Cargo.toml` vía
  `.vampus.yml`), genera el CHANGELOG con `git-cliff` y hace `git add -A` + commit
  `chore: release vX.Y.Z` + tag + push.
- `vampus` no toca `Cargo.lock` y **no** puede hacerlo con seguridad: su mecanismo es reemplazo
  de patrones por línea, y la línea `version = "0.1.0"` del lock aparece también en otros
  `[[package]]`, así que un patrón suelto los corrompería.
- `cargo update --workspace` (`-w`, "Only update the workspace packages"; verificado en cargo
  1.97) refresca **solo** los paquetes del workspace: reescribe la entrada `apimail` del lock sin
  tocar el resto de dependencias.
- `cargo package`/`cargo publish` respetan `include`/`exclude` de `[package]` con semántica tipo
  gitignore relativa a la raíz del paquete. `Cargo.toml` y el `readme`/`license-file`
  declarados se incluyen siempre.
- La verificación del paquete reveló un artefacto de proceso no previsto: `.opencode/`
  (12 ficheros de comandos y skills de agentes). Entra dentro del *Goal* «el paquete
  publicado no contendrá documentación de proceso ni CI».

## Goals / Non-Goals

**Goals**
- Que el commit de release incluya un `Cargo.lock` coherente con `Cargo.toml`.
- Que el paquete publicado no contenga documentación de proceso ni CI.
- Verificar ambos puntos con comandos, no "a ojo".

**Non-Goals**
- No se toca `release.yml` (build/publish) ni el mecanismo de bump.
- No se cambia la versión ni el CHANGELOG ni el esquema de ramas.
- No se añade `include` restrictivo: se prefiere `exclude`, que ante una omisión publica de más
  en lugar de romper el paquete.

## Decisions

1. **Dónde sincronizar el lock**: en `release-prepare.yml`, como paso nuevo *después* de
   `vampus upgrade` y *antes* del `git add -A` (que ya stagea todo, así que el lock entra en el
   mismo commit). Comando: `cargo update --workspace`.
2. **Por qué `cargo update --workspace`**: es la forma mínima y explícita de refrescar la versión
   del propio paquete en el lock. Descartadas: `cargo check`/`cargo fetch` (regeneran el lock
   como efecto colateral implícito) y un `replaces` de vampus sobre `Cargo.lock` (peligroso, ver
   Context).
3. **Exclusiones del paquete**: `exclude` en `[package]` con las rutas de proceso:
   `/openspec`, `/plans`, `/.opencode`, `/.github`, `/CHANGELOG.md`, `/AGENTS.md`,
   `/GIT_FLOW.md`, `/cliff.toml`, `/.justfile`, `/.vampus.yml`. **No** se excluyen `README.md`
   ni `LICENSE` (deben ir en el paquete).
4. **Verificación local del workflow**: no se puede ejecutar GitHub Actions en local, así que la
   lógica del paso se reproduce en un directorio temporal (copiar `Cargo.toml` + `.vampus.yml`,
   `vampus upgrade --minor`, `cargo update --workspace`) y se comprueba que el lock queda en la
   versión nueva. El YAML del workflow se revisa (y con `actionlint` si está disponible).

## Risks / Trade-offs

- *Riesgo*: `cargo update --workspace` podría actualizar algo más de lo esperado.
  *Mitigación*: `-w` restringe al workspace; se verifica con `git diff Cargo.lock` que el único
  cambio sea la versión de `apimail`.
- *Riesgo*: una exclusión demasiado agresiva rompería el paquete. *Mitigación*: se excluye solo
  documentación/CI (nunca `src/`/`tests/`), se usa `exclude` y se valida con
  `cargo package --list` + `cargo publish --dry-run`.
- *Trade-off*: el paquete seguirá llevando `Cargo.lock` (Cargo lo incluye porque hay un binario).
  Es esperado y deseable.

## Migration Plan

- Aditivo y de bajo riesgo: un paso de CI y un `exclude`. Sin migración de datos.
- Rollback: revertir la rama `fix/release-hygiene`.
