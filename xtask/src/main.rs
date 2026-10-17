//! Repository automation, run as `cargo xtask <command>`:
//!
//! - `vendor [<openapi checkout>]` copies `openapi.json` and `asyncapi.json` from the
//!   `openscreeps/openapi` checkout (default `../openapi`) into `reference/` and composes
//!   `reference/standalone.openapi.json` and `reference/standalone.asyncapi.json` from them and
//!   the standalone profile with the checkout's `composeProfile` and `composeSocketProfile`
//!   (needs `node`).
//! - `codegen [--check]` generates the typed HTTP and socket APIs of `overseer-core` from
//!   `reference/`; `--check` fails instead of writing when the generated files are stale.
//! - `web-demo` builds the browser demo (`web/demo`) into the static bundle
//!   `target/web-demo/` (needs the `wasm-bindgen` CLI matching the locked crate version).
//! - `e2e [--target openscreeps|official|community] [--rust-only | --ui-only | --desktop]
//!   [--screens[=<set>]] [--no-build] [-- <playwright args>]` runs the live feature tests and the
//!   Playwright UI tests against a disposable `openscreeps/server` world in Docker (the server
//!   image is built from the commit in `e2e/server.lock`), with `--target community` against a
//!   disposable world of the community private server in Docker (built from `e2e/community/`)
//!   or, with `--target official`, their read-only subset against screeps.com with
//!   `SCREEPS_TOKEN`; `--desktop` runs the desktop app's suite instead (Windows); see `e2e.rs`.

mod codegen;
mod e2e;
mod vendor;
mod web_demo;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Errors are reported as text; every command is a one-shot developer tool.
type Error = Box<dyn std::error::Error>;

const USAGE: &str = "usage: cargo xtask codegen [--check] | cargo xtask vendor [<openapi checkout>] | cargo xtask web-demo | cargo xtask e2e [--target openscreeps|official|community] [--rust-only | --ui-only | --desktop] [--screens[=<set>]] [--no-build] [-- <playwright args>]";

fn main() -> ExitCode {
    let root = workspace_root();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["codegen"] => codegen::run(&root, false),
        ["codegen", "--check"] => codegen::run(&root, true),
        ["vendor"] => vendor::run(&root, &root.join("../openapi")),
        ["vendor", checkout] => vendor::run(&root, Path::new(checkout)),
        ["web-demo"] => web_demo::run(&root),
        ["e2e", options @ ..] => e2e::run(&root, options),
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The workspace root: the parent of this crate's manifest directory.
fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_path_buf()
}
