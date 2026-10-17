//! HTTP through the proxy: discovery, sign-in, token rotation, authenticated requests and room
//! history.

use std::time::Duration;

use overseer_net::api::{
    AccountApi as _, ConsoleApi as _, MemoryApi as _, PasswordSignIn as _, WorldApi as _,
};
use overseer_net::core::api::ApiError;
use overseer_net::model::account::Registration;
use overseer_net::model::memory::MemoryValue;
use overseer_net::model::platform::Subsystem;
use overseer_net::serde_json::json;
use overseer_net::{Client, Error};

use crate::support::{self, PATIENCE};

/// Polls `check` every tick until it returns a value, within [`PATIENCE`].
async fn eventually<T, F: Future<Output = Option<T>>>(mut check: impl FnMut() -> F) -> T {
    tokio::time::timeout(PATIENCE, async {
        loop {
            if let Some(value) = check().await {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the condition holds in time")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn discovery_registration_and_sign_in_pass_through() {
    let proxy = support::start(None, &[]).await;
    let server = support::via(&proxy);
    assert_eq!(proxy.upstream(), &support::upstream());

    let anonymous = support::anonymous(&server);
    let world = anonymous
        .world()
        .await
        .expect("the version through the proxy");
    let direct = support::anonymous(&support::direct())
        .world()
        .await
        .expect("the version");
    assert_eq!(world.shards(), direct.shards());
    assert_eq!(world.history_chunk_size(), direct.history_chunk_size());
    assert_eq!(world.version().protocol, direct.version().protocol);
    assert_eq!(world.user(), None);

    // Registration and its logical failures (HTTP 200 `{error}`) pass unchanged: the server's
    // text for a taken username, which a community server's screepsmod-auth 2.10.1 follows with
    // the error's stack trace (`lib/register.js`,
    // <https://unpkg.com/screepsmod-auth@2.10.1/lib/register.js>).
    let account = support::register_with(&server, None).await;
    let again = Registration {
        username: account.username.clone(),
        email: format!("other-{}", account.email),
        password: account.password.clone(),
        modules: None,
    };
    let result = support::submit(&server, again).await;
    let taken = |message: &str| {
        if support::community() {
            message.lines().next() == Some("Error: User already exists")
        } else {
            message == "username already exists"
        }
    };
    assert!(
        matches!(&result, Err(Error::Api(ApiError::Server(message))) if taken(message)),
        "{result:?}"
    );

    // Sign-in through the proxy names the same user as signing in directly.
    let proxied = account
        .client(&server)
        .world()
        .await
        .expect("sign-in through the proxy");
    let straight = account
        .client(&support::direct())
        .world()
        .await
        .expect("sign-in");
    assert!(proxied.user().is_some());
    assert_eq!(proxied.user(), straight.user());

    // HTTP 401 passes unchanged.
    let result = anonymous
        .api()
        .await
        .expect("the server's operations")
        .password_sign_in()
        .expect("password sign-in")
        .sign_in(&account.username, "not-the-password")
        .await;
    assert!(
        matches!(result, Err(Error::Api(ApiError::Unauthorized))),
        "{result:?}"
    );
    proxy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn tokens_rotate_through_the_proxy_and_authenticate_requests() {
    let proxy = support::start(None, &[]).await;
    let server = support::via(&proxy);
    // A player: console and Memory writes run in its runtime, which needs a spawn.
    let player = support::player(&server).await;
    let client = player.account.client(&server);
    let mut updates = client.token_updates();
    let user = client
        .world()
        .await
        .expect("sign-in")
        .user()
        .map(str::to_owned);
    // `X-Token` reaches the client through the proxy.
    assert!(client.rotates_tokens());
    let first = client.token().expect("a token");
    let api = client.api().await.expect("the server's API");
    api.me().await.expect("auth/me");
    let second = client.token().expect("a token");
    assert_ne!(first, second);
    assert!(updates.has_changed().expect("the client is alive"));
    assert_eq!(updates.borrow_and_update().as_ref(), Some(&second));

    // A token rotated through the proxy is valid upstream, and the other way round.
    let straight = Client::builder(support::direct())
        .token(second)
        .build()
        .expect("a client");
    let me = straight.world().await.expect("auth/me");
    assert_eq!(me.user().map(str::to_owned), user);
    let back = Client::builder(server.clone())
        .token(straight.token().expect("a token"))
        .build()
        .expect("a client");
    let me = back.world().await.expect("auth/me");
    assert_eq!(me.user().map(str::to_owned), user);

    // Authenticated writes and reads: Memory through the console.
    api.console("Memory.viaProxy = 'yes'", None)
        .await
        .expect("the expression is queued");
    eventually(|| {
        let api = api.clone();
        async move {
            let value = api.memory("viaProxy", None).await.expect("Memory decodes");
            (value == MemoryValue::Json(json!("yes"))).then_some(())
        }
    })
    .await;
    api.set_memory("viaProxy", json!({ "nested": [1, 2] }), None)
        .await
        .expect("the write is queued");
    eventually(|| {
        let api = api.clone();
        async move {
            let value = api
                .memory("viaProxy.nested", None)
                .await
                .expect("Memory decodes");
            (value == MemoryValue::Json(json!([1, 2]))).then_some(())
        }
    })
    .await;
    proxy.shutdown().await;
}

/// A history chunk through the proxy replays like the one fetched directly; a chunk that is not
/// kept is the server's HTTP 404.
///
/// The community server keeps no room history: `@screeps/backend` 3.3.0 answers every
/// `/room-history` request with HTTP 500 `not implemented` unless a mod replaces its
/// `onGetRoomHistory` (`lib/game/server.js`,
/// <https://github.com/screeps/backend-local/blob/9d079282303ec04e577ac2bb97f64312b25e4ccd/lib/game/server.js#L34-L36>),
/// and the world under test runs none: there that refusal passes through unchanged, so
/// Overseer tells through the proxy that the server has no room history.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn room_history_is_forwarded() {
    let proxy = support::start(None, &[]).await;
    let server = support::via(&proxy);
    let player = support::player(&server).await;
    let client = player.account.client(&server);
    let straight = player.account.client(&support::direct());
    let size = client.world().await.expect("sign-in").history_chunk_size();
    // The first chunk that starts after the spawn was placed, once it holds a few ticks (the
    // server serves the chunk being written).
    let placed = support::game_time(&client).await;
    let base = placed - placed % size + size;
    support::wait_for_tick(&client, base + 4).await;
    if support::community() {
        let query = format!("room-history?room={}&time={base}", player.room);
        let http = support::http();
        for url in [proxy.url().join(&query), support::upstream().join(&query)] {
            let reply = http
                .get(url.expect("a URL"))
                .send()
                .await
                .expect("an answer");
            assert_eq!(reply.status(), reqwest::StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(reply.text().await.expect("a body"), "not implemented");
        }
        let api = client.api().await.expect("the server's API");
        assert!(!api.supports(Subsystem::RoomHistory));
        let refused = api.room_history(None, &player.room, base).await;
        assert!(
            matches!(&refused, Err(Error::Unsupported(Subsystem::RoomHistory))),
            "{refused:?}"
        );
        proxy.shutdown().await;
        return;
    }
    let proxied = client
        .api()
        .await
        .expect("the server's API")
        .room_history(None, &player.room, base)
        .await
        .expect("the history chunk through the proxy");
    // Fetched later, the direct chunk holds at least the same ticks.
    let expected = straight
        .api()
        .await
        .expect("the server's API")
        .room_history(None, &player.room, base)
        .await
        .expect("the history chunk");
    assert_eq!(proxied.base(), base);
    let ticks: Vec<u64> = proxied.ticks().collect();
    assert!(ticks.len() >= 2, "{ticks:?}");
    assert!(expected.ticks().collect::<Vec<_>>().starts_with(&ticks));
    let last = *ticks.last().expect("a tick");
    // The room at `last`, replayed from `history`, as comparable text.
    let state_at = |history: overseer_net::RoomHistory| {
        let (_, state) = history
            .replay()
            .find(|(tick, _)| *tick == last)
            .expect("the tick");
        let mut objects: Vec<String> = state
            .objects()
            .map(|(id, object)| format!("{id}={object:?}"))
            .collect();
        objects.sort_unstable();
        objects
    };
    let replayed = state_at(proxied);
    assert!(replayed.iter().any(|object| object.contains("Spawn1")));
    assert_eq!(replayed, state_at(expected));

    // A chunk that is not kept is HTTP 404 with the server's message.
    let missing = client
        .api()
        .await
        .expect("the server's API")
        .room_history(None, &player.room, 0)
        .await;
    assert!(
        matches!(
            &missing,
            Err(Error::Api(ApiError::Status { status, message }))
                if status.as_u16() == 404 && message.as_deref() == Some("history not found")
        ),
        "{missing:?}"
    );
    proxy.shutdown().await;
}
