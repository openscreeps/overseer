//! `cargo xtask e2e`: the live feature tests and the UI end-to-end tests, against a disposable
//! `openscreeps/server` world in Docker (target `openscreeps`, the default), a disposable world
//! of the standard community private server in Docker (`--target community`) or, opt-in and
//! read-only, against screeps.com (`--target official`). Neither runs under `cargo test`: the
//! live tests are `#[ignore]`d there and Playwright runs only from here.
//!
//! 1. Prepares what the selected suites need before any world exists: the server image
//!    (`openscreeps`, see [`world`]; `community`, see [`community`]) or the API token
//!    (`official`; see [`official`]); the web app (`trunk build --release` of `app/` into
//!    `dist/` of the run directory, its own bundle: `app/dist/` is the development build
//!    `trunk serve` rewrites on every change; skipped with `--no-build`), the `overseer-proxy`
//!    binary (copied to the run directory) and the live test targets; installs the Playwright
//!    project (`npm ci` in `e2e/` when needed, `npx playwright install chromium`).
//! 2. `openscreeps` and `community`: for each suite, starts a fresh world on a free loopback
//!    port with fast ticks and waits until `GET /api/version` answers. An `openscreeps` world is
//!    created in a new Docker volume (`init --username e2e`); a `community` world lives in its
//!    container, and the harness registers its player `e2e` over HTTP once the server answers.
//!    Players and their bots slow a world down as they add up, so the suites do not share one.
//! 3. Runs the live Rust tests (`cargo test --workspace --no-fail-fast --test live --
//!    --ignored`, so that every crate's results show; on `official` only their read-only
//!    subset, the `read_only::` modules),
//!    then Playwright (`npx playwright test`, extra arguments after `--` are passed on), each
//!    with the target's environment (see [`common_environment`], [`world_environment`],
//!    [`community_environment`] and [`official_environment`]). Each Playwright project first
//!    loads the server's world once into the app's cache and saves it
//!    (`cache-<project>.json` in the run directory); its tests start from a copy (see
//!    `e2e/tests/cache.setup.ts`). On `community`, Playwright runs the `proxy` project only
//!    (community servers send no CORS headers). On `official`, Playwright runs the
//!    `proxy` project only, starts that setup from the previous official run's cache, leaves out
//!    the `@openscreeps` tests (which need a disposable world, so also the `@admin` ones) and
//!    fails any test that sends a request that may change the account (see
//!    `e2e/playwright.config.ts` and `e2e/tests/fixtures`); the summary counts the requests the
//!    proxy forwarded to screeps.com per endpoint.
//! 4. Always removes the world's container (and volume): on success, on failure, on panic and
//!    on Ctrl-C. The servers' output goes to `server-live.log` and `server-ui.log` in the run
//!    directory (on `community` also the server processes' logs, in the directories of the
//!    same names), the test proxy's to `proxy.log`.
//!
//! `--rust-only` and `--ui-only` select one suite. `--screens[=<set>]` captures the reference
//! screenshots (the `@screens` tests, `proxy` project, `openscreeps` only) into
//! `e2e/references/<set>/` (default set `current`). `--desktop` (Windows only, opt-in, not in
//! CI) runs the desktop suite instead: the Tauri shell's test build driven over its
//! `WebView2`'s remote debugging protocol (`e2e/desktop.config.ts`, specs in `e2e/desktop/`), on a
//! fresh world (`openscreeps` or `community`; server log `server-desktop.log`) or, with
//! `--target official`, read-only against screeps.com; see [`desktop`]. The exit code is
//! non-zero when any step fails; a summary of every step closes the run.
//!
//! # Run directory
//!
//! Each run writes its files to its own directory below `<target>/e2e/` ([`RunDir`]): the web
//! app, the proxy binary, the world caches, the results (`test-results/`) and report
//! (`playwright-report/`) of Playwright, and the logs. Runs at the same time share only the
//! build in Cargo's target directory. Their web app builds go one at a time ([`build_app`]),
//! and a build of the live tests waits while another run executes them (Windows locks a
//! running executable; see [`wait_for_lock`]).
//!
//! - `openscreeps` and `community`: a fresh directory for each run (`<target>-<nonce>`). A run
//!   that passes removes it. A run that fails keeps it and names it in the summary.
//! - `official` (`official/`) and `--desktop` (`desktop/`): one directory that stays. Each
//!   official run starts from the cache of the previous one, and all of them share the
//!   rate limits of one account. The desktop test app has one state for each machine. While
//!   one run of these kinds goes on, a second run of the same kind fails at once.
//!
//! On Windows the running `xtask.exe` is locked. Do not change `xtask/` while a run goes on:
//! the next `cargo xtask` cannot rebuild it.
//!
//! `OVERSEER_E2E_SLOW=<factor>` (e.g. `4`) runs the UI tests the way the CI runner does on a
//! fast machine: software WebGL (`SwiftShader`), pages whose CPU runs `<factor>` times slower
//! and whose responses arrive `<factor>` × 125 ms later, and two workers (see
//! `e2e/playwright.config.ts`).

mod community;
mod desktop;
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
    /// A disposable world of the community private server in Docker.
    Community,
}

impl Target {
    /// The value of `OVERSEER_E2E_TARGET`.
    const fn name(self) -> &'static str {
        match self {
            Self::OpenScreeps => "openscreeps",
            Self::Official => "official",
            Self::Community => "community",
        }
    }
}

/// What to run.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one field per command-line switch, checked against each other in `parse`"
)]
struct Options {
    target: Target,
    rust: bool,
    ui: bool,
    build: bool,
    /// The reference screenshot set to capture (`--screens`).
    screens: Option<String>,
    /// The desktop suite alone (`--desktop`).
    desktop: bool,
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
            desktop: false,
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
                "--desktop" => options.desktop = true,
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
        if options.desktop {
            if only.is_some() || options.screens.is_some() {
                return Err(
                    "--desktop runs the desktop suite alone: no --rust-only, --ui-only or --screens"
                        .into(),
                );
            }
            options.rust = false;
            options.ui = false;
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
            "community" => Ok(Target::Community),
            _ => Err(format!("unknown target `{value}` (openscreeps, official, community)").into()),
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

/// Runs `cargo xtask e2e [--target openscreeps|official|community] [--rust-only | --ui-only | --desktop]
/// [--screens[=<set>]] [--no-build] [-- <playwright args>]`.
pub fn run(root: &Path, args: &[&str]) -> Result<(), Error> {
    let options = Options::parse(args)?;
    let run = RunDir::open(&target_dir(root), &options)?;
    install_interrupt_handler()?;
    let mut summary = Summary::default();
    let guard = TeardownOnDrop;
    let result = suites(root, &options, &run.path, &mut summary);
    // The teardown saves the servers' output to the run directory.
    drop(guard);
    if INTERRUPTED.load(Ordering::SeqCst) {
        // The interrupt handler exits the process once the teardown is complete.
        loop {
            std::thread::park();
        }
    }
    run.close(result.is_err() || summary.failed(), &mut summary);
    summary.print(result.is_err());
    result?;
    if summary.failed() {
        return Err("end-to-end tests failed".into());
    }
    Ok(())
}

/// The directory of one run (see the module docs).
struct RunDir {
    path: PathBuf,
    /// Whether the directory stays after the run (`official`, `--desktop`).
    stays: bool,
    /// The lock of a directory that stays, held until the run ends.
    _lock: Option<fs::File>,
}

impl RunDir {
    /// Creates the run's directory below `<target>/e2e/`: a fresh one on a disposable world,
    /// else the one of the run's kind, locked. Fails when another run holds that lock.
    fn open(target: &Path, options: &Options) -> Result<Self, Error> {
        let e2e = target.join("e2e");
        let stays = match (options.desktop, options.target) {
            (true, _) => Some("desktop"),
            (false, Target::Official) => Some("official"),
            (false, Target::OpenScreeps | Target::Community) => None,
        };
        let Some(name) = stays else {
            let path = e2e.join(format!("{}-{}", options.target.name(), nonce()));
            fs::create_dir_all(&path)?;
            return Ok(Self {
                path,
                stays: false,
                _lock: None,
            });
        };
        let path = e2e.join(name);
        fs::create_dir_all(&path)?;
        let lock = fs::File::create(path.join(".lock"))?;
        match lock.try_lock() {
            Ok(()) => Ok(Self {
                path,
                stays: true,
                _lock: Some(lock),
            }),
            Err(fs::TryLockError::WouldBlock) => Err(format!(
                "another run uses {}: run them one after the other",
                path.display()
            )
            .into()),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    /// Removes a fresh directory after a run that passed (or wrote nothing). Names a directory
    /// that stays in the summary.
    fn close(self, failed: bool, summary: &mut Summary) {
        if !self.stays {
            let removed = if failed {
                // Only when empty: a run that failed before its first step.
                fs::remove_dir(&self.path)
            } else {
                fs::remove_dir_all(&self.path)
            };
            if removed.is_ok() {
                return;
            }
        }
        summary
            .notes
            .push(format!("results, traces and logs: {}", self.path.display()));
    }
}

/// Runs [`world::teardown`], [`community::teardown`] and [`desktop::teardown`] when dropped,
/// also while a panic unwinds.
struct TeardownOnDrop;

impl Drop for TeardownOnDrop {
    fn drop(&mut self) {
        world::teardown();
        community::teardown();
        desktop::teardown();
    }
}

/// Where the suites find what was prepared.
struct Prepared {
    root: PathBuf,
    e2e: PathBuf,
    /// The run directory ([`RunDir`]).
    run: PathBuf,
    dist: PathBuf,
    proxy: PathBuf,
    /// The desktop test app (`--desktop`).
    desktop: PathBuf,
}

/// Every step; a failed preparation step skips the rest.
fn suites(root: &Path, options: &Options, run: &Path, summary: &mut Summary) -> Result<(), Error> {
    let target = target_dir(root);
    let prepared = Prepared {
        root: root.to_owned(),
        e2e: root.join("e2e"),
        run: run.to_owned(),
        dist: run.join("dist"),
        proxy: run.join(format!("overseer-proxy{}", std::env::consts::EXE_SUFFIX)),
        desktop: run.join(desktop::EXECUTABLE),
    };
    if options.desktop {
        if !cfg!(windows) {
            return Err("the desktop suite (--desktop) runs on Windows only".into());
        }
        // Before anything is built: an overlay without an identifier of its own fails at once.
        desktop::identifier(root)?;
    }
    // Before anything is built: a missing token fails at once.
    let token = match options.target {
        Target::Official => Some(official::token()?),
        Target::OpenScreeps | Target::Community => None,
    };
    let mut image = None;
    let mut server = None;
    let mut ready = match options.target {
        Target::OpenScreeps => summary.step("server image (docker build)", || {
            let (tag, source) = world::image()?;
            image = Some(tag);
            server = Some(source);
            Ok(())
        }),
        Target::Community => summary.step("community server image (docker build)", || {
            image = Some(community::image(root)?);
            Ok(())
        }),
        Target::Official => true,
    };
    summary.notes.extend(server);
    if options.ui || options.desktop {
        if options.build {
            ready = ready
                && summary.step("web app (trunk build)", || {
                    build_app(root, &target, &prepared.dist)
                });
        } else if prepared.dist.join("index.html").is_file() {
            summary.skip("web app (trunk build)", "--no-build");
        } else {
            return Err(
                format!("{} has no build; drop --no-build", prepared.dist.display()).into(),
            );
        }
    }
    if options.ui {
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
    if options.desktop {
        if options.build {
            ready = ready
                && summary.step("desktop app (cargo build --release)", || {
                    desktop::build(root, &target, &prepared.dist, &prepared.desktop)
                });
        } else if prepared.desktop.is_file() {
            summary.skip("desktop app (cargo build --release)", "--no-build");
        } else {
            return Err(format!(
                "{} has no build; drop --no-build",
                prepared.desktop.display()
            )
            .into());
        }
        // Playwright attaches to the app's WebView2: no browser to install.
        ready = ready
            && summary.step("Playwright dependencies", || {
                npm_ci_if_missing(&prepared.e2e)
            });
    }
    if options.rust {
        ready = ready
            && summary.step("live tests (build)", || {
                let _held = wait_for_lock(&target, "live", Hold::Exclusive, "the live tests")?;
                status(
                    cargo(root).args(["test", "--workspace", "--test", "live", "--no-run"]),
                    "cargo test --no-run",
                )
            });
    }
    if !ready {
        return Ok(());
    }
    match (options.target, image.as_deref(), token.as_deref()) {
        (Target::OpenScreeps, Some(image), _) => {
            world_suites(&prepared, options, Worlds::OpenScreeps { image }, summary);
        }
        (Target::Community, Some(image), _) => {
            world_suites(&prepared, options, Worlds::Community { image }, summary);
        }
        (Target::Official, _, Some(token)) => official_suites(&prepared, options, token, summary),
        _ => return Err("nothing was prepared for the target".into()),
    }
    Ok(())
}

/// The disposable worlds of a target.
#[derive(Clone, Copy)]
enum Worlds<'a> {
    /// `openscreeps/server` worlds of `image` ([`world`]).
    OpenScreeps { image: &'a str },
    /// Community private server worlds of `image` ([`community`]).
    Community { image: &'a str },
}

/// A running world of [`Worlds`].
enum World {
    OpenScreeps(world::World),
    Community(community::World),
}

impl Worlds<'_> {
    /// Starts a fresh world, its output saved to `<logs>/<log>`.
    fn start(self, logs: &Path, log: &str) -> Result<World, Error> {
        Ok(match self {
            Self::OpenScreeps { image } => World::OpenScreeps(world::start(image, logs, log)?),
            Self::Community { image } => World::Community(community::start(image, logs, log)?),
        })
    }

    /// Removes the world [`Worlds::start`] started.
    fn teardown(self) {
        match self {
            Self::OpenScreeps { .. } => world::teardown(),
            Self::Community { .. } => community::teardown(),
        }
    }
}

impl World {
    /// Sets the world the tests run against.
    fn environment(&self, command: &mut Command) {
        match self {
            Self::OpenScreeps(world) => world_environment(command, world),
            Self::Community(world) => community_environment(command, world),
        }
    }
}

/// The suites against fresh worlds.
fn world_suites(prepared: &Prepared, options: &Options, worlds: Worlds<'_>, summary: &mut Summary) {
    if options.desktop {
        desktop_suite(prepared, summary, |summary| {
            on_world(
                prepared,
                worlds,
                summary,
                "Playwright desktop",
                "server-desktop.log",
                || playwright(prepared, options),
            );
        });
        return;
    }
    // Each suite gets a fresh world: players and their bots slow a world down as they add up.
    if options.rust {
        // Windows locks a running executable: a build of the live tests in another run waits
        // until these finish.
        let target = target_dir(&prepared.root);
        match wait_for_lock(&target, "live", Hold::Shared, "a build of the live tests") {
            Ok(_held) => on_world(
                prepared,
                worlds,
                summary,
                "live Rust tests",
                "server-live.log",
                || {
                    let mut test = cargo(&prepared.root);
                    test.args(["test", "--workspace", "--no-fail-fast", "--test", "live"]);
                    test.args(["--", "--ignored"]);
                    test
                },
            ),
            Err(error) => {
                summary.step("live Rust tests", move || Err(error));
            }
        }
    } else {
        summary.skip("live Rust tests", "--ui-only");
    }
    if options.ui {
        on_world(
            prepared,
            worlds,
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
    worlds: Worlds<'_>,
    summary: &mut Summary,
    name: &str,
    log: &str,
    suite: impl FnOnce() -> Command,
) {
    let mut started = None;
    summary.step(&format!("{name}: disposable world"), || {
        started = Some(worlds.start(&prepared.run, log)?);
        Ok(())
    });
    if let Some(started) = started {
        summary.step(name, || {
            let mut command = suite();
            common_environment(&mut command, prepared)?;
            started.environment(&mut command);
            status(&mut command, name)
        });
    }
    worlds.teardown();
}

/// The read-only suites against screeps.com.
fn official_suites(prepared: &Prepared, options: &Options, token: &str, summary: &mut Summary) {
    if options.desktop {
        desktop_suite(prepared, summary, |summary| {
            summary.step("Playwright desktop (read-only)", || {
                let mut playwright = playwright(prepared, options);
                common_environment(&mut playwright, prepared)?;
                official_environment(&mut playwright, token);
                status(&mut playwright, "Playwright")
            });
        });
        return;
    }
    if options.rust {
        summary.step("live Rust tests (read-only)", || {
            let target = target_dir(&prepared.root);
            let _held = wait_for_lock(&target, "live", Hold::Shared, "a build of the live tests")?;
            let mut test = cargo(&prepared.root);
            test.args(["test", "--workspace", "--no-fail-fast", "--test", "live"])
                .args(["--", "--ignored", "read_only::"]);
            common_environment(&mut test, prepared)?;
            official_environment(&mut test, token);
            status(&mut test, "cargo test")
        });
    } else {
        summary.skip("live Rust tests (read-only)", "--ui-only");
    }
    if options.ui {
        let log = prepared.run.join("proxy.log");
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

/// The desktop suite around `tests`: removes what an earlier run left of the test app first,
/// then what the tests left, and checks that nothing is left (see [`desktop`]).
fn desktop_suite(prepared: &Prepared, summary: &mut Summary, tests: impl FnOnce(&mut Summary)) {
    let mut earlier = Vec::new();
    let clean = summary.step("desktop app state (before)", || {
        earlier = desktop::clean(&desktop::identifier(&prepared.root)?, &prepared.desktop)?;
        Ok(())
    });
    if !earlier.is_empty() {
        summary.notes.push(format!(
            "desktop: removed what an earlier run left: {}",
            earlier.join(", ")
        ));
    }
    if clean {
        tests(summary);
    }
    let mut left = Vec::new();
    if summary.step("desktop app state (after)", || {
        left = desktop::finish()?;
        Ok(())
    }) {
        summary.notes.push(if left.is_empty() {
            "desktop: the tests left no process, data directory or credential behind".to_owned()
        } else {
            format!("desktop: removed what the tests left: {}", left.join(", "))
        });
    }
}

/// `npx playwright test` with the extra arguments; the desktop suite's configuration and test
/// app (`OVERSEER_E2E_DESKTOP_APP`) with `--desktop`.
fn playwright(prepared: &Prepared, options: &Options) -> Command {
    let mut playwright = Command::new(program("npx"));
    playwright.args(["playwright", "test"]);
    if options.desktop {
        playwright
            .args(["--config", "desktop.config.ts"])
            .env("OVERSEER_E2E_DESKTOP_APP", &prepared.desktop);
    }
    playwright
        .args(&options.playwright)
        .current_dir(&prepared.e2e);
    playwright
}

/// Sets what both targets' tests read: `OVERSEER_E2E_OUTPUT` (the run directory: the built web
/// app, the `overseer-proxy` binary, the results) and two free ports for the Playwright web
/// servers (`OVERSEER_E2E_DIRECT_PORT`, `OVERSEER_E2E_PROXY_PORT`).
fn common_environment(command: &mut Command, prepared: &Prepared) -> Result<(), Error> {
    let direct = TcpListener::bind("127.0.0.1:0")?;
    let proxied = TcpListener::bind("127.0.0.1:0")?;
    command
        .env("OVERSEER_E2E_OUTPUT", &prepared.run)
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
        .env_remove("OVERSEER_LIVE_TOKEN")
        .env_remove("OVERSEER_LIVE_CLI_URL");
}

/// Sets the community world the tests run against: `OVERSEER_E2E_TARGET=community`,
/// `OVERSEER_LIVE_URL` (server base URL), `OVERSEER_LIVE_USER` and `OVERSEER_LIVE_PASSWORD` (a
/// registered player without a spawn) and `OVERSEER_LIVE_CLI_URL` (the server's CLI: `POST` a
/// JavaScript command, the result comes back as text); no admin API, no token.
fn community_environment(command: &mut Command, world: &community::World) {
    command
        .env("OVERSEER_E2E_TARGET", Target::Community.name())
        .env("OVERSEER_LIVE_URL", &world.url)
        .env("OVERSEER_LIVE_USER", community::USER)
        .env("OVERSEER_LIVE_PASSWORD", &world.password)
        .env("OVERSEER_LIVE_CLI_URL", &world.cli)
        .env_remove("OVERSEER_LIVE_ADMIN_TOKEN")
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
        .env_remove("OVERSEER_LIVE_ADMIN_TOKEN")
        .env_remove("OVERSEER_LIVE_CLI_URL");
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

/// How a run holds a lock of [`wait_for_lock`].
#[derive(Clone, Copy)]
enum Hold {
    /// With other runs that hold it shared.
    Shared,
    /// Alone.
    Exclusive,
}

/// Takes the lock `<target>/e2e/<name>.lock`, which runs at the same time share. Waits, and
/// says that it waits for `what` of another run, while another run holds it in a way that
/// excludes `hold`. The lock is released when the returned file is dropped.
fn wait_for_lock(target: &Path, name: &str, hold: Hold, what: &str) -> Result<fs::File, Error> {
    let file = fs::File::create(target.join("e2e").join(format!("{name}.lock")))?;
    let tried = match hold {
        Hold::Shared => file.try_lock_shared(),
        Hold::Exclusive => file.try_lock(),
    };
    match tried {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            println!("waiting for {what} of another run");
            match hold {
                Hold::Shared => file.lock_shared()?,
                Hold::Exclusive => file.lock()?,
            }
        }
        Err(fs::TryLockError::Error(error)) => return Err(error.into()),
    }
    Ok(file)
}

/// `trunk build --release` of `app/` into `dist`. Trunk rejects the `NO_COLOR=1` some shells
/// export, so it runs without it.
///
/// Trunk writes the output of `wasm-bindgen` below Cargo's target directory (`target`) before
/// it copies it to `dist`. Two builds at the same time overwrite each other's files there, so
/// one build goes at a time.
fn build_app(root: &Path, target: &Path, dist: &Path) -> Result<(), Error> {
    let _held = wait_for_lock(target, "trunk", Hold::Exclusive, "the web app build")?;
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

/// Builds the `overseer-proxy` binary and copies it to `copy` (in the run directory): the tests
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
        eprintln!("\ninterrupted: removing the world and the desktop test app's state");
        world::teardown();
        community::teardown();
        desktop::teardown();
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

        let community = Options::parse(&["--target", "community", "--rust-only"]).unwrap();
        assert_eq!(community.target, Target::Community);
        assert!(community.rust && !community.ui);
        assert_eq!(
            Options::parse(&["--target=community"]).unwrap().target,
            Target::Community
        );
        assert!(Options::parse(&["--screens", "--target", "community"]).is_err());
        assert_eq!(
            Options::parse(&["--desktop", "--target", "community"])
                .unwrap()
                .target,
            Target::Community
        );

        assert!(Options::parse(&["--rust-only", "--ui-only"]).is_err());
        assert!(Options::parse(&["--target", "mars"]).is_err());
        assert!(Options::parse(&["--target"]).is_err());
        assert!(Options::parse(&["--screens", "--target", "official"]).is_err());
        assert!(Options::parse(&["--screens", "--rust-only"]).is_err());
        assert!(Options::parse(&["--screens=../x"]).is_err());
        assert!(Options::parse(&["--node"]).is_err());

        let desktop =
            Options::parse(&["--desktop", "--target", "official", "--", "-g", "x"]).unwrap();
        assert!(desktop.desktop && !desktop.rust && !desktop.ui);
        assert_eq!(desktop.target, Target::Official);
        assert_eq!(desktop.playwright, ["-g", "x"]);
        assert!(!Options::parse(&[]).unwrap().desktop);
        assert!(Options::parse(&["--desktop", "--ui-only"]).is_err());
        assert!(Options::parse(&["--desktop", "--rust-only"]).is_err());
        assert!(Options::parse(&["--desktop", "--screens"]).is_err());
    }
}
