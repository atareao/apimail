# Tasks

## 1. Baseline de versión

- [x] 1.1 Bajar `current_version` a `0.0.0` en `.vampus.yml` y `version` a `0.0.0` en `Cargo.toml`; verificar con `vampus preview --minor` que el resultado es `0.1.0`.
- [x] 1.2 Ejecutar `cargo test`, `cargo clippy --all-targets -- -D warnings` y `cargo fmt --check` en verde.
