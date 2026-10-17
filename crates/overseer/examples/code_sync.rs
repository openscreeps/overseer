//! Syncs a code branch with a local directory in the official desktop client's layout
//! (`<module>.js` and `<module>.wasm` files).
//!
//! ```sh
//! cargo run -p overseer --example code_sync -- pull default ./code   # branch -> directory
//! cargo run -p overseer --example code_sync -- push default ./code   # directory -> branch
//! ```
//!
//! `pull` writes every module of the branch and deletes `.js`/`.wasm` files the branch does
//! not have; other files are kept. `push` replaces every module of the branch with the
//! directory's modules. `$activeWorld` / `$activeSim` name the active branches.
//!
//! Environment: `SCREEPS_URL` (default: the official server), `SCREEPS_TOKEN` (never printed),
//! or `SCREEPS_EMAIL` + `SCREEPS_PASSWORD` (password sign-in on private servers),
//! `SCREEPS_SERVER_PASSWORD` (password-protected private servers) and `SCREEPS_SHARD`
//! (default: the first shard the server lists).
use std::error::Error;

use overseer::code::ModuleKind;
use overseer::prelude::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [command, branch, dir] = args.as_slice() else {
        return Err("usage: code_sync (pull|push) <branch> <dir>".into());
    };
    let url = match env("SCREEPS_URL") {
        Some(url) => ServerUrl::parse(&url)?,
        None => ServerUrl::official(),
    };
    let auth = match (
        env("SCREEPS_TOKEN"),
        env("SCREEPS_EMAIL"),
        env("SCREEPS_PASSWORD"),
    ) {
        (Some(token), _, _) => Auth::Token(Token::new(token)),
        (None, Some(email), Some(password)) => Auth::Credentials { email, password },
        _ => return Err("set SCREEPS_TOKEN, or SCREEPS_EMAIL and SCREEPS_PASSWORD".into()),
    };
    let overseer = Overseer::connect(ServerProfile {
        url,
        auth,
        server_password: env("SCREEPS_SERVER_PASSWORD"),
        default_shard: env("SCREEPS_SHARD"),
        server_kind: None,
        plugins: None,
    })
    .await?;

    let code = overseer.code();
    let modules = match command.as_str() {
        "pull" => code.pull_dir(branch, dir).await?,
        "push" => code.push_dir(branch, dir).await?,
        _ => return Err(format!("unknown command `{command}`: use pull or push").into()),
    };
    let wasm = modules
        .iter()
        .filter(|(_, kind)| *kind == ModuleKind::Wasm)
        .count();
    println!(
        "{command}ed {} modules ({} JavaScript, {wasm} WebAssembly): {branch} {} {dir}",
        modules.len(),
        modules.len() - wasm,
        if command == "pull" { "->" } else { "<-" },
    );
    overseer.shutdown().await;
    Ok(())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
