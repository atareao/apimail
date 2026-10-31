# Convenciones (referencia literal)

## Convenciones
- Tests unitarios en el mismo archivo bajo `#[cfg(test)]`; tests de integración/API en `tests/`.
- Prefiere lógica inyectable y testeable sin estado global (p. ej. `Config::from_lookup`) frente a mutar variables de entorno (que en edition 2024 exige `unsafe`).
- `cargo clippy --all-targets -- -D warnings` es el estándar: el REFACTOR debe dejar cero warnings (limpia variantes/código muerto que aparezcan en cascada al eliminar funcionalidad).
