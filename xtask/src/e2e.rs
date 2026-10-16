//! `cargo xtask e2e`: the live feature tests and the UI end-to-end tests, against a disposable
//! `openscreeps/server` world in Docker (target `openscreeps`, the default) or, opt-in and read-only,
//! against screeps.com (`--target official`). Neither runs under `cargo test`: the live tests
//! are `#[ignore]`d there and Playwright runs only from here.
//!
//! 1. Prepares what the selected suites need before any world exists: the server image
//!    (`openscreeps`; see [`world`]) or the API token (`official`; see [`official`]); the web
//!    app (`trunk build --release` of `app/` into `target/e2e/dist/`, its own bundle: `app/dist/`
//!    is the development build `trunk serve` rewrites on every change; skipped with
//!    `--no-build`), the `overseer-proxy` binary (copied to `target/e2e/`) and the live test
//!    targets; installs the Playwright project (`npm ci` in `e2e/` when needed,
//!    `npx playwright install chromium`).
//! 2. `openscreeps`: for each suite, creates a fresh world in a new Docker volume
//!    (`init --username e2e`), starts it on a free loopback port with fast ticks and waits until
//!    `GET /api/version` answers. Players and their bots slow a world down as they add up, so
//!    the suites do not share one.
//! 3. Runs the live Rust tests (`cargo test --workspace --test live -- --ignored`; on
//!    `official` only their read-only subset, the `read_only::` modules), then Playwright
//!    (`npx playwright test`, extra arguments after `--` are passed on), each with the target's
//!    environment (see [`common_environment`], [`world_environment`] and
//!    [`official_environment`]). Each Playwright project first loads the server's world once
//!    into the app's cache and saves it (`target/e2e/cache-<target>-<project>.json`); its tests
//!    start from a copy (see `e2e/tests/cache.setup.ts`). On `official`, Playwright runs the
//!    `proxy` project only, starts that setup from the previous official run's cache, leaves
//!    out the `@openscreeps` tests and fails any test that sends a request that may change the
//!    account (see `e2e/playwright.config.ts` and `e2e/tests/fixtures`); the summary counts the
//!    requests the proxy forwarded to screeps.com per endpoint.
//! 4. Always removes the world's container and volume: on success, on failure, on panic and on
//!    Ctrl-C. The servers' output is kept in `target/e2e/server-live.log` and
//!    `target/e2e/server-ui.log`, the test proxy's in `target/e2e/proxy.log`
//!    (`proxy-official.log` on `official`).
//!
//! `--rust-only` and `--ui-only` select one suite. `--screens[=<set>]` captures the reference
//! screenshots (the `@screens` tests, `proxy` project, `openscreeps` only) into
//! `e2e/references/<set>/` (default set `current`). The exit code is non-zero when any step
//! fails; a summary of every step closes the run.
//!
//! `OVERSEER_E2E_SLOW=<factor>` (e.g. `4`) runs the UI tests the way the CI runner does on a
//! fast machine: software WebGL (`SwiftShader`), pages whose CPU runs `<factor>` times slower
//! and whose responses arrive `<factor>` × 125 ms later, and two workers (see
//! `e2e/playwright.config.ts`).

mod official;
mod world;

use std::ffi::OsString;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Error;

/// What the tests run against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// A disposable `openscreeps/server` world in Docker.
    OpenScreeps,
    /// screeps.com, read-only.
    Official,
}

impl Target {
    /// The value of `OVERSEER_E2E_TARGET`.
    const fn name(self) -> &'static str {
        match self {
            Self::OpenScreeps => "openscreeps",
            Self::Official => "official",
        }
    }
}

/// What to run.
#[derive(Debug)]
struct Options {
    target: Target,
    rust: bool,
    ui: bool,
    build: bool,
    /// The reference screenshot set to capture (`--screens`).
    screens: Option<String>,
    playwright: Vec<String>,
}

impl Options {
    fn parse(args: &[&str]) -> Result<Self, Error> {
        let mut options = Self {
            target: Target::OpenScreeps,
            rust: true,
            ui: true,
            build: true,
            screens: None,
            playwright: Vec::new(),
        };
        let mut only = None;
        let mut args = args.iter();
        while let Some(&arg) = args.next() {
            match arg {
                "--rust-only" | "--ui-only" if only.is_some() => {
                    return Err("--rust-only and --ui-only exclude each other".into());
                }
                "--rust-only" | "--ui-only" => only = Some(arg),
                "--no-build" => options.build = false,
                "--screens" => options.screens = Some("current".to_owned()),
                "--target" => {
                    let value = args.next().ok_or("--target needs a value")?;
                    options.target = Self::target(value)?;
                }
                "--" => options.playwright = args.by_ref().map(|&arg| arg.to_owned()).collect(),
                _ => {
                    if let Some(value) = arg.strip_prefix("--target=") {
                        options.target = Self::target(value)?;
                    } else if let Some(set) = arg.strip_prefix("--screens=") {
                        if set.is_empty() || set.contains(['/', '\\', '.']) {
                            return Err(format!("`{set}` is no screenshot set name").into());
                        }
                        options.screens = Some(set.to_owned());
                    } else {
                        return Err(format!("unknown e2e option `{arg}`").into());
                    }
                }
            }
        }
        match only {
            Some("--rust-only") => options.ui = false,
            Some(_) => options.rust = false,
            None => {}
        }
        if options.screens.is_some() {
            if options.target != Target::OpenScreeps || !options.ui {
                return Err("--screens runs UI tests on the openscreeps target only".into());
            }
            options.rust = false;
            let mut playwright = vec!["--grep", "@screens", "--project", "proxy"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            playwright.append(&mut options.playwright);
            options.playwright = playwright;
        }
        Ok(options)
    }

    fn target(value: &str) -> Result<Target, Error> {
        match value {
            "openscreeps" => Ok(Target::OpenScreeps),
            "official" => Ok(Target::Official),
            _ => Err(format!("unknown target `{value}` (openscreeps, official)").into()),
        }
    }
}

/// The result of one step, for the summary.
enum Outcome {
    Passed,
    Failed(String),
    Skipped(&'static str),
}

/// The steps run so far, and notes for the summary.
#[derive(Default)]
struct Summary {
    steps: Vec<(String, Outcome, Duration)>,
    notes: Vec<String>,
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

    /// Prints every step, the notes and the result: failed when a step failed or `aborted` (an
    /// error ended the run outside the steps, for example before the first one).
    fn print(&self, aborted: bool) {
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
        for note in &self.notes {
            println!("  note     {note}");
        }
        println!(
            "  result: {}",
            if aborted || self.failed() {
                "FAILED"
            } else {
                "passed"
            }
        );
    }
}

/// Runs `cargo xtask e2e [--target openscreeps|official] [--rust-only | --ui-only]
/// [--screens[=<set>]] [--no-build] [-- <playwright args>]`.
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
    summary.print(result.is_err());
    result?;
    if summary.failed() {
        return Err("end-to-end tests failed".into());
    }
    Ok(())
}

/// Runs [`world::teardown`] when dropped, also while a panic unwinds.
struct TeardownOnDrop;

impl Drop for TeardownOnDrop {
    fn drop(&mut self) {
        world::teardown();
    }
}

/// Where the suites find what was prepared.
struct Prepared {
    root: PathBuf,
    e2e: PathBuf,
    logs: PathBuf,
    dist: PathBuf,
    proxy: PathBuf,
}

/// Every step; a failed preparation step skips the rest.
fn suites(root: &Path, options: &Options, summary: &mut Summary) -> Result<(), Error> {
    let target = target_dir(root);
    let logs = target.join("e2e");
    let prepared = Prepared {
        root: root.to_owned(),
        e2e: root.join("e2e"),
        dist: logs.join("dist"),
        proxy: logs.join(format!("overseer-proxy{}", std::env::consts::EXE_SUFFIX)),
        logs,
    };
    // Before anything is built: a missing token fails at once.
    let token = match options.target {
        Target::Official => Some(official::token()?),
        Target::OpenScreeps => None,
    };
    let mut image = None;
    let mut ready = match options.target {
        Target::OpenScreeps => summary.step("server image (docker build)", || {
            image = Some(world::image(root)?);
            Ok(())
        }),
        Target::Official => true,
    };
    if options.ui {
        if options.build {
            ready =
                ready && summary.step("web app (trunk build)", || build_app(root, &prepared.dist));
        } else if prepared.dist.join("index.html").is_file() {
            summary.skip("web app (trunk build)", "--no-build");
        } else {
            return Err(
                format!("{} has no build; drop --no-build", prepared.dist.display()).into(),
            );
        }
        ready = ready
            && summary.step("overseer-proxy binary", || {
                build_proxy(root, &target, &prepared.proxy)
            })
            && summary.step("Playwright dependencies", || {
                npm_ci_if_missing(&prepared.e2e)?;
                status(
                    Command::new(program("npx"))
                        .args(["playwright", "install", "chromium"])
                        .current_dir(&prepared.e2e),
                    "playwright install",
                )
            });
    }
    if options.rust {
        ready = ready
            && summary.step("live tests (build)", || {
                status(
                    cargo(root).args(["test", "--workspace", "--test", "live", "--no-run"]),
                    "cargo test --no-run",
                )
            });
    }
    if !ready {
        return Ok(());
    }
    match (image, token) {
        (Some(image), _) => openscreeps_suites(&prepared, options, &image, summary),
        (None, Some(token)) => official_suites(&prepared, options, &token, summary),
        (None, None) => return Err("no server image".into()),
    }
    Ok(())
}

/// The suites against fresh worlds of `image`.
fn openscreeps_suites(prepared: &Prepared, options: &Options, image: &str, summary: &mut Summary) {
    // Each suite gets a fresh world: players and their bots slow a world down as they add up.
    if options.rust {
        on_world(
            prepared,
            image,
            summary,
            "live Rust tests",
            "server-live.log",
            || {
                let mut test = cargo(&prepared.root);
                test.args(["test", "--workspace", "--test", "live", "--", "--ignored"]);
                test
            },
        );
    } else {
        summary.skip("live Rust tests", "--ui-only");
    }
    if options.ui {
        on_world(
            prepared,
            image,
            summary,
            "Playwright",
            "server-ui.log",
            || {
                let mut playwright = playwright(prepared, options);
                if let Some(set) = &options.screens {
                    playwright.env("OVERSEER_E2E_SCREENS", set);
                }
                playwright
            },
        );
    } else {
        summary.skip("Playwright", "--rust-only");
    }
}

/// Starts a world (its output in `<logs>/<log>`), runs the command `suite` builds with the
/// world's environment and removes the world.
fn on_world(
    prepared: &Prepared,
    image: &str,
    summary: &mut Summary,
    name: &str,
    log: &str,
    suite: impl FnOnce() -> Command,
) {
    let mut started = None;
    summary.step(&format!("{name}: disposable world"), || {
        started = Some(world::start(image, &prepared.logs, log)?);
        Ok(())
    });
    if let Some(started) = started {
        summary.step(name, || {
            let mut command = suite();
            common_environment(&mut command, prepared)?;
            world_environment(&mut command, &started);
            status(&mut command, name)
        });
    }
    world::teardown();
}

/// The read-only suites against screeps.com.
fn official_suites(prepared: &Prepared, options: &Options, token: &str, summary: &mut Summary) {
    if options.rust {
        summary.step("live Rust tests (read-only)", || {
            let mut test = cargo(&prepared.root);
            test.args(["test", "--workspace", "--test", "live", "--"])
                .args(["--ignored", "read_only::"]);
            common_environment(&mut test, prepared)?;
            official_environment(&mut test, token);
            status(&mut test, "cargo test")
        });
    } else {
        summary.skip("live Rust tests (read-only)", "--ui-only");
    }
    if options.ui {
        let log = prepared.logs.join("proxy-official.log");
        summary.step("Playwright (read-only)", || {
            let mut playwright = playwright(prepared, options);
            common_environment(&mut playwright, prepared)?;
            official_environment(&mut playwright, token);
            status(&mut playwright, "Playwright")
        });
        match official::forwarded(&log) {
            Ok(requests) => summary.notes.push(format!(
                "Playwright via the proxy to screeps.com: {requests}"
            )),
            Err(error) => summary.notes.push(error.to_string()),
        }
    } else {
        summary.skip("Playwright (read-only)", "--rust-only");
    }
}

/// `npx playwright test` with the extra arguments.
fn playwright(prepared: &Prepared, options: &Options) -> Command {
    let mut playwright = Command::new(program("npx"));
    playwright
        .args(["playwright", "test"])
        .args(&options.playwright)
        .current_dir(&prepared.e2e);
    playwright
}

/// Sets what both targets' tests read: `OVERSEER_E2E_DIST` (the built web app),
/// `OVERSEER_E2E_PROXY` (the `overseer-proxy` binary) and two free ports for the Playwright
/// web servers (`OVERSEER_E2E_DIRECT_PORT`, `OVERSEER_E2E_PROXY_PORT`).
fn common_environment(command: &mut Command, prepared: &Prepared) -> Result<(), Error> {
    let direct = TcpListener::bind("127.0.0.1:0")?;
    let proxied = TcpListener::bind("127.0.0.1:0")?;
    command
        .env("OVERSEER_E2E_DIST", &prepared.dist)
        .env("OVERSEER_E2E_PROXY", &prepared.proxy)
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

/// Sets the world the tests run against: `OVERSEER_E2E_TARGET=openscreeps`,
/// `OVERSEER_LIVE_URL` (server base URL), `OVERSEER_LIVE_USER`, `OVERSEER_LIVE_PASSWORD` (a
/// player without a spawn) and `OVERSEER_LIVE_ADMIN_TOKEN` (the admin API).
fn world_environment(command: &mut Command, world: &world::World) {
    command
        .env("OVERSEER_E2E_TARGET", Target::OpenScreeps.name())
        .env("OVERSEER_LIVE_URL", &world.url)
        .env("OVERSEER_LIVE_USER", world::USER)
        .env("OVERSEER_LIVE_PASSWORD", &world.password)
        .env("OVERSEER_LIVE_ADMIN_TOKEN", &world.admin_token)
        .env_remove("OVERSEER_LIVE_TOKEN");
}

/// Sets screeps.com as the server: `OVERSEER_E2E_TARGET=official`, `OVERSEER_LIVE_URL` and
/// `OVERSEER_LIVE_TOKEN` (the API token); nothing of a disposable world.
fn official_environment(command: &mut Command, token: &str) {
    command
        .env("OVERSEER_E2E_TARGET", Target::Official.name())
        .env("OVERSEER_LIVE_URL", official::URL)
        .env("OVERSEER_LIVE_TOKEN", token)
        .env_remove("OVERSEER_LIVE_USER")
        .env_remove("OVERSEER_LIVE_PASSWORD")
        .env_remove("OVERSEER_LIVE_ADMIN_TOKEN");
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

/// Set by Ctrl-C (and SIGTERM on Unix): the run ends without a summary.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Ctrl-C (and SIGTERM on Unix) tears down, then exits with status 130.
fn install_interrupt_handler() -> Result<(), Error> {
    ctrlc::set_handler(|| {
        INTERRUPTED.store(true, Ordering::SeqCst);
        eprintln!("\ninterrupted: removing the world");
        world::teardown();
        std::process::exit(130);
    })?;
    Ok(())
}

/// A value unique enough for a container name and a throwaway password.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        let default = Options::parse(&[]).unwrap();
        assert_eq!(default.target, Target::OpenScreeps);
        assert!(default.rust && default.ui && default.build);

        let official =
            Options::parse(&["--target", "official", "--ui-only", "--", "--grep", "x"]).unwrap();
        assert_eq!(official.target, Target::Official);
        assert!(!official.rust && official.ui);
        assert_eq!(official.playwright, ["--grep", "x"]);
        assert_eq!(
            Options::parse(&["--target=official"]).unwrap().target,
            Target::Official
        );

        let screens = Options::parse(&["--screens=before", "--", "--headed"]).unwrap();
        assert_eq!(screens.screens.as_deref(), Some("before"));
        assert!(!screens.rust && screens.ui);
        assert_eq!(
            screens.playwright,
            ["--grep", "@screens", "--project", "proxy", "--headed"]
        );
        assert_eq!(
            Options::parse(&["--screens"]).unwrap().screens.as_deref(),
            Some("current")
        );

        assert!(Options::parse(&["--rust-only", "--ui-only"]).is_err());
        assert!(Options::parse(&["--target", "mars"]).is_err());
        assert!(Options::parse(&["--target"]).is_err());
        assert!(Options::parse(&["--screens", "--target", "official"]).is_err());
        assert!(Options::parse(&["--screens", "--rust-only"]).is_err());
        assert!(Options::parse(&["--screens=../x"]).is_err());
        assert!(Options::parse(&["--node"]).is_err());
    }
}
