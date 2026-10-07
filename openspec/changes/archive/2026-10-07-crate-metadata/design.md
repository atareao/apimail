# Design

## Context

Ver `proposal.md` — Why. El empaquetado ya es correcto (`cargo publish --dry-run` pasa), pero crates.io exige `description` y `license`/`license-file`, algo que el `--dry-run` no valida porque no sube.

## Goals / Non-Goals

**Goals:**
- Permitir `cargo publish` sin errores de metadatos.
- Dar identidad al crate (licencia, repositorio, docs).

**Non-Goals:**
- Cambiar la API o el comportamiento del servidor.
- Sustituir el CI/CD (ya existe).
- Publicar de verdad (ocurre al crear el tag `v*`).

## Decisions

- **Licencia MIT** (SPDX `MIT` en `Cargo.toml` + texto completo en `LICENSE`): permisiva, estándar del ecosistema, elegida por el usuario.
- **`readme = "README.md"`** con un README mínimo (qué es, build, run, endpoint).
- Campos recomendados por la doc de cargo: `repository`, `keywords`, `categories`.

## Risks / Trade-offs

- [El nombre `apimail` podría no estar disponible] → Verificado hoy: libre (API crates.io `404`, búsqueda `total: 0`).
