# Design

## Context

- `rustdoc::broken_intra_doc_links` es un lint del **compilador de documentación**, no de clippy.
  Por eso `just clippy` (`cargo clippy --all-targets -- -D warnings`) pasa con 0 warnings aunque
  `cargo doc` avise: clippy nunca compila los comentarios `//!`/`///`, así que los enlaces rotos
  solo aparecen al construir la documentación.
- docs.rs construye el crate publicado con la toolchain estable y publica la página con esos 19
  avisos de rustdoc. El defecto es visible para cualquiera que abra la doc del crate, pero
  invisible para el CI actual.
- Los 16 enlaces apuntan a items **privados** (`fn require_api_key`, `fn build_queue`,
  `fn quote_search_string`, `fn tls_strategy`, `Self::ensure_session`, `Self::run_once`): en
  rustdoc, un enlace a un item no público no se puede resolver en la documentación pública.
- Los 3 enlaces sin resolver (`Imap`, `TooLarge`, `Unparsable` en `src/imap.rs`) apuntan a
  rutas que rustdoc no encuentra desde ese punto del crate.

## Goals / Non-Goals

**Goals**
- `cargo doc --no-deps` termina con **0 warnings**.
- `AGENTS.md` describe el estado real del crate (9 capabilities, 12 ficheros de test).
- Un paso de CI impide que los warnings de rustdoc vuelvan.

**Non-Goals**
- No se añade `#![warn(missing_docs)]` ni se persigue cobertura total de documentación.
- No se toca ninguna firma, tipo, lógica ni test.
- No se cambia la API ni la configuración del crate.

## Decisions

1. **Degradar los enlaces a items privados a texto plano**, en vez de exponerlos con
   `--document-private-items`. No queremos publicar la documentación de items que **no** forman
   parte de la API pública: hacerlo ensuciaría docs.rs con helpers internos y ampliaría la
   superficie que parece soportada. El enlace pierde el hipervínculo, pero el nombre se conserva
   como `` `nombre` `` para que el lector sepa de qué helper se habla.
2. **Resolver o degradar los 3 enlaces sin resolver** de `src/imap.rs`: si la ruta correcta es
   pública, se corrige el enlace; si el destino es privado, se aplica la misma regla que en 1.
3. **Guarda en `.github/workflows/ci.yml`**, el workflow que ya corre en cada PR a `main` y
   `development`, con un paso `cargo doc --no-deps` y `RUSTDOCFLAGS="-D warnings"`. Se elige ese
   workflow (y no `release.yml`) porque es donde se verifica el código antes de mezclar, de modo
   que la regresión se corta en el PR y no al publicar.

## Alternatives Considered

- **`#![warn(missing_docs)]`**: descartado. Queda fuera de alcance; ya existe documentación
  sustancial y el objetivo es el lint de **enlaces**, no la cobertura de documentación. Activar
  ese lint generaría otra tanda de warnings de naturaleza distinta y confundiría el propósito de
  este change.
- **`--document-private-items` en docs.rs**: descartado por la razón de la Decisión 1 (publicaría
  doc de items no soportados).

## Risks / Trade-offs

- *Riesgo*: `cargo doc` con `-D warnings` puede volverse frágil si futuras versiones de rustdoc
  añaden lints nuevos (p. ej. cambios en el grupo `rustdoc::` o en `rustdoc::broken_intra_doc_links`).
  *Impacto*: un PR futuro podría fallar por un warning nuevo ajeno a este cambio.
  *Mitigación/aceptación*: se acepta; el mensaje de rustdoc es claro y el arreglo (normalmente
  degradar o corregir un enlace) es trivial.
  *Detección y respuesta*: el propio paso de CI falla con el lint identificado en el log; se
  corrige el enlace señalado o, si el lint es un falso positivo nuevo, se ajusta
  `RUSTDOCFLAGS`/`#[allow(...)]` puntual en el change correspondiente.
- *Trade-off*: degradar a texto plano pierde navegabilidad para helpers internos. Es aceptable:
  esos items no son API pública y no deberían ser enlazables desde la doc publicada.

## Migration Plan

- Aditivo y de bajo riesgo: comentarios de documentación, un fichero de texto y un paso de CI.
  Sin migración de datos.
- Rollback: revertir la rama `docs/rustdoc-hygiene`.

## Verification

- `cargo doc --no-deps` → **0 warnings**.
- `just test` → **311** tests en verde.
- `just clippy` → 0 warnings.
- `just fmt` → sin cambios pendientes.
