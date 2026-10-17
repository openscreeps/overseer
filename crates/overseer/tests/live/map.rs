//! Live tests: the map (`Map`): terrain, map statistics, world size, room status and room map
//! views. The continuous `WorldMap` has its own module.

use std::sync::Arc;

use overseer::core::api::Tile;
use overseer::core::api::types::GameRoomStatName;
use overseer::map::{RoomMap, Terrain};
use overseer::{MapStat, Overseer, StatInterval};
use tokio::time::timeout;

use crate::support::{self, PATIENCE};

/// World coordinates of a room name: `W0` is x -1 and `E0` x 0, `N0` is y -1 and `S0` y 0.
fn coords(room: &str) -> (i32, i32) {
    let split = room.find(['N', 'S']).expect("a room name");
    let (horizontal, vertical) = room.split_at(split);
    let axis = |part: &str, negative: char| {
        let value: i32 = part[1..].parse().expect("a room name");
        if part.starts_with(negative) {
            -value - 1
        } else {
            value
        }
    };
    (axis(horizontal, 'W'), axis(vertical, 'N'))
}

/// The room name at world coordinates; see [`coords`].
fn name((x, y): (i32, i32)) -> String {
    let axis = |value: i32, negative: char, positive: char| {
        if value < 0 {
            format!("{negative}{}", -value - 1)
        } else {
            format!("{positive}{value}")
        }
    };
    format!("{}{}", axis(x, 'W', 'E'), axis(y, 'N', 'S'))
}

/// The world's extent: the coordinates of its north-west and south-east rooms.
async fn bounds(overseer: &Overseer) -> ((i32, i32), (i32, i32)) {
    let world = support::world(overseer.client()).await;
    let (mut min, mut max) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
    for (x, y) in world.stats.keys().map(|room| coords(room)) {
        min = (min.0.min(x), min.1.min(y));
        max = (max.0.max(x), max.1.max(y));
    }
    assert!(min.0 <= max.0, "the world has rooms");
    (min, max)
}

/// How many tiles of the border `edge` of `terrain` are walls.
fn walls(terrain: &Terrain, edge: impl Fn(u8) -> (u8, u8)) -> usize {
    (0..50)
        .filter(|&i| {
            let (x, y) = edge(i);
            terrain.get(x, y) == Some(Tile::Wall)
        })
        .count()
}

/// Asserts that `terrain` has 50×50 tiles and walls in its corners.
fn assert_room_shaped(room: &str, terrain: &Terrain) {
    for y in 0..50 {
        for x in 0..50 {
            assert!(terrain.get(x, y).is_some(), "{room}: no tile at {x},{y}");
        }
    }
    assert_eq!(terrain.get(50, 0), None);
    assert_eq!(terrain.get(0, 50), None);
    for (x, y) in [(0, 0), (49, 0), (0, 49), (49, 49)] {
        assert_eq!(
            terrain.get(x, y),
            Some(Tile::Wall),
            "{room}: corner {x},{y}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn terrain_decodes_for_single_rooms_and_batches() {
    let overseer = support::register().await.connect().await;
    let (min, max) = bounds(&overseer).await;
    let (north_west, south_east) = (name(min), name(max));
    let inner = name((i32::midpoint(min.0, max.0), i32::midpoint(min.1, max.1)));
    let outside = name((max.0 + 5, max.1 + 5));
    let batch = overseer
        .map()
        .terrain(
            [&north_west, &south_east, &inner, &outside].map(String::as_str),
            None,
        )
        .await
        .expect("batch terrain decodes");
    let mut known = vec![north_west.as_str(), south_east.as_str(), inner.as_str()];
    known.sort_unstable();
    assert_eq!(
        batch.keys().map(String::as_str).collect::<Vec<_>>(),
        known,
        "rooms outside the world are absent"
    );
    for (room, terrain) in &batch {
        assert_room_shaped(room, terrain);
    }
    // Borders facing outside the world are walls; rooms inside have exits.
    let corner = &batch[&north_west];
    assert_eq!(walls(corner, |i| (i, 0)), 50, "{north_west} north");
    assert_eq!(walls(corner, |i| (0, i)), 50, "{north_west} west");
    let corner = &batch[&south_east];
    assert_eq!(walls(corner, |i| (i, 49)), 50, "{south_east} south");
    assert_eq!(walls(corner, |i| (49, i)), 50, "{south_east} east");
    let middle = &batch[&inner];
    let border = walls(middle, |i| (i, 0))
        + walls(middle, |i| (i, 49))
        + walls(middle, |i| (0, i))
        + walls(middle, |i| (49, i));
    assert!(border < 200, "{inner} has exits");

    // The single-room endpoint (another client, no shared cache) agrees with the batch.
    let single = support::anonymous()
        .terrain(None, &inner)
        .await
        .expect("single terrain decodes");
    assert_eq!(*single, **middle);
    // A room fetched in the batch is served from the session's cache.
    let cached = overseer
        .rooms()
        .terrain(&north_west, None)
        .await
        .expect("cached terrain");
    assert!(Arc::ptr_eq(&cached, &batch[&north_west]));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn owner_statistics_name_players_and_the_invader() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let stronghold = support::stronghold(overseer.client()).await;
    let (_, max) = bounds(&overseer).await;
    let outside = name((max.0 + 5, max.1 + 5));
    let map = overseer.map();

    let stats = map
        .stats(
            [player.room.as_str(), &stronghold, &outside],
            MapStat::Owner,
            None,
        )
        .await
        .expect("owner statistics decode");
    assert!(stats.game_time > 0);
    assert!(
        !stats.stats.contains_key(&outside),
        "rooms outside the world are absent"
    );

    let mine = &stats.stats[&player.room];
    assert_eq!(mine.status.as_deref(), Some("normal"));
    let own = mine.own.as_ref().expect("the player's room is owned");
    assert_eq!(own.user.as_deref(), Some(user.as_str()));
    assert!(own.level >= 1, "level {}", own.level);
    assert_eq!(
        stats
            .users
            .get(&user)
            .and_then(|owner| owner.username.as_deref()),
        Some(player.account.username.as_str())
    );

    let invaded = &stats.stats[&stronghold];
    let own = invaded
        .own
        .as_ref()
        .expect("the invader core owns the room");
    assert_eq!(own.user.as_deref(), Some("2"));
    assert!(own.level >= 1, "level {}", own.level);
    assert_eq!(
        stats
            .users
            .get("2")
            .and_then(|invader| invader.username.as_deref()),
        Some("Invader")
    );
    let mineral = invaded.minerals0.as_ref().expect("the room has a mineral");
    assert!(mineral.r#type.is_some() && mineral.density.is_some());

    for stat in [
        GameRoomStatName::EnergyHarvested,
        GameRoomStatName::CreepsProduced,
    ] {
        let ranked = map
            .stats(
                [player.room.as_str()],
                MapStat::Ranked(stat, StatInterval::EightMinutes),
                None,
            )
            .await
            .unwrap_or_else(|error| panic!("{stat:?} statistics decode: {error}"));
        assert!(ranked.stats.contains_key(&player.room));
    }
}

/// The world size the server reports and Overseer's reading of it both match the rooms the
/// server has. Servers measure each axis from its westernmost (northernmost) room to its
/// easternmost (southernmost) one but always count the `E0` (`S0`) line (`@screeps/common`
/// `calcWorldSize` starts the maximum at 0): an `openscreeps/server` world spans both sides of
/// `W0`|`E0` and `N0`|`S0` and reports its own size, the community default world (`W10N10` to
/// `W0N0`) reports one room more per axis. [`overseer::map::RoomRect`] undoes that from the
/// sides the central rooms are on, so `Map::world` spans exactly the server's rooms.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn world_size_and_room_status_follow_the_world() {
    let overseer = support::register().await.connect().await;
    let (min, max) = bounds(&overseer).await;
    let size = overseer
        .map()
        .world_size(None)
        .await
        .expect("world size decodes");
    assert_eq!(
        (size.width, size.height),
        (
            i64::from(max.0.max(0) - min.0 + 1),
            i64::from(max.1.max(0) - min.1 + 1)
        ),
        "the world spans {} to {}",
        name(min),
        name(max)
    );
    let world = overseer.map().world(None).await.expect("the world opens");
    let rect = world.bounds();
    assert_eq!(
        (
            name((rect.min.x, rect.min.y)),
            name((rect.max.x, rect.max.y))
        ),
        (name(min), name(max)),
        "Overseer's world bounds are the server's rooms"
    );

    let rooms = overseer.rooms();
    for room in [name(min), name(max)] {
        let inside = rooms.status(&room, None).await.expect("room status");
        assert_eq!(inside.status.as_deref(), Some("normal"), "{room}");
    }
    let outside = rooms
        .status(&name((max.0 + 5, max.1 + 5)), None)
        .await
        .expect("room status");
    assert_eq!(
        (
            outside.status,
            outside.novice,
            outside.respawn_area,
            outside.open_time
        ),
        (None, None, None, None)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn map_views_show_the_players_objects() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let stronghold = support::stronghold(overseer.client()).await;
    let mut view = overseer.map().view(None).expect("a map view");
    let change = view.set_rooms([player.room.clone(), stronghold.clone()]);
    let mut wanted = vec![player.room.clone(), stronghold.clone()];
    wanted.sort();
    assert_eq!(change.added, wanted);
    assert_eq!(change.removed, Vec::<String>::new());

    // Owned objects (the claimed controller too) are listed under their owner's id.
    let controller = overseer
        .rooms()
        .objects(&player.room, None)
        .await
        .expect("room objects")
        .objects
        .into_iter()
        .find(|object| object.kind() == "controller")
        .map(|object| [object.x(), object.y()])
        .expect("the room has a controller");
    let spawn = [u64::from(player.spawn.0), u64::from(player.spawn.1)];
    let shows_player = |map: &RoomMap| {
        map.other
            .get(&user)
            .is_some_and(|positions| positions.contains(&spawn) && positions.contains(&controller))
    };
    let shows_stronghold = |map: &RoomMap| {
        map.other
            .get("2")
            .is_some_and(|positions| !positions.is_empty())
            && map.k.as_ref().is_some_and(|lairs| !lairs.is_empty())
    };
    timeout(PATIENCE, async {
        loop {
            let views = view.views();
            if views.get(&player.room).is_some_and(|map| shows_player(map))
                && views
                    .get(&stronghold)
                    .is_some_and(|map| shows_stronghold(map))
            {
                break;
            }
            view.changed().await.expect("the session stays open");
        }
    })
    .await
    .unwrap_or_else(|_| panic!("map views: {:?}", view.views()));

    let mine = view.latest(&player.room).expect("a map view");
    assert_eq!(
        mine.c.as_ref().map(Vec::len),
        Some(0),
        "no unowned controller"
    );
    assert!(mine.s.as_ref().is_some_and(|sources| !sources.is_empty()));
    assert_eq!(mine.m.as_ref().map(Vec::len), Some(1), "one mineral");

    let change = view.set_rooms([player.room.clone()]);
    assert_eq!(change.removed, [stronghold]);
    assert_eq!(change.added, Vec::<String>::new());
    assert_eq!(view.rooms().collect::<Vec<_>>(), [player.room.as_str()]);
}
