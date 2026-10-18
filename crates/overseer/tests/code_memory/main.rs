//! Code branches, code directories and Memory: directory sync on temporary directories, and
//! requests, live data, shared Memory snapshots and observed writes against an in-process
//! server.

mod code;
mod dir;
mod memory;
#[path = "../session/server.rs"]
mod server;
mod snapshot;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use overseer::core::Token;
use overseer::{Auth, Overseer, ServerProfile};

use server::{Request, Server, Ws};

/// `GET /api/version` of a single-world `OpenScreeps` server (the `openscreeps` feature).
const VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"renderer":{"resources":{},"metadata":{}},"features":[{"name":"market","version":1},{"name":"auth","version":1},{"name":"openscreeps","version":1}]}}"#;

/// `GET /api/auth/me` of user `u1`.
const ME: &str = r#"{"ok":1,"_id":"u1","money":0,"powerExperimentations":0}"#;

/// `\0asm` version 1, the header of every `WebAssembly` module.
const WASM: [u8; 8] = [0, 0x61, 0x73, 0x6d, 1, 0, 0, 0];

/// [`WASM`] in base64 (generated with Python's `base64.b64encode`).
const WASM_BASE64: &str = "AGFzbQEAAAA=";

/// Upper bound of every wait in these tests.
const PATIENCE: Duration = Duration::from_secs(10);

/// A single-world server with user `u1` (any token), two branches (`default` active in the
/// world, `sim` in the simulator) holding a JavaScript and a `WebAssembly` module, Memory
/// `{"role":"harvester"}` at every path and segments holding `segment <id>`.
fn routes(request: &Request) -> (u16, String) {
    let ok = r#"{"ok":1}"#.to_owned();
    let timestamp = r#"{"ok":1,"timestamp":1700000000000}"#.to_owned();
    match (request.method.as_str(), request.path.as_str()) {
        (_, "/api/version") => (200, VERSION.to_owned()),
        (_, "/api/auth/me") if request.token.is_some() => (200, ME.to_owned()),
        ("GET", "/api/user/branches") => (
            200,
            r#"{"ok":1,"list":[{"_id":"b1","branch":"default","activeWorld":true,"activeSim":false},{"_id":"b2","branch":"sim","activeSim":true}]}"#.to_owned(),
        ),
        ("GET", "/api/user/code") => {
            let branch = match query(request, "branch") {
                Some("%24activeWorld" | "$activeWorld") | None => "default",
                Some("%24activeSim" | "$activeSim") => "sim",
                Some(branch) => branch,
            };
            (
                200,
                format!(
                    r#"{{"ok":1,"branch":"{branch}","modules":{{"main":"module.exports.loop = () => {{}};","engine":{{"binary":"{WASM_BASE64}"}}}}}}"#
                ),
            )
        }
        ("POST", "/api/user/code" | "/api/user/clone-branch" | "/api/user/delete-branch") => {
            (200, timestamp)
        }
        ("POST", "/api/user/set-active-branch" | "/api/user/memory-segment") => (200, ok),
        ("GET", "/api/user/memory") => (
            200,
            r#"{"ok":1,"data":"gz:H4sIAAAAAAAC/6tWKsrPSVWyUspILCpLLS5JLVKqBQD/pss5FAAAAA=="}"#
                .to_owned(),
        ),
        ("POST", "/api/user/memory") => (200, r#"{"ok":1,"insertedCount":1}"#.to_owned()),
        ("GET", "/api/user/memory-segment") => {
            let segment = query(request, "segment").unwrap_or_default();
            (200, format!(r#"{{"ok":1,"data":"segment {segment}"}}"#))
        }
        _ => (401, r#"{"error":"unauthorized"}"#.to_owned()),
    }
}

/// The value of query parameter `name`, as sent.
fn query<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// The bodies of the `method` requests to `path` served so far, as JSON.
fn bodies(server: &Server, method: &str, path: &str) -> Vec<serde_json::Value> {
    server
        .requests()
        .into_iter()
        .filter(|request| request.method == method && request.path == path)
        .map(|request| serde_json::from_str(&request.body).expect("a JSON body"))
        .collect()
}

/// The query strings of the `GET` requests to `path` served so far.
fn queries(server: &Server, path: &str) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter(|request| request.method == "GET" && request.path == path)
        .map(|request| request.query)
        .collect()
}

/// Connects user `u1` and completes the socket handshake.
async fn connect(server: &mut Server) -> (Overseer, Ws) {
    let url = server.url();
    connect_as(server, url).await
}

/// [`connect`] with the server known as `url` (for example a stand-in for screeps.com,
/// [`ServerUrl::proxied`](overseer::core::ServerUrl::proxied)).
async fn connect_as(server: &mut Server, url: overseer::core::ServerUrl) -> (Overseer, Ws) {
    let profile = ServerProfile {
        url,
        auth: Auth::Token(Token::new("t1")),
        server_password: None,
        default_shard: None,
        server_kind: None,
        plugins: None,
    };
    let overseer = Overseer::connect(profile).await.unwrap();
    let mut websocket = server.socket().await;
    assert_eq!(receive(&mut websocket, 2).await, ["gzip on", "auth t1"]);
    server::send(&mut websocket, "auth ok t2").await;
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe server-message"]
    );
    (overseer, websocket)
}

/// The next `count` text frames the client sends.
async fn receive(websocket: &mut Ws, count: usize) -> Vec<String> {
    tokio::time::timeout(PATIENCE, server::receive(websocket, count))
        .await
        .expect("frames in time")
}

/// Waits for `future` at most [`PATIENCE`].
async fn soon<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(PATIENCE, future)
        .await
        .expect("done in time")
}

/// A fresh directory under the system's temporary directory, deleted when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "overseer-code-memory-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `contents` to the file `name`.
    fn put(&self, name: &str, contents: impl AsRef<[u8]>) {
        fs::write(self.0.join(name), contents).unwrap();
    }

    /// The names of the entries, sorted; directories end with `/`.
    fn entries(&self) -> Vec<String> {
        let mut entries: Vec<_> = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name().into_string().unwrap();
                if entry.file_type().unwrap().is_dir() {
                    name + "/"
                } else {
                    name
                }
            })
            .collect();
        entries.sort();
        entries
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
