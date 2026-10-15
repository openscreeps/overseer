//! Idle clients: the socket opens after the server closed the client's pooled keep-alive
//! connection (Node closes idle ones after 5 s).

use std::time::Duration;

use overseer_net::SocketEvent;
use overseer_net::core::socket::Event;

use crate::support;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn the_socket_opens_and_authenticates_after_the_client_idled() {
    let account = support::register().await;
    let client = account.client();
    client.world().await.expect("sign-in");
    // Past Node's 5 s keep-alive timeout: the pooled connection of the sign-in is closed.
    tokio::time::sleep(Duration::from_secs(6)).await;

    let mut socket = client.connect().await.expect("the socket opens");
    support::until(&mut socket, |_, event| match event {
        SocketEvent::Event(Event::Authenticated) => Some(()),
        SocketEvent::Event(Event::AuthFailed) => panic!("the session token is rejected"),
        _ => None,
    })
    .await;
    socket.close().await.expect("a clean close");
}
