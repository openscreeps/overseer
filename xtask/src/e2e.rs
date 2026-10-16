//! `cargo xtask e2e`: the live feature tests and the UI end-to-end tests against a disposable
//! world of `openscreeps/server`.
//!
//! 1. Finds the server checkout (`OPENSCREEPS_SERVER_DIR`, else `../server`; `npm ci` when it
//!    has no `node_modules`).
//! 2. Builds what the selected suites need before any world exists: the web app
//!    (`trunk build --release` of `app/` into `target/e2e/dist/`, its own bundle: `app/dist/`
//!    is the development build `trunk serve` rewrites on every change; skipped with
//!    `--no-build`), the `overseer-proxy` binary (copied to `target/e2e/`) and the live test
//!    targets; installs the Playwright project (`npm ci` in `e2e/` when needed,
//!    `npx playwright install chromium`).
//! 3. For each suite, creates a fresh world in a temporary directory (`init --username e2e`),
//!    starts it on a free port with fast ticks and waits until `GET /api/version` answers.
//!    Players and their bots slow a world down as they add up, so the suites do not share one.
//! 4. Runs the live Rust tests (`cargo test --workspace --test live -- --ignored`), then
//!    Playwright (`npx playwright test`, extra arguments after `--` are passed on), each with
//!    its world's environment (see [`apply_environment`]).
//! 5. Always stops the server and deletes the temporary directory: on success, on failure, on
//!    panic and on Ctrl-C. The servers' output is kept in `target/e2e/server-live.log` and
//!    `target/e2e/server-ui.log`, the test proxy's in `target/e2e/proxy.log`.
//!
//! `--rust-only` and `--ui-only` select one suite. The exit code is non-zero when any step
//! fails; a summary of every step closes the run.
//!
//! `OVERSEER_E2E_SLOW=<factor>` (e.g. `4`) runs the UI tests the way the CI runner does on a
//! fast machine: software WebGL (`SwiftShader`), pages whose CPU runs `<factor>` times slower
//! and whose responses arrive `<factor>` × 125 ms later, and two workers (see
//! `e2e/playwright.config.ts`).

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Error;

/// The harness's own player (`OVERSEER_LIVE_USER`).
const USER: &str = "e2e";
/// Milliseconds per tick: fast enough that creeps visibly move within a test.
const TICK_MS: &str = "200";
/// Rooms per side of the world.
const WORLD_SIZE: &str = "11";
/// How long the server may take to start listening and to answer.
const STARTUP: Duration = Duration::from_secs(120);

/// What to run.
#[derive(Debug)]
struct Options {
    rust: bool,
    ui: bool,
    build: bool,
    playwright: Vec<String>,
}

impl Options {
    fn parse(args: &[&str]) -> Result<Self, Error> {
        let mut options = Self {
            rust: true,
            ui: true,
            build: true,
            playwright: Vec::new(),
        };
        let mut args = args.iter();
        while let Some(&arg) = args.next() {
            match arg {
                "--rust-only" => options.ui = false,
                "--ui-only" => options.rust = false,
                "--no-build" => options.build = false,
                "--" => options.playwright = args.by_ref().map(|&arg| arg.to_owned()).collect(),
                _ => return Err(format!("unknown e2e option `{arg}`").into()),
            }
        }
        if !options.rust && !options.ui {
            return Err("--rust-only and --ui-only exclude each other".into());
        }
        Ok(options)
    }
}

/// The result of one step, for the summary.
enum Outcome {
    Passed,
    Failed(String),
    Skipped(&'static str),
}

/// The steps run so far.
#[derive(Default)]
struct Summary {
    steps: Vec<(String, Outcome, Duration)>,
}

impl Summary {
    /// Runs `step` and records how it went; returns whether it passed.
    fn step(&mut self, name: &str, step: impl FnOnce() -> Result<(), Error>) -> bool {
        println!("==> {name}");
        let start = Instant::now();
        let outcome = match step() {
            Ok(()) => Outcome::Passed,
            Err(error) => Outcome::Failed(error.to_string()),
        };
        let passed = matches!(outcome, Outcome::Passed);
        self.steps.push((name.to_owned(), outcome, start.elapsed()));
        passed
    }

    fn skip(&mut self, name: &str, reason: &'static str) {
        self.steps
            .push((name.to_owned(), Outcome::Skipped(reason), Duration::ZERO));
    }

    fn failed(&self) -> bool {
        self.steps
            .iter()
            .any(|(_, outcome, _)| matches!(outcome, Outcome::Failed(_)))
    }

    fn print(&self) {
        println!("\ne2e summary");
        for (name, outcome, elapsed) in &self.steps {
            let seconds = elapsed.as_secs_f32();
            match outcome {
                Outcome::Passed => println!("  passed   {name} ({seconds:.1} s)"),
                Outcome::Failed(error) => {
                    println!("  FAILED   {name} ({seconds:.1} s): {error}");
                }
                Outcome::Skipped(reason) => println!("  skipped  {name} ({reason})"),
            }
        }
        println!(
            "  result: {}",
            if self.failed() { "FAILED" } else { "passed" }
        );
    }
}

/// Runs `cargo xtask e2e [--rust-only | --ui-only] [--no-build] [-- <playwright args>]`.
pub fn run(root: &Path, args: &[&str]) -> Result<(), Error> {
    let options = Options::parse(args)?;
    install_interrupt_handler()?;
    let mut summary = Summary::default();
    let guard = TeardownOnDrop;
    let result = suites(root, &options, &mut summary);
    drop(guard);
    if INTERRUPTED.load(Ordering::SeqCst) {
        // The interrupt handler exits the process once the teardown is complete.
        loop {
            std::thread::park();
        }
    }
    summary.print();
    result?;
    if summary.failed() {
        return Err("end-to-end tests failed".into());
    }
    Ok(())
}

/// Runs [`teardown`] when dropped, also while a panic unwinds.
struct TeardownOnDrop;

impl Drop for TeardownOnDrop {
    fn drop(&mut self) {
        teardown();
    }
}

/// Every step; a failed preparation step skips the rest.
fn suites(root: &Path, options: &Options, summary: &mut Summary) -> Result<(), Error> {
    let server = server_dir(root)?;
    let target = target_dir(root);
    let dist = target.join("e2e").join("dist");
    let e2e = root.join("e2e");
    let proxy = target
        .join("e2e")
        .join(format!("overseer-proxy{}", std::env::consts::EXE_SUFFIX));
    let mut prepared = summary.step("server dependencies", || npm_ci_if_missing(&server));
    if options.ui {
        if options.build {
            prepared = prepared && summary.step("web app (trunk build)", || build_app(root, &dist));
        } else if dist.join("index.html").is_file() {
            summary.skip("web app (trunk build)", "--no-build");
        } else {
            return Err(format!("{} has no build; drop --no-build", dist.display()).into());
        }
        prepared = prepared
            && summary.step("overseer-proxy binary", || {
                build_proxy(root, &target, &proxy)
            })
            && summary.step("Playwright dependencies", || {
                npm_ci_if_missing(&e2e)?;
                status(
                    Command::new(program("npx"))
                        .args(["playwright", "install", "chromium"])
                        .current_dir(&e2e),
                    "playwright install",
                )
            });
    }
    if options.rust {
        prepared = prepared
            && summary.step("live tests (build)", || {
                status(
                    cargo(root).args(["test", "--workspace", "--test", "live", "--no-run"]),
                    "cargo test --no-run",
                )
            });
    }
    if !prepared {
        return Ok(());
    }
    let context = Suite {
        server: &server,
        target: &target,
        dist: &dist,
        proxy: &proxy,
    };
    // Each suite gets a fresh world: players and their bots slow a world down as they add up.
    if options.rust {
        context.run(summary, "live Rust tests", "server-live.log", || {
            let mut test = cargo(root);
            test.args(["test", "--workspace", "--test", "live", "--", "--ignored"]);
            test
        });
    } else {
        summary.skip("live Rust tests", "--ui-only");
    }
    if options.ui {
        context.run(summary, "Playwright", "server-ui.log", || {
            let mut playwright = Command::new(program("npx"));
            playwright
                .args(["playwright", "test"])
                .args(&options.playwright)
                .current_dir(&e2e);
            playwright
        });
    } else {
        summary.skip("Playwright", "--rust-only");
    }
    Ok(())
}

/// What a test suite runs against.
struct Suite<'a> {
    server: &'a Path,
    target: &'a Path,
    dist: &'a Path,
    proxy: &'a Path,
}

impl Suite<'_> {
    /// Starts a world (its output in `target/e2e/<log>`), runs the command `suite` builds with
    /// the world's environment and stops the world.
    fn run(&self, summary: &mut Summary, name: &str, log: &str, suite: impl FnOnce() -> Command) {
        let mut world = None;
        summary.step(&format!("{name}: disposable world"), || {
            world = Some(start_world(self.server, &self.target.join("e2e"), log)?);
            Ok(())
        });
        if let Some(world) = world {
            summary.step(name, || {
                let mut command = suite();
                apply_environment(&mut command, &world, self.dist, self.proxy)?;
                status(&mut command, name)
            });
        }
        teardown();
    }
}

/// The server checkout: `OPENSCREEPS_SERVER_DIR`, else `../server` next to this repository.
fn server_dir(root: &Path) -> Result<PathBuf, Error> {
    let dir = std::env::var_os("OPENSCREEPS_SERVER_DIR")
        .map_or_else(|| root.join("..").join("server"), PathBuf::from);
    if !dir.join("package.json").is_file() {
        return Err(format!(
            "no openscreeps/server checkout at {} (set OPENSCREEPS_SERVER_DIR)",
            dir.display()
        )
        .into());
    }
    Ok(dir)
}

/// `npm ci` in `dir` unless it already has `node_modules`.
fn npm_ci_if_missing(dir: &Path) -> Result<(), Error> {
    if dir.join("node_modules").is_dir() {
        return Ok(());
    }
    status(
        Command::new(program("npm")).arg("ci").current_dir(dir),
        "npm ci",
    )
}

/// `trunk build --release` of `app/` into `dist`. Trunk rejects the `NO_COLOR=1` some shells
/// export, so it runs without it.
fn build_app(root: &Path, dist: &Path) -> Result<(), Error> {
    status(
        Command::new("trunk")
            .args(["build", "--release", "--dist"])
            // Trunk runs in `app/`: a relative `CARGO_TARGET_DIR` is the workspace's.
            .arg(std::path::absolute(dist)?)
            .env_remove("NO_COLOR")
            .current_dir(root.join("app")),
        "trunk build",
    )
}

/// Builds the `overseer-proxy` binary and copies it to `copy` (in `target/e2e/`): the tests
/// run the copy, so a running test proxy never blocks rebuilding the original (Windows locks
/// running executables).
fn build_proxy(root: &Path, target: &Path, copy: &Path) -> Result<(), Error> {
    status(
        cargo(root).args(["build", "--package", "overseer-proxy"]),
        "cargo build",
    )?;
    if let Some(dir) = copy.parent() {
        fs::create_dir_all(dir)?;
    }
    let built = target
        .join("debug")
        .join(format!("overseer-proxy{}", std::env::consts::EXE_SUFFIX));
    fs::copy(&built, copy).map_err(|error| format!("cannot copy {}: {error}", built.display()))?;
    Ok(())
}

/// The running world and what the tests need to reach it.
struct World {
    url: String,
    password: String,
    admin_token: String,
}

/// Sets the variables the live tests and the Playwright project read: `OVERSEER_LIVE_URL`
/// (server base URL), `OVERSEER_LIVE_USER`, `OVERSEER_LIVE_PASSWORD` (a player without a
/// spawn), `OVERSEER_LIVE_ADMIN_TOKEN` (the admin API), `OVERSEER_E2E_DIST` (the built web
/// app), `OVERSEER_E2E_PROXY` (the `overseer-proxy` binary) and two free ports for the
/// Playwright web servers (`OVERSEER_E2E_DIRECT_PORT`, `OVERSEER_E2E_PROXY_PORT`).
fn apply_environment(
    command: &mut Command,
    world: &World,
    dist: &Path,
    proxy: &Path,
) -> Result<(), Error> {
    let direct = TcpListener::bind("127.0.0.1:0")?;
    let proxied = TcpListener::bind("127.0.0.1:0")?;
    command
        .env("OVERSEER_LIVE_URL", &world.url)
        .env("OVERSEER_LIVE_USER", USER)
        .env("OVERSEER_LIVE_PASSWORD", &world.password)
        .env("OVERSEER_LIVE_ADMIN_TOKEN", &world.admin_token)
        .env("OVERSEER_E2E_DIST", dist)
        .env("OVERSEER_E2E_PROXY", proxy)
        .env(
            "OVERSEER_E2E_DIRECT_PORT",
            direct.local_addr()?.port().to_string(),
        )
        .env(
            "OVERSEER_E2E_PROXY_PORT",
            proxied.local_addr()?.port().to_string(),
        );
    Ok(())
}

/// What must be undone however the run ends; taken by the first [`teardown`].
struct Cleanup {
    server: Option<Child>,
    dir: PathBuf,
}

static CLEANUP: Mutex<Option<Cleanup>> = Mutex::new(None);

/// Set by Ctrl-C (and SIGTERM on Unix): the run ends without a summary.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Stops the server and deletes the world's directory, once. Holds the lock throughout, so a
/// concurrent call (the interrupt handler) returns only after the cleanup is complete.
fn teardown() {
    let mut guard = CLEANUP.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(mut cleanup) = guard.take() else {
        return;
    };
    if let Some(mut server) = cleanup.server.take() {
        // Ignored: the server may have exited already (Ctrl-C reaches it too).
        let _ = server.kill();
        let _ = server.wait();
    }
    // SQLite and the OS may hold the files a moment after the process ended.
    for _ in 0..20 {
        if fs::remove_dir_all(&cleanup.dir).is_ok() || !cleanup.dir.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    eprintln!("warning: could not delete {}", cleanup.dir.display());
}

/// Ctrl-C (and SIGTERM on Unix) tears down, then exits with status 130.
fn install_interrupt_handler() -> Result<(), Error> {
    ctrlc::set_handler(|| {
        INTERRUPTED.store(true, Ordering::SeqCst);
        eprintln!("\ninterrupted: stopping the server");
        teardown();
        std::process::exit(130);
    })?;
    Ok(())
}

/// Creates a world in a new temporary directory, starts it on a free port (its output in
/// `logs/<log>`) and waits until it answers.
fn start_world(server: &Path, logs: &Path, log: &str) -> Result<World, Error> {
    let nonce = nonce();
    let dir = std::env::temp_dir().join(format!("overseer-e2e-{nonce}"));
    fs::create_dir_all(&dir)?;
    *CLEANUP.lock().unwrap_or_else(PoisonError::into_inner) = Some(Cleanup {
        server: None,
        dir: dir.clone(),
    });
    let database = dir.join("world.sqlite");
    let password = format!("e2e-{nonce}");
    let mut init = node_cli(server);
    init.arg("init")
        .arg("--database")
        .arg(&database)
        .args([
            "--world-size",
            WORLD_SIZE,
            "--seed",
            "1",
            "--username",
            USER,
        ])
        .env("SCREEPS_INITIAL_PASSWORD", &password)
        .stdout(Stdio::null());
    status(&mut init, "server init")?;
    let admin_token = fs::read_to_string(dir.join("world.sqlite.admin-token"))?
        .trim()
        .to_owned();

    fs::create_dir_all(logs)?;
    let log_path = logs.join(log);
    let mut log = File::create(&log_path)?;
    let mut start = node_cli(server);
    start
        .arg("start")
        .arg("--database")
        .arg(&database)
        .args(["--port", "0", "--tick-duration", TICK_MS])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = start
        .spawn()
        .map_err(|error| format!("cannot start the server: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    if let Some(cleanup) = CLEANUP
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
    {
        cleanup.server = Some(child);
    } else {
        // Interrupted while starting: the teardown already ran.
        let _ = child.kill();
        let _ = child.wait();
        return Err("interrupted".into());
    }
    // Both streams go to the log; stdout is scanned for the listen line first.
    let (listening, url) = mpsc::channel();
    if let Some(stderr) = stderr {
        let mut log = log.try_clone()?;
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut BufReader::new(stderr), &mut log);
        });
    }
    if let Some(stdout) = stdout {
        std::thread::spawn(move || {
            let mut sent = false;
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = writeln!(log, "{line}");
                if !sent && let Some(url) = listen_url(&line) {
                    sent = listening.send(url).is_ok();
                }
            }
        });
    }
    let url = url.recv_timeout(STARTUP).map_err(|_| {
        format!(
            "the server did not start listening; see {}",
            log_path.display()
        )
    })?;
    wait_ready(&url)?;
    println!("    world at {url} ({})", dir.display());
    Ok(World {
        url,
        password,
        admin_token,
    })
}

/// The URL of `OpenScreeps server listening on http://127.0.0.1:PORT/`.
fn listen_url(line: &str) -> Option<String> {
    let (_, url) = line.split_once("listening on ")?;
    let url = url.trim();
    url.starts_with("http://").then(|| url.to_owned())
}

/// Polls `GET /api/version` until it answers 200.
fn wait_ready(url: &str) -> Result<(), Error> {
    let address = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    let deadline = Instant::now() + STARTUP;
    loop {
        if version_ok(&address) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("{url}api/version never answered 200").into());
        }
        std::thread::sleep(Duration::from_millis(200));
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

/// `node --import tsx src/cli.ts` in the server checkout: the server's `npm run cli` without
/// npm in between, so that stopping the process stops the server.
fn node_cli(server: &Path) -> Command {
    let mut command = Command::new("node");
    command
        .args(["--import", "tsx", "src/cli.ts"])
        .current_dir(server);
    command
}

/// A value unique enough for a directory name and a throwaway password.
fn nonce() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{:x}{:08x}",
        std::process::id(),
        now.as_secs(),
        now.subsec_nanos()
    )
}

/// `cargo` (the one running this task) in the workspace root.
fn cargo(root: &Path) -> Command {
    let mut command =
        Command::new(std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")));
    command.current_dir(root);
    command
}

/// A Node.js launcher by name: `npm.cmd` and `npx.cmd` on Windows.
fn program(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        name.to_owned()
    }
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
