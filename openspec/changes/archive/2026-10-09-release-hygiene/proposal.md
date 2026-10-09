# Proposal

## Why

Dos defectos de empaquetado/release detectados tras publicar `v0.2.0`:

1. **`Cargo.lock` desincronizado**: `release-prepare.yml` hace el bump de versión con
   `vampus` y **nunca regenera el lockfile**. Hoy `origin/main` y `origin/development` tienen
   `Cargo.toml` con `version = "0.2.0"` y `Cargo.lock` con `apimail 0.1.0`, así que
   **cualquier** `cargo build`/`cargo test` en un clon recién hecho deja el árbol sucio
   (reproducido: `cargo test` reescribió el lock a `0.2.0`). El tag `v0.2.0` arrastra la
   incoherencia.
2. **Paquete de crates.io inflado**: `cargo publish` empaqueta **142 ficheros / 1,1 MiB**
   (256 KiB comprimidos) porque publica `openspec/` (14 changes archivados + 11 specs),
   `plans/`, `.github/` y artefactos de proceso. `v0.1.0` y `v0.2.0` salieron igual.

## What Changes

- `release-prepare.yml`: tras `vampus upgrade` y **antes** del `git add -A`, regenerar el
  lockfile del workspace (`cargo update --workspace`) para que el commit `chore: release vX.Y.Z`
  incluya un `Cargo.lock` coherente con la nueva versión.
- Sincronizar el `Cargo.lock` actual (`apimail 0.1.0` → `0.2.0`).
- `Cargo.toml`: añadir `exclude` a `[package]` para dejar fuera del paquete publicado los
  artefactos de proceso (`openspec/`, `plans/`, `.opencode/`, `.github/`, `CHANGELOG.md`,
  `AGENTS.md`, `GIT_FLOW.md`, `cliff.toml`, `.justfile`, `.vampus.yml`).

Sin cambios de comportamiento, de API ni de configuración. No hay **BREAKING**.

## Capabilities

### New Capabilities
<!-- Ninguna: cambio de empaquetado y de CI, sin comportamiento observable (§ skip_specs). -->

### Modified Capabilities
<!-- Ninguna. -->

## Impact

- `.github/workflows/release-prepare.yml`, `Cargo.lock`, `Cargo.toml`.
- Efecto: una release publicada deja `git status` limpio en un clon recién hecho y el paquete de
  crates.io solo lleva `src/`, `tests/`, `Cargo.toml`, `README.md` y `LICENSE` (además del
  `Cargo.lock` que Cargo incluye porque el paquete tiene binario).
- `skip_specs: true` en el change: no cambia ninguna spec.
- Se publica como **patch** (`v0.2.1`).

Alineado con `plans/PLAN-001.md`, ítems **#1** (`release-lockfile`) y **#2** (`packaging`).
