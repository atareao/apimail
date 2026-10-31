# El CI de apimail (equivalente local) — referencia literal

### El equivalente al CI son CUATRO cosas, no tres

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets -- -D warnings`
3. `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps`
4. `cargo test`

**`just check-all` encadena esos cuatro pasos, en ese orden** (su `fmt` es `cargo fmt --check`: sin
workspace, idéntico a `cargo fmt --all -- --check`). Una receta que ejecutara solo
`fmt`+`clippy`+`test` daría «verde local» con el CI en rojo por un warning del rustdoc (un enlace
roto, típicamente): `cargo doc` es el único paso que lo detecta, y así fue como se subió un commit
en rojo creyéndolo verde. Por eso está encadenado y no hay que ejecutarlo «a mano» ni componer el
conjunto uno mismo.

Con la feature `mcp` hay **además** un segundo juego completo en el job `test-mcp`:
`clippy`/`doc`/`test` con `--features mcp` (y el mismo `fmt`, que no depende de la feature).
**`just check-all-mcp` lo reproduce entero.** Prueba **ambos modos** antes de dar algo por terminado.
