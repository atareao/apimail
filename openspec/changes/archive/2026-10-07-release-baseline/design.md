# Design

## Context

Ver `proposal.md` — Why. `vampus preview --minor` sobre `0.1.0` da `0.2.0`; partiendo de `0.0.0` daría `0.1.0`.

## Goals / Non-Goals

**Goals:** que la primera release sea `v0.1.0`.
**Non-Goals:** cambiar el CI/CD ni el comportamiento del servidor.

## Decisions

- **Base `0.0.0`** en `.vampus.yml` y `Cargo.toml`: convención habitual de "sin release todavía"; el primer bump minor produce `0.1.0`.
- Alternativas descartadas: crear el tag `v0.1.0` a mano (se salta el flujo automático); aceptar `v0.2.0` (no deseado).

## Risks / Trade-offs

- [El README muestra `0.1.0` en el ejemplo de `/api/health`] → Tras la release real el valor vuelve a `0.1.0`; desajuste temporal aceptado.
