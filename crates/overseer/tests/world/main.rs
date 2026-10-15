//! The world features against an in-process server: room actions, history playback and map
//! views.

mod history;
mod map;
mod rooms;
#[path = "../session/server.rs"]
mod server;

use std::time::Duration;

use overseer::core::Token;
use overseer::{Auth, Overseer, ServerProfile};

use server::{Request, Server, Ws};

/// `GET /api/version` of a single-world server with history chunks of 20 ticks.
const VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"renderer":{"resources":{},"metadata":{}}}}"#;

/// `GET /api/auth/me` of user `u1`.
const ME: &str = r#"{"ok":1,"_id":"u1","money":0,"powerExperimentations":0}"#;

/// Upper bound of every wait in these tests.
const PATIENCE: Duration = Duration::from_secs(10);

/// An encoded terrain document of `room`: a wall at (0, 0), plains elsewhere.
fn terrain_document(room: &str) -> String {
    format!(
        r#"{{"_id":"t","room":"{room}","terrain":"1{}","type":"terrain"}}"#,
        "0".repeat(2499)
    )
}

/// The value of `name` in a query string.
fn query<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.query.split('&').find_map(|pair| {
        pair.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
    })
}

/// Connects user `u1` with token `t1` to a server serving `routes` (which also answers
/// `/api/version` and `/api/auth/me`) and completes the socket handshake.
async fn connect(
    routes: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static,
) -> (Server, Overseer, Ws) {
    let mut server = Server::start(
        move |request: &Request| match request.path.as_str() {
            "/api/version" => (200, VERSION.to_owned()),
            "/api/auth/me" if request.token.is_some() => (200, ME.to_owned()),
            _ => routes(request),
        },
        |_| true,
    )
    .await;
    let profile = ServerProfile {
        url: server.url(),
        auth: Auth::Token(Token::new("t1")),
        server_password: None,
        default_shard: None,
    };
    let overseer = Overseer::connect(profile).await.unwrap();
    let mut websocket = server.socket().await;
    assert_eq!(receive(&mut websocket, 2).await, ["gzip on", "auth t1"]);
    server::send(&mut websocket, "auth ok t2").await;
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe server-message"]
    );
    (server, overseer, websocket)
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
