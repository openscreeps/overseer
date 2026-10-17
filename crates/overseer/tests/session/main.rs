//! Sessions against an in-process server: subscription reference counting, room snapshots, the
//! console, connection status and token updates, token refresh, the account, messages, market
//! and users.

mod account;
mod console;
mod live;
mod market;
mod messages;
mod refresh;
#[expect(dead_code, reason = "only the world harness holds responses")]
mod server;
mod status;
mod users;

use std::time::Duration;

use overseer::core::{ServerUrl, Token};
use overseer::{Auth, Overseer, ServerProfile};

use server::{Request, Server, Ws};

/// `GET /api/version` of a single-world server.
const VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"renderer":{"resources":{},"metadata":{}}}}"#;

/// `GET /api/auth/me` of user `u1`.
const ME: &str = r#"{"ok":1,"_id":"u1","money":0,"powerExperimentations":0}"#;

/// Upper bound of every wait in these tests.
const PATIENCE: Duration = Duration::from_secs(10);

/// A single-world server with user `u1` (any token), terrain for every room and console
/// commands.
fn routes(request: &Request) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        (_, "/api/version") => (200, VERSION.to_owned()),
        (_, "/api/auth/me") if request.token.is_some() => (200, ME.to_owned()),
        (_, "/api/game/room-terrain") => {
            let room = request
                .query
                .split('&')
                .find_map(|pair| pair.strip_prefix("room="))
                .unwrap_or_default();
            (
                200,
                format!(
                    r#"{{"ok":1,"terrain":[{{"_id":"t","room":"{room}","terrain":"1{}","type":"terrain"}}]}}"#,
                    "0".repeat(2499)
                ),
            )
        }
        ("POST", "/api/user/console") => (200, r#"{"ok":1}"#.to_owned()),
        _ => (401, r#"{"error":"unauthorized"}"#.to_owned()),
    }
}

/// Connects user `u1` with token `t1` and completes the socket handshake: the server answers
/// `auth ok` (rotating the token to `t2`) and receives the session's own `server-message`
/// subscription.
async fn connect(server: &mut Server) -> (Overseer, Ws) {
    let url = server.url();
    connect_as(server, url).await
}

/// [`connect`] with the server known as `url` (for example a stand-in for screeps.com,
/// [`ServerUrl::proxied`]).
async fn connect_as(server: &mut Server, url: ServerUrl) -> (Overseer, Ws) {
    let profile = ServerProfile {
        url,
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
