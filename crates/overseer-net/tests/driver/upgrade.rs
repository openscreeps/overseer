//! Socket upgrades over pooled keep-alive connections: an upgrade whose connection the server
//! closed while idle is sent once more on a new connection; an answer is never retried.

use std::time::Duration;

use overseer_net::{Client, ClientBuilder, Error};
use reqwest_websocket::HandshakeError;

use crate::server::{Request, Server};
use crate::{PATIENCE, VERSION};

/// How long the server keeps an idle keep-alive connection.
const IDLE: Duration = Duration::from_millis(300);

fn single_world(request: &Request) -> (u16, String) {
    match request.path.as_str() {
        "/api/version" => (200, VERSION.to_owned()),
        _ => (404, "{}".to_owned()),
    }
}

/// Pools a connection with an HTTP request, lets it idle past the server's keep-alive timeout
/// and connects the socket.
async fn connect_after_idle(configure: impl FnOnce(ClientBuilder) -> ClientBuilder) {
    let server = Server::start_keep_alive(single_world, |_| true, IDLE).await;
    let client = configure(Client::builder(server.url())).build().unwrap();
    client.world().await.unwrap();
    assert_eq!(server.connections(), 1);
    tokio::time::sleep(IDLE * 2).await;

    let socket = tokio::time::timeout(PATIENCE, client.connect())
        .await
        .expect("connected in time");
    socket.expect("the retried upgrade succeeds");
    // Exactly two upgrade attempts: the first reused the pooled connection of `world`, where
    // the server dropped it unanswered (as Node does when its keep-alive timeout closes the
    // connection the request arrives on); the retry opened the second connection and was
    // accepted there.
    assert_eq!(server.dropped(), ["/socket/websocket"]);
    assert_eq!(server.upgrades(), 1);
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn an_upgrade_on_a_connection_closed_while_idle_is_retried_on_a_new_one() {
    connect_after_idle(|builder| builder).await;
}

#[tokio::test]
async fn a_custom_http_client_retries_a_lost_upgrade_too() {
    // reqwest's rustls needs a process-wide provider before a client is built; `build` of an
    // overseer client installs it only after this client exists. Err: already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let http = reqwest::Client::builder().http1_only().build().unwrap();
    connect_after_idle(|builder| builder.http_client(http)).await;
}

#[tokio::test]
async fn an_upgrade_answered_with_an_http_error_is_not_retried() {
    let server = Server::start_keep_alive(single_world, |_| false, PATIENCE).await;
    let client = Client::builder(server.url()).build().unwrap();
    client.world().await.unwrap();

    let error = client.connect().await.unwrap_err();
    let Error::WebSocket(error) = error else {
        panic!("a WebSocket error, not {error:?}");
    };
    assert!(
        matches!(
            *error,
            reqwest_websocket::Error::Handshake(HandshakeError::UnexpectedStatusCode(status))
                if status == 503
        ),
        "{error:?}"
    );
    // One upgrade request, answered with HTTP 503: not retried.
    assert_eq!(server.upgrades(), 1);
    assert_eq!(server.dropped(), Vec::<String>::new());
}
