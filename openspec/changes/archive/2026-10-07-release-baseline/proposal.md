# Proposal

## Why

`release-prepare.yml` ejecuta `vampus upgrade --minor` ante commits `feat`. Con la base en `0.1.0`, la primera release (al fusionar el PR `development` → `main`) quedaría en `v0.2.0`. Se quiere que la primera release sea `v0.1.0`.

## What Changes

- Bajar la versión base a `0.0.0` en `.vampus.yml` (`current_version`) y `Cargo.toml` (`version`).
- Sin cambios de comportamiento ni de API.

No hay **BREAKING**.

## Capabilities

### New Capabilities
<!-- Ninguna (§ skip_specs). -->

### Modified Capabilities
<!-- Ninguna. -->

## Impact

- `.vampus.yml`, `Cargo.toml`.
- El endpoint `GET /api/health` reportará `0.0.0` hasta la release; el test usa `env!("CARGO_PKG_VERSION")`, por lo que se adapta solo.
