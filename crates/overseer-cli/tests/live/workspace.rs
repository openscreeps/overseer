//! The directory of the CLI runs: the configuration, the build directory, and the selector's
//! answer. The test runs the built `overseer` binary in this directory.
//!
//! Each run gets the app identifier of the workspace (`OVERSEER_APP_IDENTIFIER`), so the CLI
//! never reads or changes the saved servers of the user.

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::support::{self, Account};

/// The profile that runs in the world and places the spawn.
pub const PRIMARY: &str = "primary";
/// The profile of the same server that only receives uploads.
pub const MIRROR: &str = "mirror";
/// The profile of the saved server.
pub const SAVED: &str = "saved";
/// The name of the saved server in the storage of the test.
pub const SAVED_SERVER: &str = "e2e";
/// The rooms around the start room that the candidates come from, in each direction.
pub const RADIUS: u32 = 3;
/// The variable that gives the password to the CLI. The configuration names it.
const PASSWORD_VAR: &str = "OVERSEER_E2E_CLI_PASSWORD";
/// The variable that selects the storage of the saved servers.
pub const IDENTIFIER_VAR: &str = "OVERSEER_APP_IDENTIFIER";
/// The longest run of the CLI. The selector alone may run 60 s.
const CLI_TIMEOUT: Duration = Duration::from_secs(180);
/// How often the test checks whether the CLI stopped.
const CLI_POLL: Duration = Duration::from_millis(50);

/// A new temporary directory for the configuration and the build directory of one account.
/// Dropping it removes the directory.
pub struct Workspace {
    /// The directory.
    dir: PathBuf,
    /// The account of the profiles.
    account: Account,
    /// The app identifier of the storage of the saved servers.
    identifier: String,
}

impl Workspace {
    /// Makes the directory and writes the configuration for `account`. The CLI uses the
    /// storage of the app identifier `identifier`.
    ///
    /// The configuration has two profiles of one server with different branches. Only the
    /// primary profile runs in the world and places the spawn.
    ///
    /// # Panics
    ///
    /// When the directory or the configuration cannot be written.
    pub fn new(account: Account, identifier: &str) -> Self {
        let workspace = Self::empty(account, identifier);
        workspace.write_config();
        workspace
    }

    /// Makes the directory and writes a configuration without `[servers]`: its one profile
    /// [`SAVED`] names the saved server [`SAVED_SERVER`].
    ///
    /// # Panics
    ///
    /// When the directory or the configuration cannot be written.
    pub fn saved(account: Account, identifier: &str) -> Self {
        let workspace = Self::empty(account, identifier);
        let config = format!(
            "dir = \"dist\"\n\
             \n\
             [profiles.{SAVED}]\n\
             server = \"{SAVED_SERVER}\"\n\
             branch = \"{SAVED}\"\n"
        );
        fs::write(workspace.dir.join("overseer.toml"), config).expect("write the configuration");
        workspace
    }

    /// Makes the directory.
    fn empty(account: Account, identifier: &str) -> Self {
        let dir = std::env::temp_dir().join(support::unique("overseer-cli-e2e-"));
        fs::create_dir(&dir).unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));
        Self {
            dir,
            account,
            identifier: identifier.to_owned(),
        }
    }

    /// Writes `overseer.toml`. The password is not in the file: the CLI reads it from
    /// [`PASSWORD_VAR`].
    fn write_config(&self) {
        let quote = |text: &str| serde_json::to_string(text).expect("a TOML basic string");
        let selector = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/live/selector.js");
        let answer = self.answer_path();
        let argv = [
            quote("node"),
            quote(&selector.to_string_lossy()),
            quote(&answer.to_string_lossy()),
        ];
        let config = format!(
            "dir = \"dist\"\n\
             default = [\"{PRIMARY}\", \"{MIRROR}\"]\n\
             \n\
             [servers.local]\n\
             url = {url}\n\
             email = {user}\n\
             password = {{ env = \"{PASSWORD_VAR}\" }}\n\
             \n\
             [profiles.{PRIMARY}]\n\
             server = \"local\"\n\
             branch = \"{PRIMARY}\"\n\
             activate = [\"world\"]\n\
             \n\
             [profiles.{PRIMARY}.spawn]\n\
             selector = [{argv}]\n\
             radius = {RADIUS}\n\
             \n\
             [profiles.{MIRROR}]\n\
             server = \"local\"\n\
             branch = \"{MIRROR}\"\n",
            url = quote(&support::url_text()),
            user = quote(&self.account.username),
            argv = argv.join(", "),
        );
        fs::write(self.dir.join("overseer.toml"), config).expect("write the configuration");
    }

    /// Writes the build directory `dist` with the main module `main` and its source map. With
    /// `wasm`, it also writes the `WebAssembly` module; without, it removes it.
    ///
    /// # Panics
    ///
    /// When a file cannot be written or removed.
    pub fn write_build(&self, main: &str, map: &str, wasm: Option<&[u8]>) {
        let dist = self.dir.join("dist");
        fs::create_dir_all(&dist).expect("create dist");
        fs::write(dist.join("main.js"), main).expect("write main.js");
        fs::write(dist.join("main.js.map"), map).expect("write main.js.map");
        let module = dist.join("main_bg.wasm");
        match wasm {
            Some(bytes) => fs::write(module, bytes).expect("write main_bg.wasm"),
            None => fs::remove_file(module).expect("remove main_bg.wasm"),
        }
    }

    /// The selector's last answer as it wrote it, or none when the selector did not run.
    ///
    /// # Panics
    ///
    /// When the answer file cannot be read or is not JSON.
    pub fn answer(&self) -> Option<serde_json::Value> {
        let text = match fs::read_to_string(self.answer_path()) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => panic!("read the selector answer: {error}"),
        };
        Some(serde_json::from_str(&text).expect("the selector answer is JSON"))
    }

    /// The file where the selector writes its answer.
    fn answer_path(&self) -> PathBuf {
        self.dir.join("selection.json")
    }

    /// Runs `overseer args` and requires success. Returns stdout. The test prints the output
    /// for diagnosis, with the password redacted.
    ///
    /// # Panics
    ///
    /// When the CLI does not start, runs longer than [`CLI_TIMEOUT`] or fails.
    pub fn succeed(&self, args: &[&str]) -> String {
        self.succeed_with_input(args, "")
    }

    /// Runs `overseer args` with `input` on stdin and requires success. Returns stdout.
    ///
    /// # Panics
    ///
    /// When the CLI does not start, runs longer than [`CLI_TIMEOUT`] or fails.
    pub fn succeed_with_input(&self, args: &[&str], input: &str) -> String {
        let (status, stdout, stderr) = self.run(args, input);
        eprintln!("$ overseer {}\n{stdout}{stderr}{status}", args.join(" "));
        assert!(status.success(), "overseer {} failed", args.join(" "));
        stdout
    }

    /// Runs `overseer args` in the directory with the configuration, the password and the app
    /// identifier, without the CLI's own variables of the caller. Writes `input` to stdin.
    /// Returns the exit status, stdout and stderr, with the password redacted.
    fn run(&self, args: &[&str], input: &str) -> (ExitStatus, String, String) {
        let stdout_path = self.dir.join("cli.stdout");
        let stderr_path = self.dir.join("cli.stderr");
        let stdout = File::create(&stdout_path).expect("create the stdout file");
        let stderr = File::create(&stderr_path).expect("create the stderr file");
        let mut child = Command::new(env!("CARGO_BIN_EXE_overseer"))
            .arg("--config")
            .arg(self.dir.join("overseer.toml"))
            .args(args)
            .current_dir(&self.dir)
            .env(PASSWORD_VAR, &self.account.password)
            .env(IDENTIFIER_VAR, &self.identifier)
            .env_remove("OVERSEER_CONFIG")
            .env_remove("OVERSEER_ENV_FILE")
            .env_remove("OVERSEER_PROFILE")
            .stdin(Stdio::piped())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .unwrap_or_else(|error| panic!("overseer {}: {error}", args.join(" ")));
        // Dropping stdin closes it: the CLI reads only `input`.
        let mut stdin = child.stdin.take().expect("the stdin of the CLI");
        stdin
            .write_all(input.as_bytes())
            .expect("write the stdin of the CLI");
        drop(stdin);
        let deadline = Instant::now() + CLI_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait for the CLI") {
                break status;
            }
            if Instant::now() >= deadline {
                // The CLI can have stopped between the check and the kill.
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "overseer {} ran longer than {} s",
                    args.join(" "),
                    CLI_TIMEOUT.as_secs()
                );
            }
            thread::sleep(CLI_POLL);
        };
        let read = |path: &Path| {
            let bytes = fs::read(path).expect("read the CLI output");
            String::from_utf8_lossy(&bytes).replace(&self.account.password, "<redacted>")
        };
        (status, read(&stdout_path), read(&stderr_path))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.dir) {
            eprintln!("cannot remove {}: {error}", self.dir.display());
        }
    }
}
