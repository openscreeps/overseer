//! Code: branches, the active world and simulation branches, and code modules (JavaScript
//! text and WebAssembly binaries), over HTTP and from the socket's code channels.
//!
//! Every contract defines the code operations and events identically: the branch list, the
//! module map and the socket's code and branch events are the official types (codegen
//! re-exports them from the others), so each conversion here is written once.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::api::official::types::{UserCodeGetResponse, UserCodeModulesValue, UserCodeWasmModule};
use crate::socket::official::types as socket;

/// Every contract's branch documents (`GET /api/user/branches`) and module maps are these
/// types (codegen re-exports the official ones from the others).
pub use crate::api::official::types::{UserCodeBranch, UserCodeBranchTimestamp, UserCodeModules};
/// Every contract's upload hash of a code event is this type (codegen re-exports the official
/// one from the others).
pub use crate::socket::official::types::UserCodeHash;

/// Where a branch runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodeTarget {
    /// The world: the branch the player's creeps run.
    World,
    /// The simulator.
    Simulation,
}

impl CodeTarget {
    /// The branch name selecting the branch active in this target (`$activeWorld` or
    /// `$activeSim`), accepted wherever the modules of a branch are read or written.
    #[must_use]
    pub const fn selector(self) -> &'static str {
        match self {
            Self::World => "$activeWorld",
            Self::Simulation => "$activeSim",
        }
    }

    /// Whether the branch document `branch` marks the branch as active in this target. A
    /// document without the mark (`false` or absent) is not active.
    #[must_use]
    pub fn is_active(self, branch: &UserCodeBranch) -> bool {
        let marked = match self {
            Self::World => branch.active_world,
            Self::Simulation => branch.active_sim,
        };
        marked == Some(true)
    }
}

/// The names of the branches active in the world and in the simulator.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveBranches {
    /// The branch running in the world; `None` when no branch is marked.
    pub world: Option<String>,
    /// The branch running in the simulator; `None` when no branch is marked.
    pub simulation: Option<String>,
}

impl ActiveBranches {
    /// The branch active in `target`.
    #[must_use]
    pub fn get(&self, target: CodeTarget) -> Option<&str> {
        match target {
            CodeTarget::World => self.world.as_deref(),
            CodeTarget::Simulation => self.simulation.as_deref(),
        }
    }
}

/// The branches marked active in a branch list (`GET /api/user/branches`).
impl FromIterator<UserCodeBranch> for ActiveBranches {
    fn from_iter<I: IntoIterator<Item = UserCodeBranch>>(branches: I) -> Self {
        let mut active = Self::default();
        for branch in branches {
            match (
                CodeTarget::World.is_active(&branch),
                CodeTarget::Simulation.is_active(&branch),
            ) {
                (true, true) => {
                    active.world = Some(branch.branch.clone());
                    active.simulation = Some(branch.branch);
                }
                (true, false) => active.world = Some(branch.branch),
                (false, true) => active.simulation = Some(branch.branch),
                (false, false) => {}
            }
        }
        active
    }
}

/// The modules of a branch (`GET /api/user/code`), with the branch name the server resolved
/// (the active branch's name for [`CodeTarget::selector`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BranchCode {
    /// The branch name.
    pub branch: String,
    /// Every module of the branch.
    pub modules: Modules,
}

/// `From` the `GET /api/user/code` reply, which every contract shares.
impl From<UserCodeGetResponse> for BranchCode {
    fn from(reply: UserCodeGetResponse) -> Self {
        Self {
            branch: reply.branch,
            modules: reply.modules.into(),
        }
    }
}

/// The saved modules of a branch, as announced on `user:<user>/code`.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeChange {
    /// The branch name.
    pub branch: String,
    /// Every module of the branch after the change.
    pub modules: Modules,
    /// The `_hash` the uploading client sent, if any (the official web client sends a random
    /// number to recognize its own uploads).
    pub hash: Option<UserCodeHash>,
    /// Save time in milliseconds since the Unix epoch: the integer servers send for an upload,
    /// or the date (ISO 8601 UTC, as a JavaScript `Date` serializes) they send for a branch last
    /// saved by cloning. `None` when absent, before the epoch or of another form.
    pub timestamp: Option<u64>,
}

/// `From` the `user:<user>/code` event, which every contract shares.
impl From<socket::UserCode> for CodeChange {
    fn from(event: socket::UserCode) -> Self {
        let modules = event
            .modules
            .into_iter()
            .map(|(name, value)| {
                let value = match value {
                    socket::UserCodeModulesValue::String(source) => {
                        UserCodeModulesValue::String(source)
                    }
                    socket::UserCodeModulesValue::Object(object) => {
                        UserCodeModulesValue::WasmModule(UserCodeWasmModule {
                            binary: object.binary,
                        })
                    }
                };
                (name, value)
            })
            .collect::<UserCodeModules>();
        Self {
            branch: event.branch,
            modules: modules.into(),
            hash: event.hash,
            timestamp: event.timestamp.as_ref().and_then(millis),
        }
    }
}

/// A branch was activated, as announced on `user:<user>/set-active-branch`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveBranchChange {
    /// Where the branch now runs.
    pub target: CodeTarget,
    /// The branch name.
    pub branch: String,
}

/// `From` the `user:<user>/set-active-branch` event, which every contract shares.
impl From<socket::UserActiveBranch> for ActiveBranchChange {
    fn from(event: socket::UserActiveBranch) -> Self {
        let target = match event.active_name {
            socket::UserActiveBranchActiveName::ActiveWorld => CodeTarget::World,
            socket::UserActiveBranchActiveName::ActiveSim => CodeTarget::Simulation,
        };
        Self {
            target,
            branch: event.branch,
        }
    }
}

/// What a module holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModuleKind {
    /// JavaScript source text.
    Js,
    /// `WebAssembly` bytes (sent base64-encoded as `{binary}`).
    Wasm,
}

impl ModuleKind {
    /// The file extension of the module in a code directory: `js` or `wasm`.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Js => "js",
            Self::Wasm => "wasm",
        }
    }
}

/// A `WebAssembly` module whose `binary` is not standard base64.
#[derive(Debug, thiserror::Error)]
#[error("wasm module `{module}` is not valid base64: {source}")]
pub struct InvalidWasm {
    module: String,
    #[source]
    source: base64::DecodeError,
}

impl InvalidWasm {
    /// The module's name.
    #[must_use]
    pub fn module(&self) -> &str {
        &self.module
    }
}

/// The modules of a code branch by name: JavaScript source text or `WebAssembly` bytes.
///
/// Wraps the wire form ([`UserCodeModules`]), in which `WebAssembly` modules are base64 text:
/// [`Self::insert_wasm`] encodes and [`Self::wasm`] decodes it. Names are kept as given; the
/// server drops modules whose names start with `$`, `.`, `/` or `\` and adds an empty `main`
/// module when there is none.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Modules(UserCodeModules);

impl Modules {
    /// No modules.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of modules.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no modules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Every module's name and kind, ordered by name.
    pub fn iter(&self) -> impl Iterator<Item = (&str, ModuleKind)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), kind(value)))
    }

    /// The kind of module `name`; `None` when there is no such module.
    #[must_use]
    pub fn kind(&self, name: &str) -> Option<ModuleKind> {
        self.0.get(name).map(kind)
    }

    /// The source text of the JavaScript module `name`; `None` when there is no such module or
    /// it is a `WebAssembly` module.
    #[must_use]
    pub fn js(&self, name: &str) -> Option<&str> {
        match self.0.get(name)? {
            UserCodeModulesValue::String(source) => Some(source),
            UserCodeModulesValue::WasmModule(_) => None,
        }
    }

    /// The decoded bytes of the `WebAssembly` module `name`; `None` when there is no such
    /// module or it is a JavaScript module.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidWasm`] when the module's `binary` is not standard base64 (the server
    /// stores what clients upload without checking it).
    pub fn wasm(&self, name: &str) -> Result<Option<Vec<u8>>, InvalidWasm> {
        match self.0.get(name) {
            Some(UserCodeModulesValue::WasmModule(module)) => {
                decode(name, &module.binary).map(Some)
            }
            Some(UserCodeModulesValue::String(_)) | None => Ok(None),
        }
    }

    /// Sets module `name` to the JavaScript `source`, replacing any module of that name.
    pub fn insert_js(&mut self, name: impl Into<String>, source: impl Into<String>) {
        self.0
            .insert(name.into(), UserCodeModulesValue::String(source.into()));
    }

    /// Sets module `name` to the `WebAssembly` `bytes` (base64-encoded), replacing any module of
    /// that name.
    pub fn insert_wasm(&mut self, name: impl Into<String>, bytes: &[u8]) {
        let binary = STANDARD.encode(bytes);
        self.0.insert(
            name.into(),
            UserCodeModulesValue::WasmModule(UserCodeWasmModule { binary }),
        );
    }

    /// Removes module `name`; `false` when there was none.
    pub fn remove(&mut self, name: &str) -> bool {
        self.0.remove(name).is_some()
    }

    /// The wire form.
    #[must_use]
    pub const fn as_map(&self) -> &UserCodeModules {
        &self.0
    }
}

impl From<UserCodeModules> for Modules {
    fn from(modules: UserCodeModules) -> Self {
        Self(modules)
    }
}

impl From<Modules> for UserCodeModules {
    fn from(modules: Modules) -> Self {
        modules.0
    }
}

fn kind(value: &UserCodeModulesValue) -> ModuleKind {
    match value {
        UserCodeModulesValue::String(_) => ModuleKind::Js,
        UserCodeModulesValue::WasmModule(_) => ModuleKind::Wasm,
    }
}

fn decode(name: &str, binary: &str) -> Result<Vec<u8>, InvalidWasm> {
    STANDARD.decode(binary).map_err(|source| InvalidWasm {
        module: name.to_owned(),
        source,
    })
}

/// The save time of a code event in milliseconds since the Unix epoch: the integer as sent, or
/// the ISO 8601 UTC date-time (`2026-10-17T01:10:59.388Z`, as a JavaScript `Date` serializes)
/// that screeps.com and the Screeps server store for branches last saved by
/// `POST /api/user/clone-branch`. `None` for a string of another form or a date before the
/// epoch.
fn millis(timestamp: &socket::UserCodeTimestamp) -> Option<u64> {
    match timestamp {
        socket::UserCodeTimestamp::Integer(millis) => Some(*millis),
        socket::UserCodeTimestamp::String(text) => iso_millis(text),
    }
}

/// Milliseconds since the Unix epoch of `YYYY-MM-DDTHH:MM:SS[.fff]Z`.
fn iso_millis(text: &str) -> Option<u64> {
    let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<u64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock = clock.splitn(3, ':').map(str::parse::<u64>);
    let (hour, minute, second) = (
        clock.next()?.ok()?,
        clock.next()?.ok()?,
        clock.next()?.ok()?,
    );
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    if second > 60 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Milliseconds: the first three digits of the fraction.
    let millis = fraction
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(3)
        .fold(0, |millis, digit| millis * 10 + u64::from(digit - b'0'));
    let days = days_since_epoch(year, month, day)?;
    Some((((days * 24 + hour) * 60 + minute) * 60 + second) * 1000 + millis)
}

/// Days from 1970-01-01 to the proleptic Gregorian date (Howard Hinnant's `days_from_civil`);
/// `None` before the epoch.
fn days_since_epoch(year: u64, month: u64, day: u64) -> Option<u64> {
    let year = if month <= 2 {
        year.checked_sub(1)?
    } else {
        year
    };
    let era = year / 400;
    let year_of_era = year - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    (era * 146_097 + day_of_era).checked_sub(719_468)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// `\0asm` and version 1: the header of every `WebAssembly` module.
    const HEADER: [u8; 8] = [0, 0x61, 0x73, 0x6d, 1, 0, 0, 0];
    /// `HEADER` in standard base64 (generated with Python's `base64.b64encode`).
    const HEADER_BASE64: &str = "AGFzbQEAAAA=";

    #[test]
    fn wasm_modules_are_base64_on_the_wire() {
        let mut modules = Modules::new();
        modules.insert_js("main", "module.exports.loop = () => {};");
        modules.insert_wasm("engine", &HEADER);

        assert_eq!(
            serde_json::to_value(modules.as_map()).unwrap(),
            json!({
                "engine": { "binary": HEADER_BASE64 },
                "main": "module.exports.loop = () => {};",
            })
        );
        assert_eq!(
            modules.wasm("engine").unwrap().as_deref(),
            Some(&HEADER[..])
        );
        assert_eq!(modules.js("main"), Some("module.exports.loop = () => {};"));
        assert_eq!(modules.js("engine"), None);
        assert_eq!(modules.wasm("main").unwrap(), None);
        assert_eq!(
            modules.iter().collect::<Vec<_>>(),
            [("engine", ModuleKind::Wasm), ("main", ModuleKind::Js)]
        );
    }

    #[test]
    fn modules_from_the_wire_decode_on_access() {
        let wire: UserCodeModules = serde_json::from_value(json!({
            "engine": { "binary": HEADER_BASE64 },
            "broken": { "binary": "not base64!" },
            "main": "",
        }))
        .unwrap();
        let mut modules = Modules::from(wire);

        assert_eq!(modules.len(), 3);
        assert_eq!(modules.kind("engine"), Some(ModuleKind::Wasm));
        assert_eq!(
            modules.wasm("engine").unwrap().as_deref(),
            Some(&HEADER[..])
        );
        assert_eq!(modules.wasm("broken").unwrap_err().module(), "broken");
        assert!(modules.remove("broken"));
        assert!(!modules.remove("broken"));
        assert_eq!(modules.kind("broken"), None);
    }

    #[test]
    fn the_code_reply_names_the_resolved_branch() {
        let reply: UserCodeGetResponse = serde_json::from_value(json!({
            "ok": 1,
            "branch": "default",
            "modules": { "main": "x", "engine": { "binary": HEADER_BASE64 } },
        }))
        .unwrap();
        let code = BranchCode::from(reply);

        assert_eq!(code.branch, "default");
        assert_eq!(code.modules.js("main"), Some("x"));
        assert_eq!(code.modules.kind("engine"), Some(ModuleKind::Wasm));
    }

    #[test]
    fn active_branches_come_from_the_branch_list() {
        // A `GET /api/user/branches` reply: branches created by cloning lack the flags.
        let list: Vec<UserCodeBranch> = serde_json::from_value(json!([
            { "_id": "1", "branch": "default", "activeWorld": true, "activeSim": false },
            { "_id": "2", "branch": "sim", "activeWorld": false, "activeSim": true },
            { "_id": "3", "branch": "clone" },
        ]))
        .unwrap();
        assert_eq!(
            list.into_iter().collect::<ActiveBranches>(),
            ActiveBranches {
                world: Some("default".to_owned()),
                simulation: Some("sim".to_owned()),
            }
        );

        let both: Vec<UserCodeBranch> = serde_json::from_value(json!([
            { "_id": "1", "branch": "default", "activeWorld": true, "activeSim": true },
        ]))
        .unwrap();
        let active = both.into_iter().collect::<ActiveBranches>();
        assert_eq!(active.get(CodeTarget::World), Some("default"));
        assert_eq!(active.get(CodeTarget::Simulation), Some("default"));
        assert_eq!(ActiveBranches::default().get(CodeTarget::World), None);
    }

    #[test]
    fn code_events_become_changes() {
        // An upload: integer timestamp and the uploader's hash.
        let upload: socket::UserCode = serde_json::from_value(json!({
            "branch": "default",
            "modules": { "main": "x", "engine": { "binary": HEADER_BASE64 } },
            "timestamp": 1_792_199_459_388_u64,
            "hash": 7,
        }))
        .unwrap();
        let change = CodeChange::from(upload);
        assert_eq!(change.branch, "default");
        assert_eq!(change.modules.js("main"), Some("x"));
        assert_eq!(
            change.modules.wasm("engine").unwrap().as_deref(),
            Some(&HEADER[..])
        );
        assert_eq!(change.hash, Some(UserCodeHash::Number(7.0)));
        assert_eq!(change.timestamp, Some(1_792_199_459_388));

        // A clone: the date the server stores, no hash.
        let clone: socket::UserCode = serde_json::from_value(json!({
            "branch": "copy",
            "modules": { "main": "" },
            "timestamp": "2026-10-17T01:10:59.388Z",
        }))
        .unwrap();
        let change = CodeChange::from(clone);
        assert_eq!(change.hash, None);
        assert_eq!(change.timestamp, Some(1_792_199_459_388));
    }

    #[test]
    fn branch_events_name_the_target() {
        let event = |name: &str| -> socket::UserActiveBranch {
            serde_json::from_value(json!({ "activeName": name, "branch": "next" })).unwrap()
        };
        assert_eq!(
            ActiveBranchChange::from(event("activeWorld")),
            ActiveBranchChange {
                target: CodeTarget::World,
                branch: "next".to_owned(),
            }
        );
        assert_eq!(
            ActiveBranchChange::from(event("activeSim")).target,
            CodeTarget::Simulation
        );
    }

    #[test]
    fn integers_and_serialized_dates_are_milliseconds_since_the_epoch() {
        let text = |text: &str| millis(&socket::UserCodeTimestamp::String(text.to_owned()));
        assert_eq!(millis(&socket::UserCodeTimestamp::Integer(5)), Some(5));
        // `new Date(1792199459388).toISOString()`, as sent by the Screeps server.
        assert_eq!(text("2026-10-17T01:10:59.388Z"), Some(1_792_199_459_388));
        assert_eq!(text("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(text("2000-02-29T12:00:00.5Z"), Some(951_825_600_500));
        for invalid in [
            "2026-10-17",
            "2026-10-17T01:10:59",
            "2026-13-01T00:00:00Z",
            "x",
        ] {
            assert_eq!(text(invalid), None, "{invalid}");
        }
        assert_eq!(text("1969-12-31T23:59:59Z"), None);
    }
}
