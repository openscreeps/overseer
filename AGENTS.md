# Agent rules

These rules apply to all agents that change this repository. Obey all rules in all changes.

## Language: ASD-STE100 style

Write all new and changed text in a style like ASD-STE100 Simplified Technical English. This
includes doc comments, code comments, Markdown files and commit messages. Follow the writing
rules below. The STE dictionary does not apply: use any clear English word.

- Keep sentences short: about 20 words for an instruction, about 25 words for a description.
- Write one instruction in each sentence.
- Use the imperative form for instructions.
- Use the active voice.
- Prefer simple tenses.
- Give each word one meaning, and use the same term for the same item each time.
- Use the names from the code for code items (for example `Client`, `deserialize`).
- Use articles ("the", "a") where they make the text clearer.
- Avoid long noun clusters. Break up clusters of more than three words.
- Write short paragraphs with one topic each.
- Use a vertical list for a sequence of steps or for many items.
- Put a warning or a caution before the instruction it applies to.

## Lints: Clippy pedantic

The workspace denies all Clippy `pedantic` lints. `Cargo.toml` sets this in `[workspace.lints]`.

- Keep `[lints] workspace = true` in the `Cargo.toml` file of each crate.
- Do not lower a lint level in a crate or in the code.
- Do not use `#[allow(...)]`. The workspace denies `clippy::allow_attributes`.
- Use `#[expect(lint, reason = "...")]` only if no change to the code can remove the lint.
- Do not add an item to `clippy.toml` to hide a lint.
- Make sure that these commands complete without warnings:

```sh
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p overseer-net -p overseer -p overseer-web-demo -p overseer-view -p overseer-storage -p overseer-app --target wasm32-unknown-unknown -- -D warnings
```

## File size: 1000 lines

- Keep each source file at a maximum of 1000 lines.
- If a change makes a file longer than 1000 lines, split the file into submodules.
- Put each submodule in a new file below the directory of its parent module.
- Split by topic. Do not split at a random line.
- Files that `cargo xtask codegen` writes (`// @generated`) are exempt. Do not edit these files.

Clippy also limits each function to 200 lines (`clippy.toml`).

## Layout: Cargo package layout

Follow the Cargo package layout:
<https://doc.rust-lang.org/cargo/guide/project-layout.html>.

- Put the library root in `src/lib.rs`.
- Put the default executable in `src/main.rs`.
- Put other executables in `src/bin/`.
- Put integration tests in `tests/`.
- Put examples in `examples/`.
- Put benchmarks in `benches/`.
- For a test, example, benchmark or executable with many files, make a directory with a `main.rs`
  file and the modules.
- Use kebab-case names for executables, examples, benchmarks and integration tests.
- Use snake_case names for modules.
- Put a module in `foo.rs` and its child modules in `foo/`. Do not use `mod.rs` files. The
  workspace denies `clippy::mod_module_files`.
- Put each workspace crate in `crates/<crate-name>/`, with the exceptions that exist (`app/`,
  `web/demo/`, `xtask/`).

## CLI safety (`crates/overseer-cli`)

The CLI changes live Screeps accounts. These rules protect the accounts.

- Put the rules of the game and the world geometry in `overseer-core` or `overseer`, for
  example the terrain spot of a structure, room coordinates and cooldowns. Keep only the policy
  of the CLI in `crates/overseer-cli`: its configuration, its protocols with the bot and its
  safety checks, which are stricter than the server.
- A code change does not authorize a live operation. Do not upload, activate a branch, allocate
  CPU, reset (`user/respawn`) or place a spawn on a live server without explicit permission.
- Test mutations only against disposable worlds, for example the world of `cargo xtask e2e`.
  Do not reuse a live server or a persistent Compose volume.
- Keep `poll` read-only without the execution gate (`--execute`).
- Keep the heartbeat freshness, the zero-foothold evidence, the confirmation window, the cooldowns
  and the final revalidation before each reset and each placement.
- Fail closed on missing or malformed safety evidence.
- Never make an API failure into an empty account, an eligible room or permission to reset.
- Use `Error::outcome()` to find the certainty of a mutation. Do not use the HTTP status or the
  message of the server.
- After an unknown placement outcome, lock out the profile. After an unknown reset outcome, start
  the local cooldown.
- Limit the run time and the output of the selector process. Give the selector its arguments
  directly, without a shell. Revalidate its answer.
- Never log or commit credentials. Do not copy credential values into fixtures or documentation.
- Keep the explicit `--env-file` loading, the environment precedence and the redacted errors.
- Never read a secret from the command line. Read it from stdin (a prompt without echo on a
  terminal) or from the environment.
- Never save a password. `overseer login --email` saves an API token where the server makes one,
  else the session token.
- Keep the precedence: credentials in `overseer.toml` win over a saved sign-in, so that CI needs
  no OS keychain.
- Change the saved profiles only through `Storage::update` with a `profiles::Change`. Do not
  overwrite the whole document: the desktop app and the CLI change it at the same time.
- In tests, use `overseer_storage::native::memory::install()` (feature `test-support`) or a
  unique `OVERSEER_APP_IDENTIFIER`, and remove its directory and secrets afterwards. Never use
  the storage of the app identifier `com.openscreeps.overseer`.

## Checks

Make sure that all these checks pass before you report a change as complete:

```sh
cargo fmt --all -- --check
cargo xtask codegen --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p overseer-core --target wasm32-unknown-unknown
cargo clippy -p overseer-net -p overseer -p overseer-web-demo -p overseer-view -p overseer-storage -p overseer-app --target wasm32-unknown-unknown -- -D warnings
```

`mise run ci` runs the same checks.
