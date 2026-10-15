//! `cargo xtask web-demo`: builds the browser demo (`web/demo`) into the static bundle
//! `target/web-demo/` (`index.html`, the `wasm-bindgen` JavaScript glue and the module).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Error;

/// The demo's package and the file name of its compiled module.
const PACKAGE: &str = "overseer-web-demo";
const MODULE: &str = "overseer_web_demo.wasm";
const TARGET: &str = "wasm32-unknown-unknown";

/// Builds the demo for `wasm32-unknown-unknown` (release), generates its JavaScript bindings
/// with the `wasm-bindgen` CLI (`--target web`) and copies `index.html` next to them.
pub fn run(root: &Path) -> Result<(), Error> {
    let version = locked_version(root, "wasm-bindgen")?;
    check_cli(&version)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut build = Command::new(cargo);
    build
        .args([
            "build",
            "--release",
            "--package",
            PACKAGE,
            "--target",
            TARGET,
        ])
        .current_dir(root);
    status(&mut build, "cargo build")?;
    let target = target_dir(root);
    let bundle = target.join("web-demo");
    let mut bindgen = Command::new("wasm-bindgen");
    bindgen
        .args(["--target", "web", "--no-typescript", "--out-dir"])
        .arg(&bundle)
        .arg(target.join(TARGET).join("release").join(MODULE));
    status(&mut bindgen, "wasm-bindgen")?;
    fs::copy(root.join("web/demo/index.html"), bundle.join("index.html"))?;
    println!(
        "Built the web demo into {}; serve it over HTTP, for example:\n  python -m http.server 8000 --directory {}",
        bundle.display(),
        bundle.display()
    );
    Ok(())
}

/// The version of `package` in `Cargo.lock`.
fn locked_version(root: &Path, package: &str) -> Result<String, Error> {
    let lock = fs::read_to_string(root.join("Cargo.lock"))?;
    let name = format!("name = \"{package}\"");
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == name
            && let Some(version) = lines
                .next()
                .and_then(|line| line.strip_prefix("version = \""))
                .and_then(|rest| rest.strip_suffix('"'))
        {
            return Ok(version.to_owned());
        }
    }
    Err(format!("{package} is not in Cargo.lock").into())
}

/// Fails unless the `wasm-bindgen` CLI has `version`: the CLI and the crate must match.
fn check_cli(version: &str) -> Result<(), Error> {
    let install = format!("cargo install wasm-bindgen-cli --version {version}");
    let output = Command::new("wasm-bindgen")
        .arg("--version")
        .output()
        .map_err(|error| format!("cannot run wasm-bindgen ({error}); install it: {install}"))?;
    let found = String::from_utf8_lossy(&output.stdout);
    if !found.split_whitespace().any(|word| word == version) {
        return Err(format!(
            "wasm-bindgen CLI is `{}`, the crate is {version}; install the match: {install}",
            found.trim()
        )
        .into());
    }
    Ok(())
}

/// Cargo's target directory: `$CARGO_TARGET_DIR`, else `target/` in the workspace.
fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from)
}

/// Runs `command` and fails when it does not succeed.
fn status(command: &mut Command, name: &str) -> Result<(), Error> {
    let status = command
        .status()
        .map_err(|error| format!("cannot run {name}: {error}"))?;
    if !status.success() {
        return Err(format!("{name} failed ({status})").into());
    }
    Ok(())
}
