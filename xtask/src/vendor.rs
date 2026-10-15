//! `cargo xtask vendor`: refreshes `reference/` from an `openscreeps/openapi` checkout.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::Error;

/// Composes the standalone document named by `$COMPOSE` (`http` or `socket`) with the
/// checkout's own contract library, so the vendored document is exactly what the reference
/// build validates, written as the build writes its documents.
const COMPOSE: &str = "
import { readFileSync } from 'node:fs';
import { composeProfile, composeSocketProfile } from './lib/contracts.mjs';
const read = (name) => JSON.parse(readFileSync(name, 'utf8'));
const profile = read('profiles/standalone.json');
const composed =
  process.env.COMPOSE === 'socket'
    ? composeSocketProfile(read('asyncapi.json'), profile)
    : composeProfile(read('openapi.json'), profile);
process.stdout.write(JSON.stringify(composed, null, 2) + '\\n');
";

/// Copies `openapi.json` and `asyncapi.json`, writes the composed `standalone.openapi.json` and
/// `standalone.asyncapi.json` into `reference/`, and records the checkout's commit in
/// `reference/SOURCE`.
pub fn run(root: &Path, checkout: &Path) -> Result<(), Error> {
    let reference = root.join("reference");
    let mut documents = Vec::new();
    for (official, composed, kind) in [
        ("openapi.json", "standalone.openapi.json", "http"),
        ("asyncapi.json", "standalone.asyncapi.json", "socket"),
    ] {
        let path = checkout.join(official);
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        documents.push((official, text));
        documents.push((composed, compose(checkout, kind)?));
    }
    // Every document must parse, or code generation would fail later with a vaguer error.
    for (name, text) in &documents {
        serde_json::from_str::<serde_json::Value>(text)
            .map_err(|error| format!("{name}: {error}"))?;
    }
    let source = source(checkout)?;
    fs::create_dir_all(&reference)?;
    for (name, text) in documents {
        fs::write(reference.join(name), text)?;
    }
    fs::write(reference.join("SOURCE"), format!("{source}\n"))?;
    println!("Vendored {} ({source}) into reference/", checkout.display());
    if source.ends_with(DIRTY) {
        println!(
            "warning: the checkout has uncommitted changes; vendor again once they are pushed"
        );
    }
    Ok(())
}

/// Marks a recorded commit whose checkout had uncommitted changes.
const DIRTY: &str = " with uncommitted changes";

/// `openscreeps/openapi <commit>`, followed by [`DIRTY`] when the checkout differs from it.
fn source(checkout: &Path) -> Result<String, Error> {
    let commit = git(checkout, &["rev-parse", "HEAD"])?;
    let dirty = !git(checkout, &["status", "--porcelain"])?.is_empty();
    Ok(format!(
        "openscreeps/openapi {}{}",
        commit.trim(),
        if dirty { DIRTY } else { "" }
    ))
}

/// Runs git in the checkout and returns its standard output.
fn git(checkout: &Path, arguments: &[&str]) -> Result<String, Error> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(checkout)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed in {}:\n{}",
            arguments.join(" "),
            checkout.display(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// The composed standalone document of `kind` (`http` or `socket`).
fn compose(checkout: &Path, kind: &str) -> Result<String, Error> {
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", COMPOSE])
        .env("COMPOSE", kind)
        .current_dir(checkout)
        .output()
        .map_err(|error| format!("cannot run node: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "composing the standalone {kind} profile failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}
