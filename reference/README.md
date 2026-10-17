# Vendored API reference

Contracts of [openscreeps/openapi](https://github.com/openscreeps/openapi), one HTTP and one
socket document each:

- `openapi.json` (HTTP) and `asyncapi.json` (socket): the official contract (screeps.com), copies
  of the documents of the same names.
- `openscreeps.openapi.json` and `openscreeps.asyncapi.json`: the OpenScreeps contract, those
  documents composed with the reference's standalone profile (`profiles/standalone.json`) by its
  own `composeProfile` and `composeSocketProfile` (`lib/contracts.mjs`).
- `community.openapi.json` and `community.asyncapi.json`: the community contract (the stock
  Screeps server without mods), composed the same way with `profiles/community.json`.
- `plugins/<plugin>.openapi.json`: one per plugin contract of the reference (`plugins/*.json`,
  community server mods), each composed alone on the composed community document by the
  reference's `composePlugin`. Plugin contracts are HTTP only.

`SOURCE` records the reference commit they were vendored from.

Refresh them all from a checkout (default `../openapi`; needs `node`), then regenerate the typed
API:

```sh
cargo xtask vendor [<openapi checkout>]
cargo xtask codegen
```

`cargo xtask codegen --check` (part of `mise run ci` and CI) fails when the generated files in
`crates/overseer-core/src/api/` and `crates/overseer-core/src/socket/` are stale. Do not edit
these documents or the generated files by hand.
