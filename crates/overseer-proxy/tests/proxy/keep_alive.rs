//! Pooled upstream connections that the upstream closed while idle, as Node does after 5 s: a
//! lost `GET` or socket upgrade is sent once more on a new connection, a lost `POST` never.

use std::time::Duration;

use async_tungstenite::tungstenite::Message;
use http::StatusCode;
use overseer_proxy::Proxy;

use crate::upstream::Upstream;
use crate::{client, next, open_socket, proxy};

/// How long the upstream keeps an idle keep-alive connection.
const IDLE: Duration = Duration::from_millis(300);

/// Starts an upstream that drops requests on connections idle for longer than [`IDLE`] and a
/// proxy in front of it, pools a proxy connection with a `GET` and lets it idle past [`IDLE`].
async fn idle_pooled_connection() -> (Upstream, Proxy) {
    let upstream = Upstream::start_closing_idle("/", IDLE).await;
    let proxy = proxy(&upstream.base, None, &[]).await;
    let response = client()
        .get(proxy.url().join("api/echo").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    assert_eq!(upstream.connections(), 1);
    tokio::time::sleep(IDLE * 2).await;
    (upstream, proxy)
}

#[tokio::test]
async fn a_get_on_a_connection_closed_while_idle_is_retried_on_a_new_one() {
    let (upstream, proxy) = idle_pooled_connection().await;

    let response = client()
        .get(proxy.url().join("api/echo?again=1").unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), r#"{"ok":1}"#);
    // The first attempt reused the pooled connection, where the upstream dropped it unanswered
    // (as Node does when its keep-alive timeout closes the connection the request arrives on);
    // the retry opened a second connection and was answered there, once.
    assert_eq!(upstream.dropped(), ["GET /api/echo?again=1"]);
    assert_eq!(upstream.connections(), 2);
    assert_eq!(upstream.targets(), ["/api/echo", "/api/echo?again=1"]);
    proxy.shutdown().await;
}

#[tokio::test]
async fn a_post_on_a_connection_closed_while_idle_is_not_sent_again() {
    let (upstream, proxy) = idle_pooled_connection().await;

    let response = client()
        .post(proxy.url().join("api/echo").unwrap())
        .body(r#"{"expression":"Game.market.deal()"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    // The upstream drops a request only once it has arrived: the POST was written to the pooled
    // connection, so a retry could repeat it. It is sent exactly once and fails.
    assert_eq!(upstream.dropped(), ["POST /api/echo"]);
    assert_eq!(upstream.connections(), 1);
    assert_eq!(upstream.targets(), ["/api/echo"]);
    proxy.shutdown().await;
}

#[tokio::test]
async fn a_socket_upgrade_on_a_connection_closed_while_idle_is_retried_on_a_new_one() {
    let (mut upstream, proxy) = idle_pooled_connection().await;

    let mut socket = open_socket(&proxy, None).await.unwrap();
    assert_eq!(upstream.socket_event().await, "open");
    assert_eq!(next(&mut socket).await, Message::text("o"));
    socket.send(Message::text("hello")).await.unwrap();
    assert_eq!(upstream.socket_event().await, "text:hello");
    assert_eq!(next(&mut socket).await, Message::text("echo:hello"));
    assert_eq!(upstream.dropped(), ["GET /socket/websocket"]);
    assert_eq!(upstream.connections(), 2);
    assert_eq!(upstream.targets(), ["/api/echo", "/socket/websocket"]);
    proxy.shutdown().await;
}
