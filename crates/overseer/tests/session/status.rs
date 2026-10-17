//! Connection status through a dropped connection, token updates and shutdown.

use std::time::Duration;

use overseer::core::Token;
use overseer::{Closed, ConnectionStatus, Error};

use crate::server::Server;
use crate::{connect, receive, routes, soon};

#[tokio::test]
async fn status_and_subscriptions_survive_a_dropped_connection() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let mut status = overseer.status();
    soon(status.wait_for(|status| *status == ConnectionStatus::Connected))
        .await
        .unwrap();
    let mut room = overseer.rooms().watch("W1N1").await.unwrap();
    assert_eq!(receive(&mut websocket, 1).await, ["subscribe room:W1N1"]);

    websocket.close(None).await.unwrap();
    let reconnecting = *soon(status.wait_for(|status| *status != ConnectionStatus::Connected))
        .await
        .unwrap();
    assert_eq!(
        reconnecting,
        ConnectionStatus::Reconnecting {
            attempt: 1,
            delay: Duration::from_secs(1)
        }
    );
    // The new connection authenticates with the rotated token and resubscribes.
    let mut again = soon(server.socket()).await;
    soon(status.wait_for(|status| *status == ConnectionStatus::Connected))
        .await
        .unwrap();
    assert_eq!(receive(&mut again, 2).await, ["gzip on", "auth t2"]);
    crate::server::send(&mut again, "auth ok").await;
    assert_eq!(
        receive(&mut again, 2).await,
        ["subscribe server-message", "subscribe room:W1N1"]
    );
    overseer.shutdown().await;
    assert_eq!(*status.borrow(), ConnectionStatus::Closed);
    assert!(matches!(soon(room.changed()).await, Err(Closed)));
    assert!(matches!(
        overseer.rooms().watch("W1N1").await,
        Err(Error::Closed)
    ));
    // Shutting down twice is harmless.
    overseer.shutdown().await;
}

#[tokio::test]
async fn token_updates_include_auth_ok_rotations() {
    let mut server = Server::start(routes, |_| true).await;
    let profile = overseer::ServerProfile {
        url: server.url(),
        auth: overseer::Auth::Token(Token::new("t1")),
        server_password: None,
        default_shard: None,
        server_kind: None,
        plugins: None,
    };
    let overseer = overseer::Overseer::connect(profile).await.unwrap();
    let mut tokens = overseer.tokens();
    assert_eq!(*tokens.borrow_and_update(), Some(Token::new("t1")));
    let mut websocket = server.socket().await;
    assert_eq!(receive(&mut websocket, 2).await, ["gzip on", "auth t1"]);
    crate::server::send(&mut websocket, "auth ok t2").await;
    soon(tokens.changed()).await.unwrap();
    assert_eq!(*tokens.borrow_and_update(), Some(Token::new("t2")));
    assert_eq!(overseer.client().token(), Some(Token::new("t2")));
    assert_eq!(overseer.user_id(), Some("u1"));
    // A rotation that keeps the token is no change.
    crate::server::send(&mut websocket, "auth ok t2").await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), tokens.changed())
            .await
            .is_err()
    );
}
