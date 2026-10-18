//! What a failed request can have done on the server: `Error::outcome` over every attempt,
//! sign-ins and retries included.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use overseer_core::api::official::endpoints::UserRespawn;
use overseer_core::model::platform::ServerKind;
use overseer_core::{ServerUrl, Token};
use overseer_net::{Client, Error, Outcome, RateLimitPolicy};
use tokio::io::AsyncReadExt as _;
use tokio::net::TcpListener;

use crate::server::{Request, Server};

/// The URL of a port that nothing listens on.
async fn unreachable() -> ServerUrl {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    drop(listener);
    ServerUrl::parse(&url).unwrap()
}

/// Retries HTTP 429 after 10 ms.
const QUICK_RETRY: RateLimitPolicy = RateLimitPolicy {
    max_wait: Duration::ZERO,
    global_delay: Duration::from_millis(10),
};

#[tokio::test]
async fn requests_that_never_left_the_client_are_not_applied() {
    let client = Client::builder(unreachable().await)
        .token(Token::new("t"))
        .build()
        .unwrap();
    let error = client.send(&UserRespawn).await.unwrap_err();
    assert!(matches!(error, Error::Http(_)), "{error:?}");
    assert_eq!(error.outcome(), Outcome::NotApplied);

    // The sign-in before the request could not connect either.
    let client = Client::builder(unreachable().await)
        .credentials("player@example.com", "secret")
        .build()
        .unwrap();
    let error = client.send(&UserRespawn).await.unwrap_err();
    assert_eq!(error.outcome(), Outcome::NotApplied, "{error:?}");

    // A request that cannot be built is not sent.
    let server = Server::start(|_: &Request| (200, r#"{"ok":1}"#.to_owned()), |_| true).await;
    let client = Client::builder(server.url()).build().unwrap();
    let error = client.send(&UserRespawn).await.unwrap_err();
    assert_eq!(error.outcome(), Outcome::NotApplied, "{error:?}");
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn a_connection_lost_after_the_request_was_written_is_unknown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        // Reads the whole request, then closes the connection without an answer.
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.ends_with(b"\r\n\r\n") {
            let read = stream.read(&mut buffer).await.unwrap();
            assert_ne!(read, 0, "the request ended early");
            request.extend_from_slice(&buffer[..read]);
        }
    });
    let client = Client::builder(ServerUrl::parse(&url).unwrap())
        .token(Token::new("t"))
        .build()
        .unwrap();
    let error = client.send(&UserRespawn).await.unwrap_err();
    assert!(matches!(error, Error::Http(_)), "{error:?}");
    assert_eq!(error.outcome(), Outcome::Unknown);
}

#[tokio::test]
async fn every_answer_of_the_server_but_429_is_unknown() {
    let replies = [
        (401, r#"{"error":"unauthorized"}"#),
        (429, "{}"),
        (500, r#"{"error":"broken"}"#),
        // A logical failure, which a handler can also report after it changed state.
        (200, r#"{"error":"room busy"}"#),
        // Success replies that do not decode.
        (200, "{"),
        (200, "{}"),
    ];
    let calls = AtomicUsize::new(0);
    let server = Server::start(
        move |_: &Request| {
            let (status, body) = replies[calls.fetch_add(1, Ordering::SeqCst)];
            (status, body.to_owned())
        },
        |_| true,
    )
    .await;
    let client = Client::builder(server.url())
        .token(Token::new("t"))
        .rate_limit(None)
        .build()
        .unwrap();
    for (status, _) in replies {
        let error = client.send(&UserRespawn).await.unwrap_err();
        assert!(matches!(error, Error::Api(_)), "{error:?}");
        // Every server refuses a request over a rate limit before it runs the request.
        let expected = if status == 429 {
            Outcome::NotApplied
        } else {
            Outcome::Unknown
        };
        assert_eq!(error.outcome(), expected, "{error:?}");
    }
}

#[tokio::test]
async fn a_retry_after_http_429_that_is_not_sent_is_not_applied() {
    let server = Server::start_refusing_after(|_: &Request| (429, "{}".to_owned()), 1).await;
    let client = Client::builder(server.url())
        .token(Token::new("t"))
        .rate_limit(Some(QUICK_RETRY))
        .build()
        .unwrap();
    let error = client.send(&UserRespawn).await.unwrap_err();
    // The refused attempt did not run, and the retry could not connect.
    assert!(matches!(error, Error::Http(_)), "{error:?}");
    assert_eq!(error.outcome(), Outcome::NotApplied);
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn a_retry_after_http_401_that_is_not_sent_stays_unknown() {
    let routes = |request: &Request| match request.path.as_str() {
        "/api/auth/signin" => (200, r#"{"ok":1,"token":"renewed"}"#.to_owned()),
        _ => (401, r#"{"error":"unauthorized"}"#.to_owned()),
    };
    // One connection: the sign-in cannot connect. Two: the sign-in succeeds, and the retry
    // cannot connect.
    for (connections, sent) in [
        (1, ["/api/user/respawn"].as_slice()),
        (2, &["/api/user/respawn", "/api/auth/signin"]),
    ] {
        let server = Server::start_refusing_after(routes, connections).await;
        let client = Client::builder(server.url())
            .server_kind(ServerKind::OpenScreeps)
            .token(Token::new("stale"))
            .credentials("player@example.com", "secret")
            .build()
            .unwrap();
        let error = client.send(&UserRespawn).await.unwrap_err();
        let Error::RetryNotSent { source } = &error else {
            panic!("a retry that was not sent, not {error:?}");
        };
        assert!(matches!(**source, Error::Http(_)), "{source:?}");
        assert_eq!(error.outcome(), Outcome::Unknown);
        let paths: Vec<_> = server.requests().into_iter().map(|r| r.path).collect();
        assert_eq!(paths, sent);
    }
}
