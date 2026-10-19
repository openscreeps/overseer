//! The session's subsystems and connection info, discovered or forced through the profile, the
//! operations of absent subsystems failing without a request, and the cached shard information.

use std::sync::atomic::{AtomicUsize, Ordering};

use overseer::core::{ServerUrl, Token};
use overseer::model::platform::{Plugins, ServerKind, Subsystem, Subsystems};
use overseer::net::api::RateLimitLift;
use overseer::{Auth, Error, Overseer, ServerProfile, net};

use crate::server::{Request, Server};
use crate::{receive, routes};

/// A token long enough for lifting its rate limits (more than 8 characters).
const LONG_TOKEN: &str = "0123abcd-long-token";

/// Connects with `token` to `url`, forcing `server_kind` and `plugins`, and completes the
/// socket handshake.
async fn connect_forced(
    server: &mut Server,
    url: ServerUrl,
    token: &str,
    server_kind: Option<ServerKind>,
    plugins: Option<Plugins>,
) -> Overseer {
    let profile = ServerProfile {
        url,
        auth: Auth::Token(Token::new(token)),
        server_password: None,
        default_shard: None,
        server_kind,
        plugins,
    };
    let overseer = Overseer::connect(profile).await.unwrap();
    let mut websocket = server.socket().await;
    assert_eq!(
        receive(&mut websocket, 2).await,
        ["gzip on".to_owned(), format!("auth {token}")]
    );
    crate::server::send(&mut websocket, "auth ok").await;
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe server-message"]
    );
    overseer
}

/// Fails unless `result` is [`net::Error::Unsupported`] of `subsystem`.
fn assert_unsupported<T: std::fmt::Debug>(result: Result<T, Error>, subsystem: Subsystem) {
    match result {
        Err(Error::Net(net::Error::Unsupported(absent))) => assert_eq!(absent, subsystem),
        other => panic!("{subsystem} should be unsupported: {other:?}"),
    }
}

#[tokio::test]
async fn a_discovered_session_reports_its_subsystems_and_connection() {
    let mut server = Server::start(routes, |_| true).await;
    let url = server.url();
    let overseer = connect_forced(&mut server, url, "t1", None, None).await;
    let world = overseer.world();
    assert_eq!(world.connection().kind, ServerKind::OpenScreeps);
    assert_eq!(world.connection().plugins, Plugins::NONE);
    assert!(!world.connection().forced);
    assert_eq!(world.connection().server.history_chunk_size, 20);
    assert_eq!(
        world.subsystems(),
        Subsystems::of(ServerKind::OpenScreeps, Plugins::NONE)
    );
    assert_eq!(overseer.api().subsystems(), world.subsystems());
    // Absent subsystems fail at once.
    let requested = server.requests().len();
    let account = overseer.account();
    assert_unsupported(
        account.rate_limit_exemption().await,
        Subsystem::RateLimitLift,
    );
    assert_unsupported(account.rate_limit_lift(), Subsystem::RateLimitLift);
    let cpu = [("shard0".to_owned(), 20)].into_iter().collect();
    assert_unsupported(account.set_cpu_shards(cpu).await, Subsystem::CpuShards);
    assert_unsupported(overseer.shards_info().await, Subsystem::ShardsInfo);
    assert_eq!(server.requests().len(), requested, "nothing was sent");
    overseer.shutdown().await;
}

#[tokio::test]
async fn a_profile_forcing_the_kind_and_plugins_skips_their_detection() {
    let mut server = Server::start(
        |request: &Request| match request.path.as_str() {
            "/api/user/noratelimit" if request.token.is_some() => (200, r#"{"ok":1}"#.to_owned()),
            _ => routes(request),
        },
        |_| true,
    )
    .await;
    // The `OpenScreeps` server used as a community server with screepsmod-auth.
    let auth = Plugins {
        screepsmod_auth: true,
        ..Plugins::NONE
    };
    let url = server.url();
    let overseer = connect_forced(
        &mut server,
        url,
        LONG_TOKEN,
        Some(ServerKind::Community),
        Some(auth),
    )
    .await;
    let world = overseer.world();
    assert_eq!(world.connection().kind, ServerKind::Community);
    assert_eq!(world.connection().plugins, auth);
    assert!(world.connection().forced);
    // `GET /api/version` is read all the same; no plugin probe.
    assert_eq!(world.version().features, ["market", "auth", "openscreeps"]);
    let paths: Vec<_> = server
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect();
    assert_eq!(paths, ["/api/version", "/api/auth/me"]);
    assert_eq!(
        world.subsystems(),
        Subsystems::of(ServerKind::Community, auth)
    );
    assert!(world.supports(Subsystem::RateLimitLift));

    // The forced community contract has no decorations, room objects or history: refused
    // without a request.
    let requested = server.requests().len();
    let rooms = overseer.rooms();
    assert_unsupported(
        rooms.decorations("W1N1", None).await,
        Subsystem::RoomDecorations,
    );
    assert_unsupported(rooms.objects("W1N1", None).await, Subsystem::RoomObjects);
    let mut history = overseer.history().player("W1N1", None).unwrap();
    assert_unsupported(history.seek(40).await, Subsystem::RoomHistory);
    assert_eq!(server.requests().len(), requested, "nothing was sent");

    // screepsmod-auth lifts the token's rate limits with a request.
    let Ok(RateLimitLift::Request(lift)) = overseer.account().rate_limit_lift() else {
        panic!("a lift request");
    };
    lift.send().await.unwrap();
    let request = server.requests().pop().unwrap();
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("POST", "/api/user/noratelimit")
    );
    assert_eq!(request.body, r#"{"token":"0123abcd"}"#);
    overseer.shutdown().await;
}

#[tokio::test]
async fn a_profile_forcing_official_lifts_rate_limits_on_a_page() {
    let mut server = Server::start(routes, |_| true).await;
    let url = server.url();
    let overseer = connect_forced(
        &mut server,
        url,
        LONG_TOKEN,
        Some(ServerKind::Official),
        None,
    )
    .await;
    let world = overseer.world();
    assert_eq!(world.connection().kind, ServerKind::Official);
    assert!(world.connection().forced);
    // The version lists no shard: a single world all the same.
    assert!(!world.is_sharded());
    assert!(world.supports(Subsystem::AccountWebsite));
    assert!(!world.supports(Subsystem::PasswordAccounts));
    let Ok(RateLimitLift::Page(page)) = overseer.account().rate_limit_lift() else {
        panic!("a reset page");
    };
    assert!(
        page.url.ends_with("noratelimit?token=0123abcd"),
        "{}",
        page.url
    );
    overseer.shutdown().await;
}

/// screeps.com's shards: shard3 caps the CPU at 20, shard0 reports no cap (0).
const SHARDS_INFO: &str = r#"{"ok":1,"shards":[
    {"name":"shard0","lastTicks":[4100],"cpuLimit":0,"rooms":30000,"users":1200,"tick":4000},
    {"name":"shard3","lastTicks":[3100],"cpuLimit":20,"rooms":4000,"users":900,"tick":3100}]}"#;

#[tokio::test]
async fn the_shard_information_is_fetched_once_per_session_after_a_success() {
    // The first request fails; every later one answers the shards.
    let asked = AtomicUsize::new(0);
    let mut server = Server::start(
        move |request: &Request| match request.path.as_str() {
            "/api/game/shards/info" => {
                if asked.fetch_add(1, Ordering::SeqCst) == 0 {
                    (200, r#"{"error":"busy"}"#.to_owned())
                } else {
                    (200, SHARDS_INFO.to_owned())
                }
            }
            _ => routes(request),
        },
        |_| true,
    )
    .await;
    let url = server.url();
    let overseer = connect_forced(&mut server, url, "t1", Some(ServerKind::Official), None).await;
    assert!(overseer.world().supports(Subsystem::ShardsInfo));
    let sent = |server: &Server| {
        server
            .requests()
            .into_iter()
            .filter(|request| request.path == "/api/game/shards/info")
            .count()
    };
    // A failure is not cached: the next call asks again.
    assert!(overseer.shards_info().await.is_err());
    assert_eq!(sent(&server), 1);
    let shards = overseer.shards_info().await.unwrap();
    assert_eq!(sent(&server), 2);
    let limits: Vec<_> = shards
        .iter()
        .map(|shard| (shard.name.as_str(), shard.cpu_limit))
        .collect();
    assert_eq!(limits, [("shard0", None), ("shard3", Some(20.0))]);
    // Then the cache answers, also through a clone of the session.
    let again = overseer.clone().shards_info().await.unwrap();
    assert!(std::sync::Arc::ptr_eq(&shards, &again));
    assert_eq!(sent(&server), 2, "the cache answered");
    overseer.shutdown().await;
}
