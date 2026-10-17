//! The `community` target: a disposable world of the standard community private server, in
//! Docker.
//!
//! The server is the stock `screeps` 4.3.0 (`@screeps/backend` 3.3.0) run by screeps-launcher,
//! with the two mods nearly every community server runs: screepsmod-auth 2.10.1 (password
//! sign-in and registration) and screepsmod-admin-utils 1.37.1 (server settings: fast ticks and
//! `shardName: shard0`, so `/api/version` lists `shards: ["shard0"]` as community servers do).
//! Everything is pinned in `e2e/community/`: the launcher image by digest in `Dockerfile`, the
//! server, the mods and their troublesome dependencies in `config.yml`. Overseer treats every
//! server but screeps.com as single-world (shardless socket channels); this world proves that
//! against a server that names a shard. It has no admin API of `openscreeps/server`: tests that
//! set the world up (portals, a stronghold) use its CLI instead (see below). Like a stock
//! server, it runs no room history mod, so `/room-history` answers HTTP 500 `not implemented`.
//!
//! The image (`overseer-e2e-community:<hash>`) has the packages and the initial world installed
//! at build time. It is built from `e2e/community/` and cached by a hash of `Dockerfile` and
//! `config.yml`, so editing either rebuilds it.
//!
//! The launcher refuses to start the server without a Steam Web API key (`STEAM_KEY`): it
//! exits at once. With one, the backend logs
//! `STEAM_KEY environment variable found, disabling native authentication`, listens, and checks
//! the key with Steam's Web API in the background, retrying for as long as that fails. The
//! harness passes the fixed placeholder [`STEAM_KEY`], which is no key: Steam answers HTTP 403
//! to every check, which breaks only sign-in with a Steam ticket (not under test). The game
//! server listens, the engine runs, and password registration and sign-in (screepsmod-auth)
//! work, so the target needs no secret.
//!
//! Each world is a container of its own (`overseer-e2e-community-<nonce>`, labelled
//! `overseer-e2e`) holding the world itself, published on a free loopback port. The server's
//! CLI (`POST /cli` with a JavaScript command, answered with the result as text; the community
//! counterpart of the admin API of `openscreeps/server`) listens on all of the container's
//! interfaces and is published on another loopback port for the tests. Once the server
//! answers, the harness registers its player over HTTP. [`teardown`] saves the launcher's output
//! to the log, the server processes' logs (`backend.log`, `main.log`, …) to a directory named
//! like it, and removes the container.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use super::world::{self, LABEL, docker, docker_output};
use super::{nonce, status};
use crate::Error;

/// The harness's own player (`OVERSEER_LIVE_USER`).
pub const USER: &str = "e2e";
/// The value of `STEAM_KEY` in the container: a placeholder, not a Steam Web API key (see the
/// module docs).
const STEAM_KEY: &str = "overseer-e2e-placeholder";
/// How long the server may take to answer once started: the launcher checks its packages with
/// yarn, then starts the storage, the engine's processes and the backend: a few seconds on a
/// fast machine, with room to spare for a slow one.
const STARTUP: Duration = Duration::from_secs(300);
/// Where the server's processes write their logs in the container.
const PROCESS_LOGS: &str = "/server/logs";

/// The running world and what the tests need to reach it.
pub struct World {
    /// Base URL with a trailing slash (`http://127.0.0.1:<port>/`).
    pub url: String,
    /// Password of [`USER`].
    pub password: String,
    /// The server's CLI endpoint (`http://127.0.0.1:<port>/cli`).
    pub cli: String,
}

/// The server image: built from `e2e/community/` unless cached (see the module docs). Returns
/// its tag.
pub fn image(root: &Path) -> Result<String, Error> {
    world::docker_available()?;
    let dir = root.join("e2e").join("community");
    let mut hash = FNV_OFFSET;
    for file in ["Dockerfile", "config.yml"] {
        let path = dir.join(file);
        let content =
            fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        hash = fnv1a(fnv1a(fnv1a(hash, file.as_bytes()), &[0]), &content);
    }
    let tag = format!("overseer-e2e-community:{hash:016x}");
    if docker_output(&["image", "inspect", "--format", "{{.Id}}", &tag]).is_ok() {
        println!("    cached image {tag}");
        return Ok(tag);
    }
    status(
        docker().args(["build", "--tag", &tag]).arg(&dir),
        "docker build",
    )?;
    Ok(tag)
}

/// The FNV-1a offset basis (64 bits).
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// FNV-1a (64 bits) of `bytes`, continuing from `hash`: a content hash that, unlike the
/// standard library's hashers, stays the same across Rust releases.
fn fnv1a(hash: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(hash, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What must be undone however the run ends; taken by the first [`teardown`].
struct Cleanup {
    /// The server's container.
    container: String,
    /// Where the launcher's output goes.
    log: PathBuf,
    /// Where the server processes' logs go.
    process_logs: PathBuf,
}

static CLEANUP: Mutex<Option<Cleanup>> = Mutex::new(None);

/// Saves the server's output and removes its container, once. Holds the lock throughout, so a
/// concurrent call (the interrupt handler) returns only after the cleanup is complete.
pub fn teardown() {
    let mut guard = CLEANUP.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(cleanup) = guard.take() else {
        return;
    };
    world::save_logs(&cleanup.container, &cleanup.log);
    // `docker cp` copies into an existing directory as a subdirectory: start from none. A
    // container that never started has no logs, which is no error.
    let _ = fs::remove_dir_all(&cleanup.process_logs);
    let _ = docker()
        .args(["cp", &format!("{}:{PROCESS_LOGS}", cleanup.container)])
        .arg(&cleanup.process_logs)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // `--volumes`: the launcher image declares `/screeps` a volume, so every container gets
    // an anonymous one. A container that never started is no error.
    let removed = docker()
        .args(["rm", "--force", "--volumes", &cleanup.container])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if removed.is_err() {
        eprintln!("warning: cannot remove the container {}", cleanup.container);
    }
}

/// Starts a world of `image` on a free loopback port (its output saved to
/// `logs/<log>` at [`teardown`], the server processes' logs to `logs/<log without .log>/`),
/// waits until it answers and registers [`USER`].
pub fn start(image: &str, logs: &Path, log: &str) -> Result<World, Error> {
    let name = format!("{LABEL}-community-{}", nonce());
    fs::create_dir_all(logs)?;
    let process_logs = logs.join(log.strip_suffix(".log").unwrap_or(log));
    *CLEANUP.lock().unwrap_or_else(PoisonError::into_inner) = Some(Cleanup {
        container: name.clone(),
        log: logs.join(log),
        process_logs: process_logs.clone(),
    });
    status(
        docker()
            .args(["run", "--detach", "--name", &name, "--label", LABEL])
            .args(["--publish", "127.0.0.1::21025"])
            .args(["--publish", "127.0.0.1::21026"])
            .args(["--env", &format!("STEAM_KEY={STEAM_KEY}")])
            .arg(image)
            .stdout(Stdio::null()),
        "docker run",
    )?;
    let address = world::published_address(&name)?;
    world::wait_ready(&name, &address, STARTUP).map_err(|error| {
        format!(
            "{error}; the server's logs will be in {}",
            process_logs.display()
        )
    })?;
    let password = format!("e2e-{}", nonce());
    register(&address, &password)?;
    let url = format!("http://{address}/");
    let cli = format!("http://{}/cli", world::published_port(&name, "21026/tcp")?);
    println!("    world at {url} (container {name})");
    Ok(World { url, password, cli })
}

/// Registers [`USER`] (email `e2e@e2e.invalid`) with `password` through screepsmod-auth's
/// `POST /api/register/submit`.
fn register(address: &str, password: &str) -> Result<(), Error> {
    let body = serde_json::json!({
        "username": USER,
        "email": format!("{USER}@e2e.invalid"),
        "password": password,
    })
    .to_string();
    let mut stream = TcpStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    write!(
        stream,
        "POST /api/register/submit HTTP/1.1\r\nHost: {address}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    let response = String::from_utf8_lossy(&response);
    let (head, reply) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let ok = head.split_whitespace().nth(1) == Some("200")
        && serde_json::from_str::<serde_json::Value>(reply.trim())
            .is_ok_and(|reply| reply["ok"] == 1);
    if !ok {
        let status = head.lines().next().unwrap_or_default();
        return Err(format!("registering {USER} failed: {status} {}", reply.trim()).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash() {
        // The published FNV-1a test vectors.
        assert_eq!(fnv1a(FNV_OFFSET, b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(FNV_OFFSET, b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(FNV_OFFSET, b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(
            fnv1a(fnv1a(FNV_OFFSET, b"foo"), b"bar"),
            fnv1a(FNV_OFFSET, b"foobar")
        );
    }
}
