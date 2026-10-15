# Vendored API reference

`openapi.json` (HTTP) and `asyncapi.json` (socket) are copies of the documents of the same names
from [openscreeps/openapi](https://github.com/openscreeps/openapi); `standalone.openapi.json` and
`standalone.asyncapi.json` are those documents composed with the reference's standalone profile
(`profiles/standalone.json`) by its own `composeProfile` and `composeSocketProfile`
(`lib/contracts.mjs`).

Refresh all four from a checkout (default `../openapi`; needs `node`), then regenerate the typed
API:

```sh
cargo xtask vendor [<openapi checkout>]
cargo xtask codegen
```

`cargo xtask codegen --check` (part of `mise run ci` and CI) fails when the generated files in
`crates/overseer-core/src/api/` and `crates/overseer-core/src/socket/types.rs` are stale. Do not
edit these documents or the generated files by hand.
