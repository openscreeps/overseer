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
cargo clippy -p overseer-net -p overseer -p overseer-web-demo -p overseer-view -p overseer-app --target wasm32-unknown-unknown -- -D warnings
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

## Checks

Make sure that all these checks pass before you report a change as complete:

```sh
cargo fmt --all -- --check
cargo xtask codegen --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo check -p overseer-core --target wasm32-unknown-unknown
cargo clippy -p overseer-net -p overseer -p overseer-web-demo -p overseer-view -p overseer-app --target wasm32-unknown-unknown -- -D warnings
```

`mise run ci` runs the same checks.
