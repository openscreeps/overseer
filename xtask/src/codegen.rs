//! `cargo xtask codegen`: the typed HTTP and socket APIs of `overseer-core` from `reference/`.
//!
//! Reads the official documents (`reference/openapi.json`, `reference/asyncapi.json`) and the
//! composed standalone documents (`reference/standalone.openapi.json`,
//! `reference/standalone.asyncapi.json`), merges each pair's schemas so messages of both kinds
//! of server deserialize, and writes `api/types.rs`, `api/endpoints.rs`,
//! `api/endpoints/tests.rs` and `socket/types.rs`, formatted with rustfmt. Socket schemas the
//! HTTP document defines identically are the HTTP types.

mod docs;
mod emit;
mod merge;
mod model;
mod names;
mod ops;
mod sample;
mod socket;
mod union;

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::Error;
use merge::Components;
use model::Model;

/// Generated files, relative to the workspace root.
const TYPES: &str = "crates/overseer-core/src/api/types.rs";
const ENDPOINTS: &str = "crates/overseer-core/src/api/endpoints.rs";
const TESTS: &str = "crates/overseer-core/src/api/endpoints/tests.rs";
const SOCKET_TYPES: &str = "crates/overseer-core/src/socket/types.rs";

/// Generates the files; with `check`, fails when a file on disk differs instead of writing.
pub fn run(root: &Path, check: bool) -> Result<(), Error> {
    let official = read_json(&root.join("reference/openapi.json"))?;
    let standalone = read_json(&root.join("reference/standalone.openapi.json"))?;
    let official_components = components(&official)?;
    let standalone_components = components(&standalone)?;
    let mut model = Model::new(merge::components(
        official_components,
        standalone_components,
    ))?;
    let operations = ops::operations(
        &mut model,
        &official,
        official_components,
        &standalone,
        standalone_components,
    )?;
    let mut names = BTreeSet::new();
    for operation in &operations {
        if !names.insert(operation.name.as_str()) {
            return Err(format!("two operations generate the endpoint {}", operation.name).into());
        }
    }
    model.finish()?;
    let socket_official = read_json(&root.join("reference/asyncapi.json"))?;
    let socket_standalone = read_json(&root.join("reference/standalone.asyncapi.json"))?;
    let socket_components = merge::components(
        components(&socket_official)?,
        components(&socket_standalone)?,
    );
    let options = socket::options(&socket_components, &model.components)?;
    let mut socket_model = Model::with_options(socket_components, options)?;
    socket_model.finish()?;
    let files = [
        (TYPES, emit::types(&model, &emit::API_TYPES)),
        (ENDPOINTS, emit::endpoints(&model, &operations)),
        (TESTS, emit::tests(&model, &operations)),
        (
            SOCKET_TYPES,
            emit::types(&socket_model, &emit::SOCKET_TYPES),
        ),
    ];
    let mut stale = Vec::new();
    for (path, source) in files {
        let formatted = rustfmt(root, &source).map_err(|error| format!("{path}: {error}"))?;
        let target = root.join(path);
        let current = fs::read_to_string(&target).ok();
        if current.as_deref() == Some(formatted.as_str()) {
            continue;
        }
        if check {
            stale.push(path);
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, formatted)?;
            println!("wrote {path}");
        }
    }
    if !stale.is_empty() {
        return Err(format!(
            "stale generated files (run `cargo xtask codegen`): {}",
            stale.join(", ")
        )
        .into());
    }
    println!(
        "{} {} endpoints, {} HTTP types and {} socket types",
        if check { "Verified" } else { "Generated" },
        operations.len(),
        model.defs.len(),
        socket_model.defs.len()
    );
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, Error> {
    let text = fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?)
}

/// The component schemas of a document.
fn components(document: &Value) -> Result<&Components, Error> {
    document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .ok_or_else(|| "document without components.schemas".into())
}

/// Formats `source` with the toolchain's rustfmt, as `cargo fmt` would.
fn rustfmt(root: &Path, source: &str) -> Result<String, Error> {
    let rustfmt = std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let mut child = Command::new(rustfmt)
        .args(["--edition", "2024"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run rustfmt: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("rustfmt has no stdin")?;
    let input = source.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| "rustfmt input thread panicked")??;
    if !output.status.success() {
        return Err(format!(
            "rustfmt failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
