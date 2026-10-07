# Proposal

## Why

El crate aún no puede publicarse en crates.io: `Cargo.toml` carece de `description` y `license`, y el servidor rechaza la subida con `400 missing or empty metadata fields: description, license`. Como `CARGO_REGISTRY_TOKEN` ya está configurado, el job `publish` de `release.yml` fallará en el tag `v0.1.0`.

## What Changes

- Añadir a `[package]` de `Cargo.toml`: `description`, `license = "MIT"`, `repository`, `readme`, `keywords` y `categories`.
- Añadir los ficheros `LICENSE` (texto MIT) y `README.md`.
- Sin cambios de comportamiento ni de API.

No hay **BREAKING**.

## Capabilities

### New Capabilities
<!-- Ninguna: cambio de empaquetado, sin comportamiento observable (§ skip_specs). -->

### Modified Capabilities
<!-- Ninguna. -->

## Impact

- `Cargo.toml` (metadatos), `LICENSE`, `README.md`.
- Desbloquea `cargo publish` (job `publish` de `release.yml`).
- `skip_specs: true` en el change: no cambia ninguna spec.
