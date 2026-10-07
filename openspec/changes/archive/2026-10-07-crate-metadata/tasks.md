# Tasks

## 1. Metadatos del crate

- [x] 1.1 Añadir a `[package]` de `Cargo.toml` los campos `description`, `license = "MIT"`, `repository`, `readme`, `keywords` y `categories`; verificar con `cargo publish --dry-run` que desaparece el warning de metadatos.
- [x] 1.2 Añadir `LICENSE` (texto MIT) y `README.md`; verificar con `cargo publish --dry-run` que ambos se empaquetan y no hay warnings de ficheros.
- [x] 1.3 Ejecutar `cargo test`, `cargo clippy --all-targets -- -D warnings` y `cargo fmt --check` en verde.
