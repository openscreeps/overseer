//! HTTP through the proxy: discovery, sign-in, token rotation, authenticated requests and room
//! history.

use std::time::Duration;

use overseer_net::core::api::types::{AuthSigninRequest, RegisterSubmitRequest};
use overseer_net::core::api::{ApiError, MemoryValue, endpoints};
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
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
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

    // Registration and its logical failures (HTTP 200 `{error}`) pass unchanged.
    let account = support::register_with(&server, None).await;
    let again = RegisterSubmitRequest {
        email: format!("other-{}", account.email),
        password: account.password.clone(),
        username: account.username.clone(),
        ..RegisterSubmitRequest::default()
    };
    let result = anonymous
        .send(&endpoints::RegisterSubmit { body: again })
        .await;
    assert!(
        matches!(&result, Err(Error::Api(ApiError::Server(message))) if message == "username already exists"),
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
    let wrong = endpoints::AuthSignin {
        body: AuthSigninRequest {
            email: account.username.clone(),
            password: "not-the-password".to_owned(),
            recaptcha: None,
            steam_ticket: None,
        },
    };
    let result = anonymous.send(&wrong).await;
    assert!(
        matches!(result, Err(Error::Api(ApiError::Unauthorized))),
        "{result:?}"
    );
    proxy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
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
    client.send(&endpoints::AuthMe).await.expect("auth/me");
    let second = client.token().expect("a token");
    assert_ne!(first, second);
    assert!(updates.has_changed().expect("the client is alive"));
    assert_eq!(updates.borrow_and_update().as_ref(), Some(&second));

    // A token rotated through the proxy is valid upstream, and the other way round.
    let straight = Client::builder(support::direct())
        .token(second)
        .build()
        .expect("a client");
    let me = straight
        .send(&endpoints::AuthMe)
        .await
        .expect("auth/me")
        .body;
    assert_eq!(Some(me.id), user);
    let back = Client::builder(server.clone())
        .token(straight.token().expect("a token"))
        .build()
        .expect("a client");
    let me = back.send(&endpoints::AuthMe).await.expect("auth/me").body;
    assert_eq!(Some(me.id), user);

    // Authenticated writes and reads: Memory through the console.
    client
        .console(None, "Memory.viaProxy = 'yes'")
        .await
        .expect("the expression is queued");
    eventually(|| {
        let client = client.clone();
        async move {
            let value = client
                .memory(None, Some("viaProxy"))
                .await
                .expect("Memory decodes");
            (value == MemoryValue::Json(json!("yes"))).then_some(())
        }
    })
    .await;
    client
        .set_memory(None, "viaProxy", json!({ "nested": [1, 2] }))
        .await
        .expect("the write is queued");
    eventually(|| {
        let client = client.clone();
        async move {
            let value = client
                .memory(None, Some("viaProxy.nested"))
                .await
                .expect("Memory decodes");
            (value == MemoryValue::Json(json!([1, 2]))).then_some(())
        }
    })
    .await;
    proxy.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
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
    let proxied = client
        .room_history(None, &player.room, base)
        .await
        .expect("the history chunk through the proxy");
    // Fetched later, the direct chunk holds at least the same ticks.
    let expected = straight
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
    let missing = client.room_history(None, &player.room, 0).await;
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
