# Tasks

## 1. Reproducir los dos defectos (evidencia)

- [x] 1.1 `git show origin/main:Cargo.lock | grep -A2 'name = "apimail"'` muestra `0.1.0`
  mientras `git show origin/main:Cargo.toml | grep '^version'` muestra `0.2.0`; y `cargo test`
  reescribe `Cargo.lock` (aparece como modificado en `git status`). Anotar la evidencia.
- [x] 1.2 `cargo package --list | wc -l` (≈142 ficheros) y
  `cargo package --list | grep -c '^openspec/'` (≠0): el paquete incluye la documentación de
  proceso. Anotar la evidencia.

## 2. Corregir

- [x] 2.1 `Cargo.toml`: añadir `exclude` a `[package]` con `/openspec`, `/plans`, `/.opencode`,
  `/.github`, `/CHANGELOG.md`, `/AGENTS.md`, `/GIT_FLOW.md`, `/cliff.toml`, `/.justfile` y
  `/.vampus.yml`.
- [x] 2.2 `.github/workflows/release-prepare.yml`: añadir, **después** del paso
  «Bump version with vampus» y **antes** de «Commit, tag and push», un paso
  `cargo update --workspace` (nombre p. ej. «Sync Cargo.lock with the new version»).
- [x] 2.3 Sincronizar el lock actual: `cargo update --workspace` y comprobar con
  `git diff Cargo.lock` que el único cambio es la versión de `apimail` (`0.1.0` → `0.2.0`).

## 3. Verificar

- [x] 3.1 `cargo package --list` no contiene ninguna ruta de `openspec/`, `plans/`, `.github/`,
  `CHANGELOG.md`, `AGENTS.md`, `GIT_FLOW.md`, `cliff.toml`, `.justfile` ni `.vampus.yml`, y sí
  `src/`, `tests/`, `Cargo.toml`, `README.md` y `LICENSE`.
- [x] 3.2 `cargo publish --dry-run` termina en verde.
- [x] 3.3 Simular el paso del workflow en un directorio temporal (copiar `Cargo.toml` +
  `.vampus.yml`, `vampus upgrade --minor`, `cargo update --workspace`) y comprobar que la versión
  del lock queda igual que la de `Cargo.toml`.
- [x] 3.4 `just test`, `just clippy` y `just fmt` en verde.
- [x] 3.5 Revisar el YAML del workflow (y con `actionlint` si está disponible).

## 4. Cierre

- [x] 4.1 Revisar con `rust-reviewer` (foco: el paso de CI es correcto, el `exclude` no deja
  fuera nada necesario y los criterios se comprueban por CLI).
- [x] 4.2 Marcar tareas, `openspec validate release-hygiene --strict` y
  `openspec archive release-hygiene`.
- [x] 4.3 PR `fix/release-hygiene` → `development`.
