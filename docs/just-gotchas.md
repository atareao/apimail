# Gotchas del task runner (`just`) — texto completo (referencia literal)

## Gotchas del task runner (`just`)
- El `.justfile` está **adaptado a apimail**: las recetas de contenedor y salud usan `ghcr.io/atareao/apimail` y el `compose.yml` real (existen `Dockerfile`, `.dockerignore`, `.env.example` y `.env.j2`), y la versión de la imagen se resuelve con `vampus show` (`vampus` instalado + `.vampus.yml`). Las recetas heredadas `frontend-*` se **retiraron** (el proyecto no tiene `frontend/`). `dev`/`build`/`push`/`deploy*` usan **podman** (instalado aquí); `dev-docker` requiere **docker** (no instalado en esta máquina).
- Recetas fiables hoy: `just test`, `just clippy`, `just fmt`, `just doc`, `just check-spec`,
  `just check-all` (encadena **los cuatro** pasos del CI: `fmt`, `clippy`, `doc`, `test`) y
  `just check-all-mcp` (los mismos cuatro con `--features mcp`, paridad con el job `test-mcp`).
- ⚠️ **`just check-all` ya cubre `cargo doc`**, así que úsalo en lugar de encadenar `fmt`+`clippy`+`test`
  a mano: sin el paso de `doc` el árbol parece verde y el CI falla por un warning del rustdoc
  (un enlace roto), que es justo lo que pasó una vez.
- `just check-spec` **fallará** mientras no haya un change activo en `openspec/changes/` — es lo esperado, no un fallo del repo. El mensaje de error explica cómo crearlo a mano.
