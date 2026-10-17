//! Sign-in with credentials over HTTP and after the socket's `auth failed`, and `X-Token`
//! rotations.

use overseer_core::Token;
use overseer_core::api::ApiError;
use overseer_core::socket::Event;
use overseer_net::api::AccountApi as _;
use overseer_net::{Client, Error, SocketEvent};

use crate::server::{self, Request, Response, Server};
use crate::{ME, VERSION, descriptions_until};

/// A single-world `OpenScreeps` server that accepts the tokens `fresh` and `renewed`; sign-in
/// issues `renewed`.
fn accounts(request: &Request) -> (u16, String) {
    match (request.path.as_str(), request.token.as_deref()) {
        ("/api/version", _) => (200, VERSION.to_owned()),
        ("/api/auth/signin", _) => (200, r#"{"ok":1,"token":"renewed"}"#.to_owned()),
        ("/api/auth/me", Some("fresh" | "renewed")) => (200, ME.to_owned()),
        _ => (401, r#"{"error":"unauthorized"}"#.to_owned()),
    }
}

/// [`accounts`], whose `GET /api/auth/me` replies rotate the token to `rotated`.
fn rotating(request: &Request) -> Response {
    let mut response = Response::from(accounts(request));
    if request.path == "/api/auth/me" && response.status == 200 {
        response.headers.push(("X-Token", "rotated".to_owned()));
    }
    response
}

/// `(method, path, token)` of each request.
fn summary(server: &Server) -> Vec<(String, String, Option<String>)> {
    server
        .requests()
        .into_iter()
        .map(|request| (request.method, request.path, request.token))
        .collect()
}

fn entry(method: &str, path: &str, token: Option<&str>) -> (String, String, Option<String>) {
    (method.to_owned(), path.to_owned(), token.map(str::to_owned))
}

#[tokio::test]
async fn unauthorized_replies_sign_in_again_and_retry_once() {
    let server = Server::start(accounts, |_| true).await;
    let client = Client::builder(server.url())
        .token(Token::new("stale"))
        .credentials("player@example.com", "secret")
        .build()
        .unwrap();
    let me = client.api().await.unwrap().me().await.unwrap();
    assert_eq!(me.id, "u1");
    assert_eq!(client.token(), Some(Token::new("renewed")));
    assert_eq!(
        summary(&server),
        [
            // Discovery: password sign-in is native on this server.
            entry("GET", "/api/version", Some("stale")),
            entry("GET", "/api/auth/me", Some("stale")),
            entry("POST", "/api/auth/signin", Some("stale")),
            entry("GET", "/api/auth/me", Some("renewed")),
        ]
    );
    let sign_in = &server.requests()[2].body;
    assert!(
        sign_in.contains(r#""email":"player@example.com""#),
        "{sign_in}"
    );
    assert!(sign_in.contains(r#""password":"secret""#), "{sign_in}");
    // A sign-in is no rotation.
    assert!(!client.rotates_tokens());
}

#[tokio::test]
async fn credentials_sign_in_when_no_token_is_held() {
    let server = Server::start(accounts, |_| true).await;
    let client = Client::builder(server.url())
        .credentials("player@example.com", "secret")
        .build()
        .unwrap();
    client.api().await.unwrap().me().await.unwrap();
    assert_eq!(
        summary(&server),
        [
            entry("GET", "/api/version", None),
            entry("POST", "/api/auth/signin", None),
            entry("GET", "/api/auth/me", Some("renewed")),
        ]
    );
}

#[tokio::test]
async fn x_token_rotations_replace_the_token_and_are_reported() {
    let server = Server::start(rotating, |_| true).await;
    let client = Client::builder(server.url())
        .token(Token::new("stale"))
        .build()
        .unwrap();
    // A token set by the app is no rotation.
    client.set_token(Some(Token::new("fresh")));
    assert!(!client.rotates_tokens());
    client.api().await.unwrap().me().await.unwrap();
    assert_eq!(client.token(), Some(Token::new("rotated")));
    assert!(client.rotates_tokens());
}

#[tokio::test]
async fn unauthorized_replies_are_returned_without_credentials() {
    let server = Server::start(accounts, |_| true).await;
    let client = Client::builder(server.url())
        .token(Token::new("stale"))
        .build()
        .unwrap();
    let error = client.api().await.unwrap().me().await.unwrap_err();
    assert!(
        matches!(error, Error::Api(ApiError::Unauthorized)),
        "{error:?}"
    );
    // Discovery, then the refused `auth/me`.
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn socket_auth_failure_signs_in_and_authenticates_again() {
    let mut server = Server::start(accounts, |_| true).await;
    let client = Client::builder(server.url())
        .token(Token::new("fresh"))
        .credentials("player@example.com", "secret")
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    let mut websocket = server.socket().await;

    let (events, lines) = tokio::join!(
        descriptions_until(&mut socket, |event| matches!(
            event,
            SocketEvent::Event(Event::Authenticated)
        )),
        async {
            let mut lines = server::receive(&mut websocket, 2).await;
            server::send(&mut websocket, "auth failed").await;
            lines.extend(server::receive(&mut websocket, 1).await);
            server::send(&mut websocket, "auth ok").await;
            lines
        }
    );
    assert_eq!(events, ["connected", "Authenticated"]);
    assert_eq!(lines, ["gzip on", "auth fresh", "auth renewed"]);
    assert_eq!(client.token(), Some(Token::new("renewed")));
}

#[tokio::test]
async fn socket_auth_failure_is_returned_without_credentials() {
    let mut server = Server::start(accounts, |_| true).await;
    let client = Client::builder(server.url())
        .token(Token::new("fresh"))
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    let mut websocket = server.socket().await;

    let (events, ()) = tokio::join!(
        descriptions_until(&mut socket, |event| matches!(
            event,
            SocketEvent::Event(Event::AuthFailed)
        )),
        async {
            server::receive(&mut websocket, 2).await;
            server::send(&mut websocket, "auth failed").await;
        }
    );
    assert_eq!(events, ["connected", "AuthFailed"]);
}
