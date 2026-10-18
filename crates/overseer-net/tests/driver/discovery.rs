//! Discovery: the server's kind (official by address, `OpenScreeps` by its feature, community
//! otherwise) and the community plugins, the subsystems they make up and the operations reached
//! through them, and forcing the kind and the plugins.

use overseer_core::Token;
use overseer_core::model::platform::{Plugins, ServerKind, Subsystem, Subsystems};
use overseer_net::api::{AccountApi as _, PasswordSignIn as _, RateLimitLift, WorldApi as _};
use overseer_net::{Api, Client, Error};

use crate::server::{Request, Server};
use crate::{ME, VERSION};

/// `GET /api/version` of screeps.com: shards, its features and no plugins.
const OFFICIAL_VERSION: &str = r#"{"ok":1,"package":247,"protocol":14,"users":1802,"serverData":{"customObjectTypes":{},"historyChunkSize":100,"renderer":{"resources":{},"metadata":{}},"features":[{"name":"auth","version":1},{"name":"market","version":1}],"shards":["shard0","shard1","shard2","shard3","shardX"]}}"#;

/// `GET /api/version` of the stock Screeps server: no features.
const STOCK_VERSION: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"socketUpdateThrottle":100,"renderer":{"resources":{},"metadata":{}}},"packageVersion":"4.2.21"}"#;

/// `GET /api/version` of the Screeps server with screepsmod-auth and screepsmod-admin-utils
/// (an unset `shardName`).
const MODDED_VERSION: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,"serverData":{"shards":[null],"customObjectTypes":{},"historyChunkSize":20,"socketUpdateThrottle":1000,"renderer":{"resources":{},"metadata":{}},"features":[{"name":"auth","version":1},{"name":"auth-tokens"},{"name":"screepsmod-auth","version":"2.10.1"},{"name":"screepsmod-admin-utils","version":"1.37.1"}]}}"#;

/// `GET /api/authmod` of screepsmod-auth.
const AUTHMOD: &str = r#"{"ok":1,"name":"screepsmod-auth","version":"2.10.1","allowRegistration":true,"steam":true,"github":false,"gitlab":false}"#;

/// An API token long enough to lift its rate limits with (more than 8 characters).
const API_TOKEN: &str = "0123abcd-ffff-4444";

/// A server answering `GET /api/version` with `version`, `GET /api/authmod` with `authmod`
/// (HTTP 404 without), `GET /room-history` as the stock server does, signing in to token `t`
/// and answering `GET /api/auth/me` and `POST /api/user/noratelimit` with it.
fn routes(
    version: &'static str,
    authmod: Option<&'static str>,
) -> impl Fn(&Request) -> (u16, String) {
    move |request| match (request.path.as_str(), request.token.as_deref()) {
        ("/api/version", _) => (200, version.to_owned()),
        ("/api/authmod", _) => match authmod {
            Some(reply) => (200, reply.to_owned()),
            None => (404, "Not Found".to_owned()),
        },
        // backend-local's default history provider (`lib/game/server.js`).
        ("/room-history", _) => (500, "not implemented".to_owned()),
        ("/api/auth/signin", _) => (200, r#"{"ok":1,"token":"t"}"#.to_owned()),
        ("/api/auth/me", Some("t")) => (200, ME.to_owned()),
        ("/api/user/noratelimit", Some("t")) => (200, r#"{"ok":1}"#.to_owned()),
        _ => (401, r#"{"error":"unauthorized"}"#.to_owned()),
    }
}

/// The paths requested so far.
fn paths(server: &Server) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .map(|request| request.path)
        .collect()
}

/// The subsystems of `kind` with `plugins`.
fn subsystems(kind: ServerKind, plugins: Plugins) -> Subsystems {
    Subsystems::of(kind, plugins)
}

/// Every subsystem accessor of `api` answers exactly when the server offers its subsystem,
/// and the area operations of absent subsystems fail without a request.
async fn assert_accessors_follow_subsystems(api: &Api, server: &Server) {
    let token = Token::new(API_TOKEN);
    assert_eq!(
        api.password_sign_in().is_some(),
        api.supports(Subsystem::PasswordSignIn)
    );
    assert_eq!(
        api.password_account().is_some(),
        api.supports(Subsystem::PasswordAccounts)
    );
    assert_eq!(
        api.api_tokens().is_some(),
        api.supports(Subsystem::ApiTokens)
    );
    assert_eq!(
        api.rate_limit_lift(&token).is_some(),
        api.supports(Subsystem::RateLimitLift)
    );
    assert_eq!(
        api.account_website().is_some(),
        api.supports(Subsystem::AccountWebsite)
    );
    assert_eq!(
        api.room_objects_source().is_some(),
        api.supports(Subsystem::RoomObjects)
    );
    // The token prefix must not be the whole token.
    assert!(api.rate_limit_lift(&Token::new("12345678")).is_none());
    let before = server.requests().len();
    let unsupported = |error: Error, subsystem: Subsystem| {
        assert!(
            matches!(error, Error::Unsupported(absent) if absent == subsystem),
            "{error:?}"
        );
    };
    if !api.supports(Subsystem::RoomDecorations) {
        let error = api.room_decorations(None, "W1N1").await.unwrap_err();
        unsupported(error, Subsystem::RoomDecorations);
    }
    if !api.supports(Subsystem::RoomHistory) {
        let error = api.room_history(None, "W1N1", 47).await.unwrap_err();
        unsupported(error, Subsystem::RoomHistory);
    }
    if !api.supports(Subsystem::ObjectSearch) {
        unsupported(
            api.find_object("o1").await.unwrap_err(),
            Subsystem::ObjectSearch,
        );
    }
    if !api.supports(Subsystem::NoviceRooms) {
        unsupported(
            api.random_novice_room(None).await.unwrap_err(),
            Subsystem::NoviceRooms,
        );
    }
    if !api.supports(Subsystem::UsernameLookup) {
        unsupported(api.name().await.unwrap_err(), Subsystem::UsernameLookup);
    }
    if !api.supports(Subsystem::CpuShards) {
        let cpu = [("shard0".to_owned(), 20)].into_iter().collect();
        unsupported(
            api.set_cpu_shards(cpu).await.unwrap_err(),
            Subsystem::CpuShards,
        );
    }
    if !api.supports(Subsystem::ShardsInfo) {
        unsupported(api.shards_info().await.unwrap_err(), Subsystem::ShardsInfo);
    }
    assert_eq!(server.requests().len(), before, "unsupported sent nothing");
}

#[tokio::test]
async fn official_servers_are_known_by_address_also_behind_a_proxy() {
    let server = Server::start(routes(OFFICIAL_VERSION, Some(AUTHMOD)), |_| true).await;
    let client = Client::builder(server.official_url())
        .token(Token::new("t"))
        .build()
        .unwrap();
    let (world, me) = client.discover().await.unwrap();
    assert_eq!(world.connection().kind, ServerKind::Official);
    assert_eq!(world.connection().plugins, Plugins::NONE);
    assert!(!world.connection().forced);
    assert_eq!(
        world.subsystems(),
        subsystems(ServerKind::Official, Plugins::NONE)
    );
    assert!(world.supports(Subsystem::AccountWebsite));
    assert!(!world.supports(Subsystem::PasswordAccounts));
    assert_eq!(
        world.shards(),
        ["shard0", "shard1", "shard2", "shard3", "shardX"]
    );
    assert_eq!(world.shard(), Some("shard0"));
    assert_eq!(world.history_chunk_size(), 100);
    assert_eq!(world.version().package, Some(247));
    assert_eq!(world.version().features, ["auth", "market"]);
    assert_eq!(world.user(), Some("u1"));
    assert_eq!(me.unwrap().id, "u1");
    // No plugin probe; screeps.com registers and changes passwords on its website only.
    assert_eq!(paths(&server), ["/api/version", "/api/auth/me"]);
    let api = client.api().await.unwrap();
    assert_eq!(api.subsystems(), world.subsystems());
    assert_accessors_follow_subsystems(&api, &server).await;
    assert!(!api.password_sign_in().unwrap().accepts_username());
    let website = api.account_website().unwrap();
    assert_eq!(website.register, "https://screeps.com/a/#!/register");
    // screeps.com lifts rate limits on its reset page, with the token's prefix only.
    let Some(RateLimitLift::Page(page)) = api.rate_limit_lift(&Token::new(API_TOKEN)) else {
        panic!("a reset page");
    };
    assert_eq!(
        page.url,
        "https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=0123abcd"
    );
    assert!(page.is_success("https://screeps.com", Some("screepsTokenSuccess")));
    assert!(!page.is_success("https://example.com", Some("screepsTokenSuccess")));
    assert_eq!(api.me().await.unwrap().id, "u1");
}

#[tokio::test]
async fn official_servers_sign_in_with_email_and_password() {
    let server = Server::start(routes(OFFICIAL_VERSION, None), |_| true).await;
    let client = Client::builder(server.official_url())
        .credentials("player@example.com", "secret")
        .build()
        .unwrap();
    let (world, me) = client.discover().await.unwrap();
    assert!(world.supports(Subsystem::PasswordSignIn));
    assert_eq!(me.unwrap().id, "u1");
    assert_eq!(
        paths(&server),
        ["/api/version", "/api/auth/signin", "/api/auth/me"]
    );
    let sign_in = &server.requests()[1].body;
    assert!(
        sign_in.contains(r#""email":"player@example.com""#) && sign_in.contains("secret"),
        "{sign_in}"
    );
    assert_eq!(client.token(), Some(Token::new("t")));
}

#[tokio::test]
async fn openscreeps_servers_list_their_feature() {
    let server = Server::start(routes(VERSION, Some(AUTHMOD)), |_| true).await;
    let client = Client::builder(server.url())
        .credentials("player@example.com", "secret")
        .build()
        .unwrap();
    let (world, me) = client.discover().await.unwrap();
    assert_eq!(world.connection().kind, ServerKind::OpenScreeps);
    assert_eq!(world.connection().plugins, Plugins::NONE);
    assert_eq!(
        world.subsystems(),
        subsystems(ServerKind::OpenScreeps, Plugins::NONE)
    );
    assert!(!world.is_sharded());
    assert_eq!(world.version().features, ["market", "auth", "openscreeps"]);
    assert_eq!(me.unwrap().id, "u1");
    // Signed in natively; no plugin probe.
    assert_eq!(
        paths(&server),
        ["/api/version", "/api/auth/signin", "/api/auth/me"]
    );
    let api = client.api().await.unwrap();
    assert_accessors_follow_subsystems(&api, &server).await;
    assert!(api.api_tokens().is_none());
    assert!(api.password_account().is_some());
    let sign_in = api.password_sign_in().unwrap();
    assert!(sign_in.accepts_username());
    assert_eq!(
        sign_in
            .sign_in("player@example.com", "secret")
            .await
            .unwrap(),
        Token::new("t")
    );
}

#[tokio::test]
async fn community_servers_without_plugins_offer_no_subsystem() {
    let server = Server::start(routes(STOCK_VERSION, None), |_| true).await;
    let anonymous = Client::builder(server.url()).build().unwrap();
    let world = anonymous.world().await.unwrap();
    assert_eq!(world.connection().kind, ServerKind::Community);
    assert_eq!(world.connection().plugins, Plugins::NONE);
    assert_eq!(world.subsystems(), Subsystems::NONE);
    assert_eq!(world.version().package_version.as_deref(), Some("4.2.21"));
    assert_eq!(world.version().features, [] as [String; 0]);
    assert_eq!(
        paths(&server),
        ["/api/version", "/api/authmod", "/room-history"]
    );
    let api = anonymous.api().await.unwrap();
    assert_accessors_follow_subsystems(&api, &server).await;
    assert!(api.password_sign_in().is_none());

    // Credentials need password sign-in, which the server lacks.
    let client = Client::builder(server.url())
        .credentials("player@example.com", "secret")
        .build()
        .unwrap();
    let error = client.world().await.unwrap_err();
    assert!(
        matches!(error, Error::Unsupported(Subsystem::PasswordSignIn)),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "the server does not offer password sign-in"
    );
}

#[tokio::test]
async fn community_servers_with_screepsmod_auth_sign_in_and_lift_rate_limits() {
    let server = Server::start(routes(STOCK_VERSION, Some(AUTHMOD)), |_| true).await;
    let client = Client::builder(server.url())
        .credentials("player", "secret")
        .build()
        .unwrap();
    let world = client.world().await.unwrap();
    let auth = Plugins {
        screepsmod_auth: true,
        ..Plugins::NONE
    };
    assert_eq!(world.connection().plugins, auth);
    assert_eq!(world.subsystems(), subsystems(ServerKind::Community, auth));
    assert!(world.supports(Subsystem::RateLimitLift));
    assert!(!world.supports(Subsystem::RoomObjects));
    let api = client.api().await.unwrap();
    assert_accessors_follow_subsystems(&api, &server).await;
    assert!(api.password_sign_in().unwrap().accepts_username());
    // screepsmod-auth lifts rate limits with a request carrying the token's prefix.
    let Some(RateLimitLift::Request(lift)) = api.rate_limit_lift(&Token::new(API_TOKEN)) else {
        panic!("a lift request");
    };
    lift.send().await.unwrap();
    let request = server.requests().pop().unwrap();
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("POST", "/api/user/noratelimit")
    );
    assert_eq!(request.body, r#"{"token":"0123abcd"}"#);
}

#[tokio::test]
async fn community_servers_with_screepsmod_admin_utils_have_shards_info_and_room_objects_only() {
    let server = Server::start(routes(MODDED_VERSION, None), |_| true).await;
    let anonymous = Client::builder(server.url()).build().unwrap();
    let world = anonymous.world().await.unwrap();
    let admin_utils = Plugins {
        screepsmod_admin_utils: true,
        ..Plugins::NONE
    };
    assert_eq!(world.connection().plugins, admin_utils);
    assert_eq!(
        world.subsystems().iter().collect::<Vec<_>>(),
        [Subsystem::ShardsInfo, Subsystem::RoomObjects]
    );
    let api = anonymous.api().await.unwrap();
    assert_accessors_follow_subsystems(&api, &server).await;
}

#[tokio::test]
async fn community_servers_with_a_history_mod_serve_room_history() {
    // screepsmod-history: a chunk it holds, else its read error (here the file adapter's),
    // which backend-local sends as JSON with HTTP 500.
    let stock = routes(STOCK_VERSION, None);
    let server = Server::start(
        move |request: &Request| match (request.path.as_str(), request.query.as_str()) {
            ("/room-history", "room=W1N1&time=40") => (
                200,
                r#"{"base":40,"timestamp":1760000000000,"ticks":{"40":{"a":{"_id":"a","type":"source","x":10,"y":10,"room":"W1N1"}}}}"#.to_owned(),
            ),
            ("/room-history", _) => (
                500,
                r#"{"errno":-2,"code":"ENOENT","syscall":"open"}"#.to_owned(),
            ),
            _ => stock(request),
        },
        |_| true,
    )
    .await;
    let client = Client::builder(server.url()).build().unwrap();
    let world = client.world().await.unwrap();
    let history = Plugins {
        room_history: true,
        ..Plugins::NONE
    };
    assert_eq!(world.connection().plugins, history);
    assert_eq!(
        world.connection().plugins.names().collect::<Vec<_>>(),
        ["room-history mod"]
    );
    assert_eq!(
        world.subsystems().iter().collect::<Vec<_>>(),
        [Subsystem::RoomHistory]
    );
    let api = client.api().await.unwrap();
    assert_accessors_follow_subsystems(&api, &server).await;
    // 20-tick chunks: tick 47 is in the chunk from 40.
    let chunk = api.room_history(None, "W1N1", 47).await.unwrap();
    assert_eq!(chunk.ticks().collect::<Vec<_>>(), [40]);
    let missing = api.room_history(None, "W1N1", 0).await.unwrap_err();
    assert!(
        matches!(
            &missing,
            Error::Api(overseer_core::api::ApiError::Status { status, .. }) if status.as_u16() == 500
        ),
        "{missing:?}"
    );
}

#[tokio::test]
async fn community_servers_with_both_plugins_sign_in_through_screepsmod_auth() {
    let server = Server::start(routes(MODDED_VERSION, Some(AUTHMOD)), |_| true).await;
    let client = Client::builder(server.url())
        .credentials("player@example.com", "secret")
        .build()
        .unwrap();
    let (world, me) = client.discover().await.unwrap();
    let both = Plugins {
        screepsmod_auth: true,
        screepsmod_admin_utils: true,
        room_history: false,
    };
    assert_eq!(world.connection().kind, ServerKind::Community);
    assert_eq!(world.connection().plugins, both);
    assert_eq!(world.subsystems(), subsystems(ServerKind::Community, both));
    // Its `shardName` is no shard.
    assert!(!world.is_sharded());
    assert_eq!(world.version().socket_update_throttle, Some(1000.0));
    assert_eq!(
        world.version().features,
        [
            "auth",
            "auth-tokens",
            "screepsmod-auth",
            "screepsmod-admin-utils"
        ]
    );
    assert_eq!(me.unwrap().id, "u1");
    assert_eq!(
        paths(&server),
        [
            "/api/version",
            "/api/authmod",
            "/room-history",
            "/api/auth/signin",
            "/api/auth/me"
        ]
    );
    let sign_in = &server.requests()[3].body;
    assert!(
        sign_in.contains(r#""email":"player@example.com""#),
        "{sign_in}"
    );
    let api = client.api().await.unwrap();
    assert_accessors_follow_subsystems(&api, &server).await;
    assert!(api.password_sign_in().is_some());
    assert!(api.password_account().is_some());
    assert!(api.api_tokens().is_some());
}

#[tokio::test]
async fn a_forced_kind_skips_telling_it_and_still_reads_the_version() {
    // An `OpenScreeps` server used as a community one: its plugins are still detected.
    let server = Server::start(routes(VERSION, Some(AUTHMOD)), |_| true).await;
    let client = Client::builder(server.url())
        .server_kind(ServerKind::Community)
        .build()
        .unwrap();
    let world = client.world().await.unwrap();
    assert_eq!(world.connection().kind, ServerKind::Community);
    assert!(world.connection().forced);
    assert_eq!(
        world.connection().plugins,
        Plugins {
            screepsmod_auth: true,
            ..Plugins::NONE
        }
    );
    assert_eq!(world.version().features, ["market", "auth", "openscreeps"]);
    assert_eq!(
        paths(&server),
        ["/api/version", "/api/authmod", "/room-history"]
    );
    let api = client.api().await.unwrap();
    assert_eq!(api.subsystems(), world.subsystems());
    assert!(!api.supports(Subsystem::RoomHistory));
    // A private address forced to a private contract: no limit until the server reports one.
    assert!(!client.has_rate_limits());

    // A private address forced official: read with the official contract, no plugin probe.
    let server = Server::start(routes(STOCK_VERSION, Some(AUTHMOD)), |_| true).await;
    let client = Client::builder(server.url())
        .server_kind(ServerKind::Official)
        .plugins(Plugins {
            screepsmod_auth: true,
            screepsmod_admin_utils: true,
            room_history: true,
        })
        .build()
        .unwrap();
    let world = client.world().await.unwrap();
    assert_eq!(world.connection().kind, ServerKind::Official);
    // Plugins only extend community servers.
    assert_eq!(world.connection().plugins, Plugins::NONE);
    assert_eq!(
        world.subsystems(),
        subsystems(ServerKind::Official, Plugins::NONE)
    );
    assert_eq!(world.version().package_version.as_deref(), Some("4.2.21"));
    assert!(!world.is_sharded());
    assert_eq!(paths(&server), ["/api/version"]);
    assert!(client.api().await.unwrap().account_website().is_some());
    // Only screeps.com speaks the official contract: assume its server-wide limit.
    assert!(client.has_rate_limits());
    // A fully forced kind needs no request for its operations, even before discovery.
    let fresh = Client::builder(server.url())
        .server_kind(ServerKind::Official)
        .build()
        .unwrap();
    assert!(
        fresh
            .api()
            .await
            .unwrap()
            .supports(Subsystem::AccountWebsite)
    );
    assert_eq!(paths(&server), ["/api/version"]);

    // The official address forced to `OpenScreeps`.
    let server = Server::start(routes(VERSION, None), |_| true).await;
    let client = Client::builder(server.official_url())
        .server_kind(ServerKind::OpenScreeps)
        .build()
        .unwrap();
    let world = client.world().await.unwrap();
    assert_eq!(world.connection().kind, ServerKind::OpenScreeps);
    assert!(world.connection().forced);
    assert!(world.supports(Subsystem::PasswordAccounts));
    // The address is screeps.com, so the limit stays.
    assert!(client.has_rate_limits());
}

#[tokio::test]
async fn forced_plugins_replace_their_detection() {
    // screepsmod-auth answering: forced away, so not even asked.
    let server = Server::start(routes(STOCK_VERSION, Some(AUTHMOD)), |_| true).await;
    let client = Client::builder(server.url())
        .plugins(Plugins::NONE)
        .build()
        .unwrap();
    let world = client.world().await.unwrap();
    assert_eq!(world.connection().kind, ServerKind::Community);
    assert!(!world.connection().forced, "the kind was discovered");
    assert_eq!(world.connection().plugins, Plugins::NONE);
    assert_eq!(world.subsystems(), Subsystems::NONE);
    assert_eq!(paths(&server), ["/api/version"]);

    // Plugins forced away read the version with the community contract, which cannot read
    // screepsmod-admin-utils' feature versions.
    let server = Server::start(routes(MODDED_VERSION, Some(AUTHMOD)), |_| true).await;
    let client = Client::builder(server.url())
        .plugins(Plugins::NONE)
        .build()
        .unwrap();
    let error = client.world().await.unwrap_err();
    assert!(
        matches!(error, Error::Api(overseer_core::api::ApiError::Decode(_))),
        "{error:?}"
    );

    // screepsmod-auth forced on a stock server: no probe, its operations are used.
    let server = Server::start(routes(STOCK_VERSION, None), |_| true).await;
    let auth = Plugins {
        screepsmod_auth: true,
        ..Plugins::NONE
    };
    let client = Client::builder(server.url())
        .server_kind(ServerKind::Community)
        .plugins(auth)
        .credentials("player", "secret")
        .build()
        .unwrap();
    let (world, me) = client.discover().await.unwrap();
    assert_eq!(world.connection().plugins, auth);
    assert!(world.supports(Subsystem::PasswordSignIn));
    assert_eq!(me.unwrap().id, "u1");
    assert_eq!(
        paths(&server),
        ["/api/version", "/api/auth/signin", "/api/auth/me"]
    );
}
