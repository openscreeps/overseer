//! Read-only tests of a proxy in front of the server, as the [`support::reader`] account. They
//! only read (GET requests and the socket), so they also run against screeps.com
//! (`cargo xtask e2e --target official`) and catch drift of what the proxy forwards there.

use http::StatusCode;
use overseer_net::SocketEvent;
use overseer_net::core::socket::{Event, Payload};
use overseer_net::serde_json::{self, Value, json};

use crate::support;

/// The names of the `Access-Control-*` headers of `response` (only names: replies of
/// authenticated requests may carry a token).
fn cors_headers(response: &reqwest::Response) -> Vec<String> {
    response
        .headers()
        .keys()
        .map(|name| name.as_str().to_owned())
        .filter(|name| name.starts_with("access-control-"))
        .collect()
}

/// `api/version` and the reader's `auth/me` pass through to a same-origin page without CORS
/// headers, and the client discovers the world and the user through the proxy:
/// `GET /api/version` (raw), `GET /api/version` and `GET /api/auth/me` (the client's world),
/// `GET /api/auth/me` (raw, with the client's token).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live server: cargo xtask e2e (read-only, also --target official)"]
async fn version_and_the_readers_user_pass_through() {
    let proxy = support::start(None, &[]).await;
    let http = support::http();
    // A page the proxy serves sends its own origin.
    let origin = proxy.url().origin().ascii_serialization();

    let version = http
        .get(proxy.url().join("api/version").expect("a URL"))
        .header("Origin", &origin)
        .send()
        .await
        .expect("the version");
    assert_eq!(version.status(), StatusCode::OK);
    assert_eq!(cors_headers(&version), Vec::<String>::new());
    let version: Value =
        serde_json::from_str(&version.text().await.expect("a body")).expect("JSON");
    assert_eq!(version["ok"], json!(1));

    let client = support::reader(&support::via(&proxy));
    let world = client
        .world()
        .await
        .expect("the world and the user through the proxy");
    assert_eq!(world.is_sharded(), support::official());
    let user = world.user().expect("an authenticated user");
    let token = client.token().expect("a token");
    let me = http
        .get(proxy.url().join("api/auth/me").expect("a URL"))
        .header("Origin", &origin)
        .header("X-Token", token.as_str())
        .send()
        .await
        .expect("auth/me");
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(cors_headers(&me), Vec::<String>::new());
    let me: Value = serde_json::from_str(&me.text().await.expect("a body")).expect("JSON");
    assert_eq!(me["_id"].as_str(), Some(user));
    proxy.shutdown().await;
}

/// The proxy relays the reader's authenticated socket: a user channel (`user:<id>/cpu` on every
/// run of its code, `user:<id>/resources` at once on `OpenScreeps`) and its watched start room
/// come back decoded: `GET /api/version`, `GET /api/auth/me`, `GET /api/user/world-start-room`,
/// `GET /api/game/room-terrain`, the upgrade `GET /socket/websocket`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live server: cargo xtask e2e (read-only, also --target official)"]
async fn the_socket_relays_the_readers_subscriptions() {
    let proxy = support::start(None, &[]).await;
    let client = support::reader(&support::via(&proxy));
    client
        .world()
        .await
        .expect("the world and the user through the proxy");
    let (shard, room) = support::start_room(&client).await;
    let mut socket = client
        .connect()
        .await
        .expect("the socket opens through the proxy");
    socket.subscribe_cpu().expect("signed in");
    socket.subscribe_resources().expect("signed in");
    match &shard {
        Some(shard) => socket.watch_room_on(shard, &room).await,
        None => socket.watch_room(&room).await,
    }
    .expect("the start room's terrain");
    support::until(&mut socket, |_, event| match event {
        SocketEvent::Event(Event::Authenticated) => Some(()),
        SocketEvent::Event(Event::AuthFailed) => panic!("the token is rejected"),
        _ => None,
    })
    .await;

    let (mut user_channel, mut watched) = (false, false);
    support::until(&mut socket, |socket, event| {
        let SocketEvent::Event(event) = event else {
            return None;
        };
        match event {
            Event::Channel {
                payload: Payload::Cpu(_) | Payload::Resources(_),
                ..
            } => user_channel = true,
            Event::Channel {
                payload: Payload::Room,
                ..
            } => {
                let view = match &shard {
                    Some(shard) => socket.room_view_on(shard, &room),
                    None => socket.room_view(&room),
                }
                .expect("the watched start room");
                for (id, object) in view.objects() {
                    object.unwrap_or_else(|error| panic!("object {id} does not decode: {error}"));
                }
                watched |= view.game_time().is_some();
            }
            Event::ChannelError { channel, message } => panic!("{channel:?} failed: {message}"),
            Event::CannotSubscribe(line) => panic!("{line}"),
            _ => {}
        }
        (user_channel && watched).then_some(())
    })
    .await;
    socket.close().await.expect("a clean close");
    proxy.shutdown().await;
}
