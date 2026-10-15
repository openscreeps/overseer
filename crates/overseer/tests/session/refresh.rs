//! Rotating session tokens kept alive while idle, on paused time.
//!
//! Time is paused once the session is connected, and every wait runs the runtime by yielding:
//! a runtime that never idles never advances paused time on its own, so only
//! `tokio::time::advance` moves the clock.

use std::time::Duration;

use futures_util::StreamExt;
use overseer::core::Token;
use overseer::core::api::endpoints;
use overseer::{Auth, ConnectionStatus, Notice, Overseer, ServerProfile};

use crate::server::{self, Request, Response, Server, Ws};
use crate::{ME, PATIENCE, VERSION, connect, receive, routes};

/// A single-world server with user `u1` that accepts every token and, like private servers,
/// replaces token `t<n>` with `t<n+1>` on every authenticated reply of `GET /api/auth/me` and
/// `GET /api/user/world-status`.
fn rotating(request: &Request) -> Response {
    let body = match request.path.as_str() {
        "/api/version" => return (200, VERSION.to_owned()).into(),
        "/api/auth/me" => ME,
        "/api/user/world-status" => r#"{"ok":1,"status":"normal"}"#,
        _ => return routes(request).into(),
    };
    let Some(next) = request.token.as_deref().map(next_token) else {
        return (401, r#"{"error":"unauthorized"}"#.to_owned()).into();
    };
    Response {
        status: 200,
        headers: vec![("X-Token", next)],
        body: body.as_bytes().to_vec(),
    }
}

/// `t<n+1>` for `t<n>`.
fn next_token(token: &str) -> String {
    let number: u32 = token.trim_start_matches('t').parse().unwrap();
    format!("t{}", number + 1)
}

/// Connects user `u1` with token `t1` to a [`rotating`] server (discovery rotates it to `t2`),
/// completes the socket handshake, keeps answering keep-alive pings and pauses time.
async fn connect_rotating(server: &mut Server) -> Overseer {
    let profile = ServerProfile {
        url: server.url(),
        auth: Auth::Token(Token::new("t1")),
        server_password: None,
        default_shard: None,
    };
    let overseer = Overseer::connect(profile).await.unwrap();
    let mut websocket = server.socket().await;
    assert_eq!(receive(&mut websocket, 2).await, ["gzip on", "auth t2"]);
    server::send(&mut websocket, "auth ok t2").await;
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe server-message"]
    );
    answer_pings(websocket);
    tokio::time::pause();
    overseer
}

/// Reads `websocket` in the background, so keep-alive pings are answered.
fn answer_pings(mut websocket: Ws) {
    tokio::spawn(async move { while websocket.next().await.is_some() {} });
}

/// The tokens of the `GET /api/auth/me` requests served so far (`-` for none).
fn auth_me(server: &Server) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/auth/me")
        .map(|request| request.token.unwrap_or_else(|| "-".to_owned()))
        .collect()
}

/// The session's current token.
fn token(overseer: &Overseer) -> Option<Token> {
    overseer.tokens().borrow().clone()
}

/// Runs the other tasks, without moving the clock, until `done` holds.
async fn settle(mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() {
        assert!(start.elapsed() < PATIENCE, "settled in time");
        tokio::task::yield_now().await;
    }
}

/// Lets the other tasks react to what just happened (and answer any request started) without
/// moving the clock.
async fn run() {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(200) {
        tokio::task::yield_now().await;
    }
}

/// Moves the clock by `duration` once the other tasks have reacted to what happened before,
/// then lets them react to the new time.
async fn advance(duration: Duration) {
    run().await;
    tokio::time::advance(duration).await;
    run().await;
}

/// Advances the clock by 26 s `times` times.
async fn idle(times: usize) {
    for _ in 0..times {
        advance(Duration::from_secs(26)).await;
    }
}

#[tokio::test]
async fn rotated_tokens_are_refreshed_every_25_idle_seconds() {
    let mut server = Server::start(rotating, |_| true).await;
    let overseer = connect_rotating(&mut server).await;
    assert_eq!(auth_me(&server), ["t1"]);
    assert_eq!(token(&overseer), Some(Token::new("t2")));
    // Discovery issued `t2` just before the clock was paused.
    advance(Duration::from_secs(20)).await;
    assert_eq!(auth_me(&server), ["t1"]);
    advance(Duration::from_secs(6)).await;
    settle(|| auth_me(&server).len() == 2).await;
    assert_eq!(auth_me(&server), ["t1", "t2"]);
    settle(|| token(&overseer) == Some(Token::new("t3"))).await;
    for (sent, issued) in [("t3", "t4"), ("t4", "t5")] {
        let count = auth_me(&server).len();
        advance(Duration::from_secs(24)).await;
        assert_eq!(auth_me(&server).len(), count);
        advance(Duration::from_secs(2)).await;
        settle(|| auth_me(&server).len() == count + 1).await;
        assert_eq!(auth_me(&server).last().map(String::as_str), Some(sent));
        settle(|| token(&overseer) == Some(Token::new(issued))).await;
    }
    assert_eq!(*overseer.status().borrow(), ConnectionStatus::Connected);
}

#[tokio::test]
async fn requests_of_the_app_postpone_the_refresh() {
    let mut server = Server::start(rotating, |_| true).await;
    let overseer = connect_rotating(&mut server).await;
    advance(Duration::from_secs(20)).await;
    let client = overseer.client().clone();
    let request =
        tokio::spawn(async move { client.send(&endpoints::UserWorldStatus).await.map(|_| ()) });
    settle(|| request.is_finished()).await;
    request.await.unwrap().unwrap();
    assert_eq!(token(&overseer), Some(Token::new("t3")));
    // 40 s after connecting, 20 s after the app's request.
    advance(Duration::from_secs(20)).await;
    assert_eq!(auth_me(&server), ["t1"]);
    advance(Duration::from_secs(6)).await;
    settle(|| auth_me(&server).len() == 2).await;
    assert_eq!(auth_me(&server), ["t1", "t3"]);
    settle(|| token(&overseer) == Some(Token::new("t4"))).await;
}

#[tokio::test]
async fn tokens_that_never_rotate_get_no_refreshes() {
    let mut server = Server::start(routes, |_| true).await;
    // The socket's `auth ok` replaces the token, but replies never do.
    let (overseer, websocket) = connect(&mut server).await;
    answer_pings(websocket);
    tokio::time::pause();
    idle(4).await;
    assert_eq!(auth_me(&server), ["t1"]);
    assert!(!overseer.client().rotates_tokens());
    assert_eq!(token(&overseer), Some(Token::new("t2")));
}

#[tokio::test]
async fn refreshing_stops_after_shutdown() {
    let mut server = Server::start(rotating, |_| true).await;
    let overseer = connect_rotating(&mut server).await;
    let closing = overseer.clone();
    let shutdown = tokio::spawn(async move { closing.shutdown().await });
    settle(|| shutdown.is_finished()).await;
    shutdown.await.unwrap();
    idle(3).await;
    assert_eq!(auth_me(&server), ["t1"]);
    assert_eq!(*overseer.status().borrow(), ConnectionStatus::Closed);
}

#[tokio::test]
async fn refreshing_stops_when_the_last_handle_is_dropped() {
    let mut server = Server::start(rotating, |_| true).await;
    let overseer = connect_rotating(&mut server).await;
    let status = overseer.status();
    drop(overseer);
    settle(|| *status.borrow() == ConnectionStatus::Closed).await;
    idle(3).await;
    assert_eq!(auth_me(&server), ["t1"]);
}

#[tokio::test]
async fn a_rejected_refresh_is_a_notice_and_the_session_goes_on() {
    // Token `t2` has expired by the time of the first refresh.
    let mut server = Server::start(
        |request: &Request| {
            if request.path == "/api/auth/me" && request.token.as_deref() == Some("t2") {
                (401, r#"{"error":"unauthorized"}"#.to_owned()).into()
            } else {
                rotating(request)
            }
        },
        |_| true,
    )
    .await;
    let overseer = connect_rotating(&mut server).await;
    let mut notices = overseer.notices();
    idle(1).await;
    settle(|| auth_me(&server).len() == 2).await;
    settle(|| notices.try_recv() == Ok(Notice::AuthFailed)).await;
    // No more requests with the rejected token.
    idle(3).await;
    assert_eq!(auth_me(&server), ["t1", "t2"]);
    assert_eq!(*overseer.status().borrow(), ConnectionStatus::Connected);
    // A new token resumes refreshing.
    overseer.client().set_token(Some(Token::new("t7")));
    idle(1).await;
    settle(|| auth_me(&server).len() == 3).await;
    assert_eq!(auth_me(&server).last().map(String::as_str), Some("t7"));
    settle(|| token(&overseer) == Some(Token::new("t8"))).await;
}
