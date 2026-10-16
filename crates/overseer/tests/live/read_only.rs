//! Read-only tests of the [`support::reader`] account: the session, the account profile and the
//! start room. They only read (GET requests and the socket), so they also run against
//! screeps.com (`cargo xtask e2e --target official`) and catch contract drift there.

use overseer::core::api::Tile;
use overseer::core::api::types::{GameRoomObject, UserWorldStatus};
use overseer::{ConnectionStatus, Overseer};
use tokio::time::timeout;

use crate::support::{self, PATIENCE};

/// The reader's session connects and its account profile decodes: `GET /api/version`,
/// `GET /api/auth/me`, the upgrade `GET /socket/websocket` (connect), `GET /api/auth/me`
/// (profile), `GET /api/user/world-status`, `GET /api/user/world-start-room`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live server: cargo xtask e2e (read-only, also --target official)"]
async fn the_readers_session_and_profile_decode() {
    let overseer = Overseer::connect(support::reader())
        .await
        .expect("the reader connects");
    let world = overseer.world();
    assert_eq!(world.is_sharded(), support::official());
    let mut status = overseer.status();
    timeout(
        PATIENCE,
        status.wait_for(|status| *status == ConnectionStatus::Connected),
    )
    .await
    .expect("the socket connects in time")
    .expect("the session is open");

    let account = overseer.account();
    let me = account.me().await.expect("the profile");
    assert_eq!(overseer.user_id(), Some(me.id.as_str()));
    assert!(
        me.username.as_deref().is_some_and(|name| !name.is_empty()),
        "the profile names no user"
    );
    let status = account.world_status().await.expect("the world status");
    let (shard, room) = support::start_room(&overseer).await;
    match &shard {
        Some(shard) => assert!(
            world.shards().contains(shard),
            "{shard} is not one of {:?}",
            world.shards()
        ),
        None => assert!(!world.is_sharded(), "a sharded server names the shard"),
    }
    assert!(
        room.starts_with('W') || room.starts_with('E'),
        "not a room name: {room}"
    );
    if support::official() {
        // On screeps.com the reader plays.
        assert_eq!(status, UserWorldStatus::Normal);
        assert!(me.gcl.is_some_and(|gcl| gcl > 0.0), "GCL {:?}", me.gcl);
        assert!(me.cpu.is_some_and(|cpu| cpu > 0.0), "CPU {:?}", me.cpu);
    } else {
        assert_eq!(
            me.username.as_deref(),
            Some(support::admin().username.as_str())
        );
    }
    overseer.shutdown().await;
}

/// Watching the reader's start room yields a decoded snapshot of it: `GET /api/version`,
/// `GET /api/auth/me`, the upgrade `GET /socket/websocket` (connect),
/// `GET /api/user/world-start-room`, `GET /api/game/room-terrain` (watch), then the room's
/// socket subscription.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live server: cargo xtask e2e (read-only, also --target official)"]
async fn watching_the_start_room_yields_its_snapshot() {
    let overseer = Overseer::connect(support::reader())
        .await
        .expect("the reader connects");
    let user = overseer.user_id().expect("signed in").to_owned();
    let (shard, room) = support::start_room(&overseer).await;
    let rooms = overseer.rooms();
    let mut watch = match &shard {
        Some(shard) => rooms.watch_on(shard, &room).await,
        None => rooms.watch(&room).await,
    }
    .expect("the start room is watched");

    let snapshot = timeout(PATIENCE, async {
        loop {
            let snapshot = watch.changed().await.expect("the session stays open");
            assert_eq!(snapshot.subscription_error, None, "{room} is refused");
            if snapshot.game_time.is_some() {
                return snapshot;
            }
        }
    })
    .await
    .expect("a snapshot of the start room in time");
    assert_eq!(snapshot.room, room);
    assert_eq!(snapshot.shard, shard);
    assert!(
        snapshot.undecoded.is_empty(),
        "undecodable objects in {room}: {:?}",
        snapshot.undecoded
    );
    assert_ne!(snapshot.objects.len(), 0, "{room} is empty");
    let tiles: Vec<Tile> = (0..50)
        .flat_map(|y| (0..50).map(move |x| (x, y)))
        .map(|(x, y)| snapshot.terrain.get(x, y).expect("a tile"))
        .collect();
    assert!(tiles.contains(&Tile::Wall), "a room has walls");
    if support::official() {
        // On screeps.com the reader owns its start room.
        assert!(
            snapshot.objects.iter().any(|object| matches!(
                object,
                GameRoomObject::Controller(controller)
                    if controller.user.as_deref() == Some(user.as_str())
            )),
            "the reader does not own {room}"
        );
        assert!(
            snapshot.users.contains_key(&user),
            "the reader is not a user of {room}"
        );
    }
    overseer.shutdown().await;
}
