//! The socket relayed by the proxy: the client's commands reach the server and its events come
//! back, and upgrades follow the origin policy.

use async_tungstenite::tokio::connect_async;
use async_tungstenite::tungstenite::client::IntoClientRequest as _;
use async_tungstenite::tungstenite::{Error as WsError, Message};
use futures_util::StreamExt as _;
use http::{HeaderValue, StatusCode};
use overseer_net::SocketEvent;
use overseer_net::core::socket::{Event, Payload};
use overseer_net::model::socket::UserCpuCpu;
use overseer_net::model::world::GameRoomObject;
use overseer_proxy::Proxy;

use crate::support::{self, PATIENCE};

const ALLOWED: &str = "http://allowed.example";
const DENIED: &str = "http://denied.example";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn the_socket_is_relayed_both_ways() {
    let proxy = support::start(None, &[]).await;
    let server = support::via(&proxy);
    let player = support::player(&server).await;
    let client = player.account.client(&server);
    let user = client
        .world()
        .await
        .expect("sign-in")
        .user()
        .expect("a user")
        .to_owned();
    let before = client.token();
    let mut socket = client
        .connect()
        .await
        .expect("the socket opens through the proxy");
    socket.subscribe_cpu().expect("signed in");
    socket
        .watch_room(&player.room)
        .await
        .expect("the room's terrain");
    support::until(&mut socket, |_, event| match event {
        SocketEvent::Event(Event::Authenticated) => Some(()),
        SocketEvent::Event(Event::AuthFailed) => panic!("the session token is rejected"),
        _ => None,
    })
    .await;
    // The `auth ok` token came back through the proxy.
    assert_ne!(client.token(), before);

    let (mut cpu, mut room) = (false, false);
    support::until(&mut socket, |socket, event| {
        if let SocketEvent::Event(Event::Channel { payload, .. }) = event {
            match payload {
                Payload::Cpu(usage) => {
                    assert!(matches!(usage.cpu, UserCpuCpu::Number(_)), "{usage:?}");
                    cpu = true;
                }
                Payload::Room => {
                    let view = socket.room_view(&player.room).expect("a watched room");
                    room |= view.objects().any(|(id, object)| {
                        matches!(
                            object.unwrap_or_else(|error| panic!("object {id}: {error}")),
                            GameRoomObject::Spawn(spawn) if spawn.user == user
                        )
                    });
                }
                other => panic!("unexpected payload {other:?}"),
            }
        }
        (cpu && room).then_some(())
    })
    .await;
    socket.close().await.expect("a clean close");
    proxy.shutdown().await;
}

/// Opens the proxied socket with a raw WebSocket client, sending `origin`.
async fn open(
    proxy: &Proxy,
    origin: &str,
) -> Result<async_tungstenite::WebSocketStream<async_tungstenite::tokio::ConnectStream>, WsError> {
    let url = proxy.url().join("socket/websocket").expect("a socket URL");
    let mut request = url
        .as_str()
        .replacen("http", "ws", 1)
        .into_client_request()
        .expect("a request");
    request.headers_mut().insert(
        "origin",
        HeaderValue::from_str(origin).expect("a header value"),
    );
    tokio::time::timeout(PATIENCE, connect_async(request))
        .await
        .expect("the handshake ends in time")
        .map(|(socket, _)| socket)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn socket_upgrades_follow_the_origin_policy() {
    let proxy = support::start(None, &[ALLOWED]).await;
    // An allowed page reaches the server, which greets with its clock.
    let mut socket = open(&proxy, ALLOWED).await.expect("an allowed upgrade");
    let greeting = loop {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .expect("a frame in time")
            .expect("an open socket")
            .expect("a frame");
        if let Message::Text(text) = message {
            break text.to_string();
        }
    };
    assert!(greeting.starts_with("time "), "{greeting}");
    socket.close(None).await.expect("a clean close");

    // A foreign page's upgrade is refused before reaching the server.
    match open(&proxy, DENIED).await {
        Err(WsError::Http(response)) => assert_eq!(response.status(), StatusCode::FORBIDDEN),
        other => panic!("expected a refused upgrade, got {:?}", other.map(|_| ())),
    }
    proxy.shutdown().await;
}
