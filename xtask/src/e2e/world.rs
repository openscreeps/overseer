//! The disposable `openscreeps/server` world of the `openscreeps` target, in Docker.
//!
//! The server image is built once per pinned commit and cached by tag
//! (`overseer-e2e-server:<commit>`): Docker builds it straight from
//! `https://github.com/openscreeps/server.git#<commit>`, so no checkout is needed. With
//! `OPENSCREEPS_SERVER_DIR`, the image is built from that checkout instead (tag
//! `overseer-e2e-server:local`, rebuilt every run; Docker's layer cache keeps that quick), to
//! test server changes before they are pinned.
//!
//! Each world lives in its own named volume and container (`overseer-e2e-<nonce>`, labelled
//! `overseer-e2e`), published on a free loopback port, and is removed with its volume by
//! [`teardown`] (its output saved to the log first).

use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::{nonce, status};
use crate::Error;

/// The harness's own player (`OVERSEER_LIVE_USER`).
pub const USER: &str = "e2e";
/// Milliseconds per tick: fast enough that creeps visibly move within a test.
const TICK_MS: &str = "200";
/// Rooms per side of the world.
const WORLD_SIZE: &str = "11";
/// How long the server may take to answer once started.
const STARTUP: Duration = Duration::from_secs(120);
/// The server's port in the container.
const PORT: &str = "21025/tcp";
/// Label of every container and volume the harness creates.
pub(super) const LABEL: &str = "overseer-e2e";
/// Where the server's image is built from when no checkout is given.
const REPOSITORY: &str = "https://github.com/openscreeps/server.git";

/// The running world and what the tests need to reach it.
pub struct World {
    /// Base URL with a trailing slash (`http://127.0.0.1:<port>/`).
    pub url: String,
    /// Password of [`USER`].
    pub password: String,
    /// Bearer token of the admin API.
    pub admin_token: String,
}

/// The server image: built from the commit in `e2e/server.lock` unless cached (or from
/// `OPENSCREEPS_SERVER_DIR`, see the module docs). Returns its tag.
pub fn image(root: &Path) -> Result<String, Error> {
    docker_available()?;
    if let Some(dir) = std::env::var_os("OPENSCREEPS_SERVER_DIR") {
        let tag = "overseer-e2e-server:local".to_owned();
        status(
            docker().args(["build", "--tag", &tag]).arg(dir),
            "docker build",
        )?;
        return Ok(tag);
    }
    let lock = root.join("e2e").join("server.lock");
    let commit = fs::read_to_string(&lock)
        .map_err(|error| format!("cannot read {}: {error}", lock.display()))?
        .trim()
        .to_owned();
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{} holds no commit SHA", lock.display()).into());
    }
    let tag = format!("overseer-e2e-server:{commit}");
    if docker_output(&["image", "inspect", "--format", "{{.Id}}", &tag]).is_ok() {
        println!("    cached image {tag}");
        return Ok(tag);
    }
    status(
        docker()
            .args(["build", "--tag", &tag])
            .arg(format!("{REPOSITORY}#{commit}")),
        "docker build",
    )?;
    Ok(tag)
}

/// Fails early, with a clear message, when Docker is not running.
pub(super) fn docker_available() -> Result<(), Error> {
    docker_output(&["version", "--format", "{{.Server.Version}}"])
        .map_err(|error| format!("Docker is not available ({error}); start Docker"))?;
    Ok(())
}

/// What must be undone however the run ends; taken by the first [`teardown`].
struct Cleanup {
    /// The server's container.
    container: String,
    /// The container that creates the world.
    init: String,
    /// The world's volume.
    volume: String,
    /// Where the server's output goes.
    log: PathBuf,
}

static CLEANUP: Mutex<Option<Cleanup>> = Mutex::new(None);

/// Saves the server's output, removes its containers and the world's volume, once. Holds the
/// lock throughout, so a concurrent call (the interrupt handler) returns only after the
/// cleanup is complete.
pub fn teardown() {
    let mut guard = CLEANUP.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(cleanup) = guard.take() else {
        return;
    };
    save_logs(&cleanup.container, &cleanup.log);
    // Containers that never started are no error.
    let removed = docker()
        .args([
            "rm",
            "--force",
            "--volumes",
            &cleanup.container,
            &cleanup.init,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if removed.is_err() {
        eprintln!(
            "warning: cannot remove the containers {}",
            cleanup.container
        );
    }
    let volume = docker()
        .args(["volume", "rm", "--force", &cleanup.volume])
        .stdout(Stdio::null())
        .status();
    if !volume.is_ok_and(|status| status.success()) {
        eprintln!(
            "warning: cannot remove the Docker volume {}",
            cleanup.volume
        );
    }
}

/// Creates a world in a new volume with `image`, starts it on a free loopback port (its output
/// saved to `logs/<log>` at [`teardown`]) and waits until it answers.
pub fn start(image: &str, logs: &Path, log: &str) -> Result<World, Error> {
    let name = format!("{LABEL}-{}", nonce());
    let init = format!("{name}-init");
    fs::create_dir_all(logs)?;
    *CLEANUP.lock().unwrap_or_else(PoisonError::into_inner) = Some(Cleanup {
        container: name.clone(),
        init: init.clone(),
        volume: name.clone(),
        log: logs.join(log),
    });
    let volume = format!("{name}:/data");
    docker_output(&["volume", "create", "--label", LABEL, &name])?;

    let password = format!("e2e-{}", nonce());
    status(
        docker()
            .args(["run", "--rm", "--name", &init, "--label", LABEL])
            .args(["--volume", &volume])
            // The value comes from this process's environment, not the command line.
            .args(["--env", "SCREEPS_INITIAL_PASSWORD"])
            .env("SCREEPS_INITIAL_PASSWORD", &password)
            .arg(image)
            .args(["init", "--world-size", WORLD_SIZE, "--seed", "1"])
            .args(["--username", USER])
            .stdout(Stdio::null()),
        "server init",
    )?;
    docker_output(&[
        "run",
        "--detach",
        "--name",
        &name,
        "--label",
        LABEL,
        "--volume",
        &volume,
        "--publish",
        "127.0.0.1::21025",
        image,
        "start",
        "--tick-duration",
        TICK_MS,
    ])?;
    let admin_token = docker_output(&["exec", &name, "cat", "/data/world.sqlite.admin-token"])?;
    let address = published_address(&name)?;
    wait_ready(&name, &address, STARTUP)?;
    let url = format!("http://{address}/");
    println!("    world at {url} (container {name})");
    Ok(World {
        url,
        password,
        admin_token,
    })
}

/// Writes the output of `container` to `log`; warns when that fails.
pub(super) fn save_logs(container: &str, log: &Path) {
    if let Ok(output) = docker().args(["logs", container]).output() {
        let written = fs::File::create(log).and_then(|mut log| {
            log.write_all(&output.stdout)?;
            log.write_all(&output.stderr)
        });
        if let Err(error) = written {
            eprintln!("warning: cannot write {}: {error}", log.display());
        }
    }
}

/// The loopback address (`127.0.0.1:<port>`) the server's port of `container` is published on.
pub(super) fn published_address(container: &str) -> Result<String, Error> {
    published_port(container, PORT)
}

/// The loopback address `container` publishes its `port` (such as `21026/tcp`) on.
pub(super) fn published_port(container: &str, port: &str) -> Result<String, Error> {
    // `127.0.0.1:49153` (one line per published address).
    let published = docker_output(&["port", container, port])?;
    let port = published
        .lines()
        .find_map(|line| line.trim().strip_prefix("127.0.0.1:"))
        .ok_or_else(|| format!("unexpected `docker port` output: {published}"))?;
    Ok(format!("127.0.0.1:{port}"))
}

/// Polls `GET /api/version` until it answers 200, for up to `startup`; fails early when the
/// container stopped.
pub(super) fn wait_ready(container: &str, address: &str, startup: Duration) -> Result<(), Error> {
    let deadline = Instant::now() + startup;
    let mut polls = 0_u32;
    loop {
        if version_ok(address) {
            return Ok(());
        }
        polls += 1;
        if polls.is_multiple_of(8)
            && docker_output(&["inspect", "--format", "{{.State.Running}}", container])? != "true"
        {
            return Err(format!("the server stopped; see `docker logs {container}`").into());
        }
        if Instant::now() > deadline {
            return Err(format!("http://{address}/api/version never answered 200").into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Whether `GET /api/version` on `address` answers HTTP 200.
fn version_ok(address: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect(address) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let request =
        format!("GET /api/version HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut status = [0; 12];
    stream.read_exact(&mut status).is_ok() && status.ends_with(b" 200")
}

/// The `docker` CLI.
pub(super) fn docker() -> Command {
    Command::new("docker")
}

/// Runs `docker <args>` and returns its trimmed standard output; its error output on failure.
pub(super) fn docker_output(args: &[&str]) -> Result<String, Error> {
    let output = docker()
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run docker: {error}"))?;
    if !output.status.success() {
        let verb = args.first().copied().unwrap_or_default();
        return Err(format!(
            "docker {verb} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
