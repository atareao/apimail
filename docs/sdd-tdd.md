# Flujo SDD + TDD — texto completo (referencia literal)

## Flujo SDD + TDD (obligatorio)

Antes de escribir/editar código, `just check-spec` debe confirmar un change proposal aprobado en `openspec/changes/<feature>/`. Si no existe, detente y pide aprobación; no escribas código. Tras aprobar la spec: **RED → GREEN → REFACTOR**, verificando por CLI con `just check-all` (los cuatro pasos del CI; `just test`, `just clippy`, `just fmt` si quieres ir paso a paso). Nunca asumas que los tests compilan o pasan sin ejecutarlos.

⚠️ **`openspec` 1.13.2 no tiene `openspec new change`** (ese subcomando no existe: `openspec change` solo ofrece `show`, `list` y `validate`). El change se crea **a mano**: el directorio `openspec/changes/<feature>/` con su `.openspec.yaml` (`schema: spec-driven`, `created`, `goal`) más `proposal.md`, `design.md`, `tasks.md` y `specs/<capability>/spec.md`. Plantillas: `openspec templates`.

Artefactos OpenSpec: `proposal.md` → `specs/<capability>/spec.md` → `design.md` → `tasks.md` (headings/`SHALL` en inglés, prosa en español). Los `## Requirement`/`#### Scenario` del delta deben copiar EXACTAMENTE el header de la spec destino, o `openspec archive` falla. Consolida con `openspec archive <feature>`; las specs resultantes viven en `openspec/specs/<capability>/`.
