//! `cargo xtask vendor`: refreshes `reference/` from an `openscreeps/openapi` checkout.

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value};

use crate::Error;

/// Prints, as one JSON object, every vendored document's file name (relative to `reference/`)
/// mapped to its text: the official documents as the checkout has them, and the documents the
/// checkout's own contract library composes from them, so each vendored document is exactly
/// what the reference build validates, written as the build writes its documents. The profiles
/// are composed with `composeProfile` and `composeSocketProfile`; every plugin of `plugins/`
/// is composed alone on the composed community document with `composePlugin`.
const COMPOSE: &str = "
import { readdirSync, readFileSync } from 'node:fs';
import { composePlugin, composeProfile, composeSocketProfile } from './lib/contracts.mjs';
const text = (name) => readFileSync(name, 'utf8');
const read = (name) => JSON.parse(text(name));
const write = (document) => JSON.stringify(document, null, 2) + '\\n';
const official = read('openapi.json');
const socket = read('asyncapi.json');
const documents = { 'openapi.json': text('openapi.json'), 'asyncapi.json': text('asyncapi.json') };
for (const [profile, prefix] of [['standalone', 'openscreeps'], ['community', 'community']]) {
  const contract = read(`profiles/${profile}.json`);
  documents[`${prefix}.openapi.json`] = write(composeProfile(official, contract));
  documents[`${prefix}.asyncapi.json`] = write(composeSocketProfile(socket, contract));
}
const community = composeProfile(official, read('profiles/community.json'));
for (const file of readdirSync('plugins').filter((file) => file.endsWith('.json')).sort()) {
  const plugin = read(`plugins/${file}`);
  const name = file.slice(0, -'.json'.length);
  documents[`plugins/${name}.openapi.json`] = write(composePlugin(official, community, plugin));
}
process.stdout.write(JSON.stringify(documents));
";

/// Writes into `reference/` the official `openapi.json` and `asyncapi.json`, the composed
/// `OpenScreeps` (`openscreeps.*`) and community (`community.*`) documents and one composed
/// document per plugin (`plugins/<plugin>.openapi.json`), replacing the plugin documents
/// vendored before, and records the checkout's commit in `reference/SOURCE`.
pub fn run(root: &Path, checkout: &Path) -> Result<(), Error> {
    let reference = root.join("reference");
    let documents = compose(checkout)?;
    // Every document must parse, or code generation would fail later with a vaguer error.
    for (name, text) in &documents {
        serde_json::from_str::<Value>(text).map_err(|error| format!("{name}: {error}"))?;
    }
    let source = source(checkout)?;
    let plugins = reference.join("plugins");
    if plugins.exists() {
        fs::remove_dir_all(&plugins)?;
    }
    fs::create_dir_all(&plugins)?;
    for (name, text) in &documents {
        fs::write(reference.join(name), text)?;
    }
    fs::write(reference.join("SOURCE"), format!("{source}\n"))?;
    println!(
        "Vendored {} documents from {} ({source}) into reference/",
        documents.len(),
        checkout.display()
    );
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

/// Every vendored document's file name (relative to `reference/`) and text, from one run of
/// [`COMPOSE`] in the checkout.
fn compose(checkout: &Path) -> Result<Vec<(String, String)>, Error> {
    let output = Command::new("node")
        .args(["--input-type=module", "--eval", COMPOSE])
        .current_dir(checkout)
        .output()
        .map_err(|error| format!("cannot run node: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "composing the contracts failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let documents: Map<String, Value> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("the composition printed no JSON object: {error}"))?;
    documents
        .into_iter()
        .map(|(name, text)| match text {
            Value::String(text) => Ok((name, text)),
            _ => Err(format!("the composition printed no text for {name}").into()),
        })
        .collect()
}
