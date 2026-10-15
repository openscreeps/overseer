//! Room snapshots, terrain, room history, and Memory and the console over HTTP.

use std::sync::Arc;
use std::time::Duration;

use overseer_net::core::api::types::GameRoomObject;
use overseer_net::core::api::{ApiError, MemoryValue, Tile};
use overseer_net::serde_json::{Value, json};
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
async fn room_objects_and_terrain_decode() {
    let player = support::player().await;
    let client = player.account.client();
    let user = client
        .world()
        .await
        .expect("sign-in")
        .user()
        .expect("a user")
        .to_owned();
    let snapshot = client
        .room_objects(None, &player.room)
        .await
        .expect("the room's objects decode");
    let spawn = snapshot
        .objects
        .iter()
        .find_map(|object| match object {
            GameRoomObject::Spawn(spawn) => Some(spawn),
            _ => None,
        })
        .expect("the spawn");
    assert_eq!(spawn.name.as_deref(), Some("Spawn1"));
    assert_eq!(spawn.user, user);
    assert_eq!(spawn.room, player.room);
    assert_eq!(
        (spawn.x, spawn.y),
        (u64::from(player.spawn.0), u64::from(player.spawn.1))
    );
    assert!(
        snapshot
            .objects
            .iter()
            .any(|object| matches!(object, GameRoomObject::Controller(_)))
    );
    assert_eq!(
        snapshot
            .users
            .get(&user)
            .and_then(|user| user.username.as_deref()),
        Some(player.account.username.as_str())
    );
    // Another owned room (the Invader's or another player's; else any other room) holds other
    // kinds of objects; they decode as well.
    let map = support::world_map(&client).await;
    let others: Vec<(&String, bool)> = map
        .stats
        .iter()
        .filter(|(room, _)| **room != player.room)
        .map(|(room, stat)| (room, stat.own.is_some()))
        .collect();
    let other = others
        .iter()
        .find(|(_, owned)| *owned)
        .or_else(|| others.first())
        .map(|(room, _)| room.as_str())
        .expect("another room");
    let snapshot = client
        .room_objects(None, other)
        .await
        .expect("the other room's objects decode");
    assert_ne!(snapshot.objects.len(), 0);

    let terrain = client
        .terrain(None, &player.room)
        .await
        .expect("the room's terrain");
    assert_eq!(
        terrain.get(player.spawn.0, player.spawn.1),
        Some(Tile::Plain)
    );
    let cached = client.terrain(None, &player.room).await.expect("cached");
    assert!(Arc::ptr_eq(&terrain, &cached), "terrain is cached");
    let several = client
        .terrains(None, [player.room.as_str(), other, "W99N99"])
        .await
        .expect("several terrains");
    assert_eq!(
        several.keys().map(String::as_str).collect::<Vec<_>>(),
        {
            let mut rooms = vec![player.room.as_str(), other];
            rooms.sort_unstable();
            rooms
        },
        "unknown rooms are absent"
    );
    assert_eq!(*several[&player.room], *terrain);
    // Every tile is known, and the room has walls as well as open ground.
    let tiles: Vec<Tile> = (0..50)
        .flat_map(|y| (0..50).map(move |x| (x, y)))
        .map(|(x, y)| terrain.get(x, y).expect("a tile"))
        .collect();
    assert!(tiles.contains(&Tile::Wall));
    assert!(tiles.contains(&Tile::Plain));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn room_history_replays_the_room() {
    let player = support::player().await;
    let client = player.account.client();
    let world = client.world().await.expect("sign-in");
    let size = world.history_chunk_size();
    // The first chunk that starts after the spawn was placed, once it holds a few ticks (the
    // server serves the chunk being written).
    let placed = support::game_time(&client).await;
    let base = placed - placed % size + size;
    support::wait_for_tick(&client, base + 4).await;
    let history = client
        .room_history(None, &player.room, base + 1)
        .await
        .expect("the history chunk");
    assert_eq!(history.room(), player.room);
    assert_eq!(history.base(), base);
    assert!(history.timestamp() > 0.0);
    let ticks: Vec<u64> = history.ticks().collect();
    assert_eq!(ticks.first(), Some(&base));
    assert!(ticks.len() >= 2, "{ticks:?}");
    assert!(ticks.len() <= usize::try_from(size).expect("a small chunk"));
    assert!(ticks.iter().all(|tick| (base..base + size).contains(tick)));

    let mut replay = history.replay();
    let first = replay.advance().expect("the first tick");
    assert_eq!(first, base);
    let spawns = |replay: &overseer_net::HistoryReplay| {
        replay
            .state()
            .typed_objects()
            .filter_map(|(id, object)| {
                match object.unwrap_or_else(|error| panic!("object {id} does not decode: {error}"))
                {
                    GameRoomObject::Spawn(spawn) => spawn.name,
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(spawns(&replay), ["Spawn1"]);
    let mut last = first;
    while let Some(tick) = replay.advance() {
        assert!(tick > last);
        last = tick;
        assert_eq!(replay.state().game_time(), Some(tick));
        assert_eq!(spawns(&replay), ["Spawn1"]);
    }

    // A chunk that is not kept is HTTP 404.
    let missing = client.room_history(None, &player.room, 0).await;
    assert!(
        matches!(
            &missing,
            Err(Error::Api(ApiError::Status { status, message }))
                if status.as_u16() == 404 && message.as_deref() == Some("history not found")
        ),
        "{missing:?}"
    );
}

/// The player's Memory at `path`.
async fn memory(client: &Client, path: &str) -> MemoryValue {
    client
        .memory(None, Some(path))
        .await
        .expect("Memory decodes")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn memory_and_console_requests_reach_the_runtime() {
    let player = support::player().await;
    let client = player.account.client();
    // The bot counts its runs.
    eventually(|| {
        let client = client.clone();
        async move {
            match memory(&client, "ticks").await {
                MemoryValue::Json(ticks) => ticks.as_u64().filter(|&ticks| ticks > 0),
                _ => None,
            }
        }
    })
    .await;
    assert_eq!(memory(&client, "absent").await, MemoryValue::Undefined);
    assert_eq!(
        memory(&client, "absent.deeper").await,
        MemoryValue::IncorrectPath
    );
    let MemoryValue::Json(all) = client.memory(None, None).await.expect("all of Memory") else {
        panic!("Memory is an object");
    };
    assert!(all.get("ticks").is_some_and(Value::is_number));

    let value = json!({ "flag": true, "list": [1, "two"] });
    client
        .set_memory(None, "live.value", value.clone())
        .await
        .expect("the write is queued");
    eventually(|| {
        let (client, value) = (client.clone(), value.clone());
        async move { (memory(&client, "live.value").await == MemoryValue::Json(value)).then_some(()) }
    })
    .await;
    assert_eq!(
        memory(&client, "live.value.flag").await,
        MemoryValue::Json(json!(true))
    );
    client
        .delete_memory(None, "live.value")
        .await
        .expect("the deletion is queued");
    eventually(|| {
        let client = client.clone();
        async move { (memory(&client, "live.value").await == MemoryValue::Undefined).then_some(()) }
    })
    .await;

    // A console expression runs in the player's runtime: it writes Memory.
    client
        .console(None, "Memory.fromConsole = 6 * 7")
        .await
        .expect("the expression is queued");
    eventually(|| {
        let client = client.clone();
        async move {
            (memory(&client, "fromConsole").await == MemoryValue::Json(json!(42))).then_some(())
        }
    })
    .await;
    let long = format!("'{}'", "x".repeat(1100));
    let result = client.console(None, long).await;
    assert!(
        matches!(&result, Err(Error::Api(ApiError::Server(message))) if message == "expression size is too large"),
        "{result:?}"
    );
}
