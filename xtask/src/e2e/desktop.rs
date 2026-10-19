//! The desktop suite (`cargo xtask e2e --desktop`, Windows only): the Tauri shell
//! (`app/src-tauri`) built for testing and driven by Playwright over the remote debugging
//! protocol (CDP) of its `WebView2` (`e2e/desktop.config.ts`, specs in `e2e/desktop/`).
//!
//! The test build is the release build (`tauri/custom-protocol`: the web app embedded, as
//! `cargo tauri build` makes it) with `TAURI_CONFIG` merged into `tauri.conf.json`:
//! - the overlay `app/src-tauri/tauri.e2e.conf.json`: its own identifier
//!   (`com.openscreeps.overseer.e2e`), so its `WebView2` profile, saved window state and
//!   keychain entries (service `overseer:<identifier>`) are apart from the app's
//!   (`com.openscreeps.overseer`, service `overseer`), which the suite never reads or writes;
//!   and the window controls (move, resize, maximize, minimize, close) the tests use in place
//!   of a user's clicks and drags on a window's frame;
//! - from here: the web app the harness built (`build.frontendDist`, relative: an absolute
//!   Windows path would parse as a URL) and remote debugging on a free port in the main
//!   window's `WebView2` arguments (`--remote-debugging-port=0`; route windows share them). The
//!   tests read the port from `DevToolsActivePort` in the profile. The `WEBVIEW2_*`
//!   environment variables cannot do it: the arguments the shell passes take precedence.
//!
//! The executable `overseer-ui.exe` is copied to `<target>/e2e/desktop/overseer-e2e.exe`: a
//! running test app never blocks a rebuild, and it is told apart from the user's own
//! `overseer-ui.exe`. The tests write its process id to `overseer-e2e.pid` next to it while it
//! runs.
//!
//! [`clean`] runs before the tests, [`finish`] after them and [`teardown`] however the run
//! ends: they stop a test app left running and delete the identifier's data directories
//! (`%APPDATA%\<identifier>`, `%LOCALAPPDATA%\<identifier>`) and keychain entries.

use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{cargo, status};
use crate::Error;

/// The test build's overlay of `tauri.conf.json`, relative to the workspace.
const OVERLAY: &str = "app/src-tauri/tauri.e2e.conf.json";
/// The shell's configuration directory, relative to the workspace.
const SHELL: &str = "app/src-tauri";
/// The identifier of the app itself, which the test build must not share.
const APP_IDENTIFIER: &str = "com.openscreeps.overseer";
/// The test build's executable name.
pub const EXECUTABLE: &str = "overseer-e2e.exe";
/// The `WebView2` arguments `wry` passes when the configuration names none: keep them.
const WRY_DEFAULT_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";
/// How long a data directory may stay locked by `WebView2` processes that are shutting down.
const UNLOCK: Duration = Duration::from_secs(20);

/// The test build's identifier, from the overlay; never the app's own.
pub fn identifier(root: &Path) -> Result<String, Error> {
    let overlay = read_json(&root.join(OVERLAY))?;
    let identifier = overlay
        .get("identifier")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{OVERLAY} names no identifier"))?;
    if identifier == APP_IDENTIFIER || identifier.is_empty() {
        return Err(format!("{OVERLAY} must give the test build an identifier of its own").into());
    }
    Ok(identifier.to_owned())
}

/// Builds the test app with the web app in `dist` and copies it to `executable`.
pub fn build(root: &Path, target: &Path, dist: &Path, executable: &Path) -> Result<(), Error> {
    let config = test_config(root, dist)?;
    status(
        cargo(root)
            .args(["build", "--package", "overseer-app-tauri", "--release"])
            .args(["--features", "tauri/custom-protocol"])
            .env("TAURI_CONFIG", config.to_string()),
        "cargo build (desktop app)",
    )?;
    if let Some(dir) = executable.parent() {
        fs::create_dir_all(dir)?;
    }
    let built = target
        .join("release")
        .join(format!("overseer-ui{}", std::env::consts::EXE_SUFFIX));
    fs::copy(&built, executable)
        .map_err(|error| format!("cannot copy {}: {error}", built.display()))?;
    Ok(())
}

/// `TAURI_CONFIG` of the test build (see the module docs).
fn test_config(root: &Path, dist: &Path) -> Result<Value, Error> {
    let mut config = read_json(&root.join(OVERLAY))?;
    let base = read_json(&root.join(SHELL).join("tauri.conf.json"))?;
    let mut windows = base
        .pointer("/app/windows")
        .and_then(Value::as_array)
        .cloned()
        .ok_or("tauri.conf.json has no app.windows")?;
    let main = windows
        .iter_mut()
        .find(|window| window.get("label").and_then(Value::as_str) == Some("main"))
        .ok_or("tauri.conf.json has no main window")?;
    let arguments = main
        .get("additionalBrowserArgs")
        .and_then(Value::as_str)
        .unwrap_or(WRY_DEFAULT_ARGS);
    main["additionalBrowserArgs"] = format!("{arguments} --remote-debugging-port=0").into();
    config["app"]["windows"] = windows.into();
    config["build"]["frontendDist"] = relative(&root.join(SHELL), dist)?.into();
    Ok(config)
}

/// The JSON file at `path`.
fn read_json(path: &Path) -> Result<Value, Error> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("{} is no JSON: {error}", path.display()).into())
}

/// `to` relative to the directory `from`, with `/` separators.
fn relative(from: &Path, to: &Path) -> Result<String, Error> {
    let from = std::path::absolute(from)?;
    let to = std::path::absolute(to)?;
    let from: Vec<Component<'_>> = from.components().collect();
    let to: Vec<Component<'_>> = to.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return Err("the web app must be built on the workspace's drive".into());
    }
    let mut parts = vec!["..".to_owned(); from.len() - common];
    for component in &to[common..] {
        let part = component
            .as_os_str()
            .to_str()
            .ok_or("the web app's path is not UTF-8")?;
        parts.push(part.to_owned());
    }
    Ok(parts.join("/"))
}

/// What the test app may leave behind; taken by [`finish`] or [`teardown`].
struct Cleanup {
    identifier: String,
    /// Where the tests write the running app's process id.
    pid_file: PathBuf,
}

static CLEANUP: Mutex<Option<Cleanup>> = Mutex::new(None);

/// Before the tests: registers the test app's state for [`finish`] and [`teardown`] and
/// removes what an earlier run left of it. Returns what it removed.
pub fn clean(identifier: &str, executable: &Path) -> Result<Vec<String>, Error> {
    let cleanup = Cleanup {
        identifier: identifier.to_owned(),
        pid_file: executable.with_extension("pid"),
    };
    let removed = remove(&cleanup)?;
    *CLEANUP.lock().unwrap_or_else(PoisonError::into_inner) = Some(cleanup);
    Ok(removed)
}

/// After the tests: removes what they left of the test app, then checks that nothing is left.
/// Returns what it removed (the tests clean up after themselves: normally nothing).
pub fn finish() -> Result<Vec<String>, Error> {
    let Some(cleanup) = CLEANUP
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    else {
        return Ok(Vec::new());
    };
    let removed = remove(&cleanup)?;
    let mut left = credentials(&service(&cleanup.identifier))?;
    left.extend(
        data_dirs(&cleanup.identifier)
            .into_iter()
            .filter(|dir| dir.exists())
            .map(|dir| dir.display().to_string()),
    );
    if running(&cleanup.pid_file).is_some() {
        left.push("the test app".to_owned());
    }
    if !left.is_empty() {
        return Err(format!("left behind: {}", left.join(", ")).into());
    }
    Ok(removed)
}

/// Removes the test app's state if [`clean`] registered it and [`finish`] did not run (a
/// failure, a panic or Ctrl-C).
pub fn teardown() {
    let Some(cleanup) = CLEANUP
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
    else {
        return;
    };
    if let Err(error) = remove(&cleanup) {
        eprintln!("warning: cannot remove the desktop test app's state: {error}");
    }
}

/// Stops the test app if it runs, deletes its keychain entries and data directories; returns
/// what it removed.
fn remove(cleanup: &Cleanup) -> Result<Vec<String>, Error> {
    let mut removed = Vec::new();
    if let Some(pid) = running(&cleanup.pid_file) {
        // `/T`: its `WebView2` processes too.
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
        removed.push(format!("the test app (process {pid})"));
    }
    let _ = fs::remove_file(&cleanup.pid_file);
    for target in credentials(&service(&cleanup.identifier))? {
        status(
            Command::new("cmdkey")
                .arg(format!("/delete:{target}"))
                .stdout(Stdio::null()),
            "cmdkey /delete",
        )?;
        removed.push(format!("credential {target}"));
    }
    for dir in data_dirs(&cleanup.identifier) {
        if remove_dir(&dir)? {
            removed.push(dir.display().to_string());
        }
    }
    Ok(removed)
}

/// The keychain service of the app with identifier `identifier` (see `app/src-tauri`'s
/// `secrets.rs`).
fn service(identifier: &str) -> String {
    format!("overseer:{identifier}")
}

/// The Windows credential targets of `service`: `<key>.<service>` (the `keyring` crate's
/// default target name).
fn credentials(service: &str) -> Result<Vec<String>, Error> {
    let output = Command::new("cmdkey")
        .arg(format!("/list:*.{service}"))
        .output()
        .map_err(|error| format!("cannot run cmdkey: {error}"))?;
    if !output.status.success() {
        return Err(format!("cmdkey /list failed ({})", output.status).into());
    }
    let suffix = format!(".{service}");
    // `Target: <target>` lines (the label may be translated); `User:` lines name the key only.
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().split_once(": "))
        .map(|(_, value)| value.trim())
        .filter(|target| target.ends_with(&suffix))
        .map(str::to_owned)
        .collect())
}

/// The data directories of the app with identifier `identifier`: its configuration (saved
/// window state) and its local data (the `WebView2` profile).
fn data_dirs(identifier: &str) -> Vec<PathBuf> {
    ["APPDATA", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|base| PathBuf::from(base).join(identifier))
        .collect()
}

/// Removes `dir` and its content, waiting while `WebView2` processes that are shutting down
/// hold files in it. Returns whether it existed.
fn remove_dir(dir: &Path) -> Result<bool, Error> {
    let deadline = Instant::now() + UNLOCK;
    let mut existed = false;
    loop {
        match fs::remove_dir_all(dir) {
            Ok(()) => return Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(existed),
            Err(_) if Instant::now() < deadline => {
                existed = true;
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => {
                return Err(format!("cannot remove {}: {error}", dir.display()).into());
            }
        }
    }
}

/// The process id in `pid_file` when that process is the test app and still runs.
fn running(pid_file: &Path) -> Option<u32> {
    let pid: u32 = fs::read_to_string(pid_file).ok()?.trim().parse().ok()?;
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    // The first column is the image name (never translated).
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .any(|line| line.starts_with(&format!("\"{EXECUTABLE}\"")))
        .then_some(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_keeps_the_app_apart() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let identifier = identifier(root).unwrap();
        assert_ne!(identifier, APP_IDENTIFIER);

        let config = test_config(root, &root.join("target/e2e/desktop/dist")).unwrap();
        assert_eq!(config["identifier"], identifier.as_str());
        assert_eq!(
            config["build"]["frontendDist"],
            "../../target/e2e/desktop/dist"
        );
        let windows = config["app"]["windows"].as_array().unwrap();
        let main = windows
            .iter()
            .find(|window| window["label"] == "main")
            .unwrap();
        let arguments = main["additionalBrowserArgs"].as_str().unwrap();
        assert!(
            arguments.ends_with(" --remote-debugging-port=0"),
            "{arguments}"
        );
        assert!(arguments.starts_with("--disable-features="), "{arguments}");
        // The rest of the main window is the app's.
        assert_eq!(main["title"], "Overseer");
        assert_eq!(service(&identifier), format!("overseer:{identifier}"));
    }

    #[test]
    fn relative_paths() {
        let base = std::path::absolute("a/b").unwrap();
        assert_eq!(relative(&base, &base.join("c/d")).unwrap(), "c/d");
        assert_eq!(
            relative(&base, &base.parent().unwrap().join("x")).unwrap(),
            "../x"
        );
    }
}
