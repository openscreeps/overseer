//! Live tests: the continuous world map (`WorldMap`): bounds, overviews, terrain, minimaps and
//! detail of the disposable world, and how interest changes release minimaps and detail rooms.
//!
//! The tests do not assume where the world lies: its rooms are those the server has terrain
//! for, and it holds an invader stronghold somewhere.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use overseer::core::api::Tile;
use overseer::map::{
    DetailState, Interest, RoomCoord, RoomRect, RoomStatus, Terrain, WorldEvent, WorldMap,
};
use tokio::time::timeout;

use crate::support::{self, PATIENCE};

fn room(name: &str) -> RoomCoord {
    name.parse().expect("a room name")
}

/// The rooms of `bounds` other than `except`, nearest its centre first.
fn others(bounds: RoomRect, except: RoomCoord) -> Vec<RoomCoord> {
    let centre = bounds.center();
    let mut rooms = bounds
        .iter()
        .filter(|&room| room != except)
        .collect::<Vec<_>>();
    rooms.sort_by_key(|room| (room.x - centre.x).pow(2) + (room.y - centre.y).pow(2));
    rooms
}

/// Drives `world` until `done` accepts the world after an event, checking that every live
/// room decodes.
async fn until(
    world: &mut WorldMap,
    what: &str,
    mut done: impl FnMut(&WorldMap, &WorldEvent) -> bool,
) {
    timeout(PATIENCE, async {
        loop {
            let event = world.next().await.expect("the session stays open");
            if let WorldEvent::Detail(room, DetailState::Live(snapshot)) = &event {
                assert!(
                    snapshot.undecoded.is_empty(),
                    "undecodable objects in {room}: {:?}",
                    snapshot.undecoded
                );
            }
            if done(world, &event) {
                return;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

/// Every event of `world` during `duration`.
async fn events_during(world: &mut WorldMap, duration: Duration) -> Vec<WorldEvent> {
    let mut events = Vec::new();
    timeout(duration, async {
        loop {
            events.push(world.next().await.expect("the session stays open"));
        }
    })
    .await
    .expect_err("the world map never ends");
    events
}

/// Asserts that `terrain` has 50×50 tiles and walls in its corners.
fn assert_room_shaped(room: RoomCoord, terrain: &Terrain) {
    for y in 0..50 {
        for x in 0..50 {
            assert!(terrain.get(x, y).is_some(), "{room}: tile {x},{y}");
        }
    }
    assert_eq!(terrain.get(50, 0), None);
    for (x, y) in [(0, 0), (49, 0), (0, 49), (49, 49)] {
        assert_eq!(
            terrain.get(x, y),
            Some(Tile::Wall),
            "{room}: corner {x},{y}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn bounds_span_the_disposable_world() {
    let overseer = support::register().await.connect().await;
    let map = overseer.map();
    let world = map.world(None).await.expect("the world opens");
    let bounds = world.bounds();
    assert!(!bounds.is_empty(), "{bounds:?}");
    assert_eq!(world.shard(), None);

    // The bounds hold exactly the rooms the server knows, in and around them.
    let around = RoomRect {
        min: RoomCoord {
            x: bounds.min.x - 1,
            y: bounds.min.y - 1,
        },
        max: RoomCoord {
            x: bounds.max.x + 1,
            y: bounds.max.y + 1,
        },
    };
    let names = around.iter().map(|room| room.to_string());
    let terrain = map.terrain(names, None).await.expect("terrain decodes");
    let known = terrain
        .keys()
        .map(|name| room(name))
        .collect::<BTreeSet<_>>();
    assert_eq!(known, bounds.iter().collect::<BTreeSet<_>>());

    // Bounds persisted from an earlier session open the same world without measuring it.
    let known = map
        .world_with_bounds(None, bounds)
        .expect("the world opens");
    assert_eq!(known.bounds(), bounds);
    assert_eq!(known.shard(), None);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn overviews_cover_the_whole_world() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let mut world = overseer.map().world(None).await.expect("the world opens");
    let bounds = world.bounds();

    // A view larger than the world is clipped to it; each room is reported once.
    let margin = |room: RoomCoord, by: i32| RoomCoord {
        x: room.x + by,
        y: room.y + by,
    };
    world.set_interest(&Interest {
        stats: Some(RoomRect::new(margin(bounds.min, -5), margin(bounds.max, 5))),
        ..Interest::default()
    });
    let mut reported = BTreeSet::new();
    until(&mut world, "every overview", |_, event| {
        if let WorldEvent::Stats(stats) = event {
            for (room, _) in stats {
                assert!(reported.insert(*room), "{room} reported twice");
            }
        }
        reported.len() == bounds.len()
    })
    .await;
    assert!(reported.iter().all(|&room| bounds.contains(room)));

    let mine = world
        .stat(room(&player.room))
        .expect("the player's overview");
    assert_eq!(mine.status, RoomStatus::Normal);
    let owner = mine.owner.as_ref().expect("the player owns the room");
    assert_eq!(owner.id, user);
    assert_eq!(
        owner.username.as_deref(),
        Some(player.account.username.as_str())
    );
    assert!(owner.level >= 1, "level {}", owner.level);

    let stronghold = bounds
        .iter()
        .filter_map(|room| world.stat(room))
        .find(|stat| stat.owner.as_ref().is_some_and(|owner| owner.id == "2"))
        .expect("an invader core owns a room");
    let owner = stronghold.owner.as_ref().expect("checked above");
    assert_eq!(owner.username.as_deref(), Some("Invader"));
    assert!(owner.level >= 1, "level {}", owner.level);
    assert!(
        bounds
            .iter()
            .filter_map(|room| world.stat(room))
            .any(|stat| stat.mineral.is_some() && stat.mineral_density.is_some()),
        "minerals"
    );

    // Overviews live for the TTL: nothing is asked or reported again meanwhile.
    let later = events_during(&mut world, Duration::from_secs(2)).await;
    assert!(later.is_empty(), "{later:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn terrain_fills_in_and_seeded_rooms_are_not_fetched() {
    let overseer = support::register().await.connect().await;
    let map = overseer.map();
    let mut world = map.world(None).await.expect("the world opens");
    let bounds = world.bounds();
    let centre = bounds.center();
    let distance = |room: &RoomCoord| {
        let (dx, dy) = (room.x - centre.x, room.y - centre.y);
        dx * dx + dy * dy
    };
    let mut wanted = bounds.iter().collect::<Vec<_>>();
    wanted.sort_by_key(distance);
    let unknown = RoomCoord {
        x: bounds.max.x + 3,
        y: bounds.max.y + 3,
    };
    wanted.push(unknown);
    world.set_interest(&Interest {
        terrain: wanted,
        ..Interest::default()
    });

    let mut reported = BTreeSet::new();
    until(&mut world, "every terrain", |_, event| {
        if let WorldEvent::Terrain(room, terrain) = event {
            assert_room_shaped(*room, terrain);
            assert!(reported.insert(*room), "{room} reported twice");
        }
        reported.len() == bounds.len()
    })
    .await;
    assert!(!reported.contains(&unknown));
    assert!(bounds.iter().all(|room| world.terrain(room).is_some()));
    assert!(world.terrain(unknown).is_none());

    // Terrain the app persisted seeds another world: never reported, the very same data (the
    // rest of the world still comes in the background).
    let terrain = world.terrain(centre).expect("the centre's terrain");
    let mut seeded = map
        .world_with_bounds(None, bounds)
        .expect("the world opens");
    seeded.insert_terrain(centre, Arc::clone(&terrain));
    seeded.set_interest(&Interest {
        terrain: vec![centre],
        ..Interest::default()
    });
    let events = events_during(&mut seeded, Duration::from_secs(1)).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, WorldEvent::Terrain(room, _) if *room == centre)),
        "{events:?}"
    );
    assert!(Arc::ptr_eq(
        &seeded.terrain(centre).expect("seeded"),
        &terrain
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn minimaps_show_the_rooms_and_stop_when_unwanted() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let mut world = overseer.map().world(None).await.expect("the world opens");
    let mine = room(&player.room);
    let other = others(world.bounds(), mine)[0];
    world.set_interest(&Interest {
        minimaps: vec![mine, other],
        ..Interest::default()
    });

    let spawn = [u64::from(player.spawn.0), u64::from(player.spawn.1)];
    until(&mut world, "both minimaps", |world, _| {
        // The player's spawn and controller are drawn as theirs (`c` lists unowned controllers).
        let shows_player = world.minimap(mine).is_some_and(|map| {
            map.other
                .get(&user)
                .is_some_and(|positions| positions.contains(&spawn))
                && map.s.as_ref().is_some_and(|sources| !sources.is_empty())
        });
        shows_player && world.minimap(other).is_some()
    })
    .await;

    // A room that leaves the interest is released: gone, and no update arrives for it.
    world.set_interest(&Interest {
        minimaps: vec![mine],
        ..Interest::default()
    });
    until(
        &mut world,
        "the other minimap gone",
        |_, event| matches!(event, WorldEvent::MinimapGone(room) if *room == other),
    )
    .await;
    assert!(world.minimap(other).is_none());
    let events = events_during(&mut world, Duration::from_secs(1)).await;
    assert!(
        events.iter().all(|event| match event {
            WorldEvent::Minimap(room, _) => *room == mine,
            _ => false,
        }),
        "{events:?}"
    );

    world.set_interest(&Interest::default());
    until(
        &mut world,
        "the player's minimap gone",
        |_, event| matches!(event, WorldEvent::MinimapGone(room) if *room == mine),
    )
    .await;
    assert!(world.minimap(mine).is_none());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn detail_stays_within_the_servers_cap_and_is_released_when_unwanted() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let mut world = overseer.map().world(None).await.expect("the world opens");
    let mine = room(&player.room);
    let [second, third, ..] = others(world.bounds(), mine)[..] else {
        panic!("a world of at least 3 rooms");
    };

    // The server sends a user 2 rooms per update: the least important one is refused.
    world.set_interest(&Interest {
        detail: vec![mine, second, third],
        terrain: vec![mine],
        ..Interest::default()
    });
    // State is readable before its events are taken from `next`: wait for the refusal event,
    // so that no event of this phase is left for the next one.
    let live = |world: &WorldMap, room| matches!(world.detail(room), Some(DetailState::Live(_)));
    let mut refused = false;
    until(&mut world, "two live rooms and a refusal", |world, event| {
        refused |= matches!(event, WorldEvent::Detail(room, DetailState::Refused(_)) if *room == third);
        refused && live(world, mine) && live(world, second)
    })
    .await;
    let Some(DetailState::Refused(error)) = world.detail(third) else {
        unreachable!("checked above");
    };
    assert_eq!(error, "subscribe limit reached");

    // The player's room shows the spawn, on the terrain the world map fetched.
    let Some(DetailState::Live(snapshot)) = world.detail(mine) else {
        unreachable!("checked above");
    };
    let spawn = (u64::from(player.spawn.0), u64::from(player.spawn.1));
    assert!(
        snapshot
            .objects
            .iter()
            .any(|object| object.kind() == "spawn" && (object.x(), object.y()) == spawn),
        "no spawn at {spawn:?}"
    );
    let terrain = world.terrain(mine).expect("the world map has the terrain");
    assert!(Arc::ptr_eq(&snapshot.terrain, &terrain));

    // A refused room stays refused while the interest stays.
    let events = events_during(&mut world, Duration::from_secs(1)).await;
    let changes = events
        .iter()
        .filter_map(|event| match event {
            WorldEvent::Detail(room, DetailState::Live(_)) => Some((*room, "live".to_owned())),
            WorldEvent::Detail(room, state) => Some((*room, format!("{state:?}"))),
            WorldEvent::DetailGone(room) => Some((*room, "gone".to_owned())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        changes.iter().all(|(room, _)| *room != third),
        "{changes:?}"
    );

    // Leaving the interest releases rooms: the server then serves the refused room.
    world.set_interest(&Interest {
        detail: vec![mine],
        ..Interest::default()
    });
    let mut gone = BTreeSet::new();
    until(&mut world, "two rooms gone", |_, event| {
        if let WorldEvent::DetailGone(room) = event {
            gone.insert(*room);
        }
        gone.len() == 2
    })
    .await;
    assert_eq!(gone, BTreeSet::from([second, third]));
    assert!(world.detail(second).is_none());
    // The server refuses an over-limit room on every update until it processes the releases,
    // and a refusal sent before then can arrive after the room is subscribed again: the world
    // map releases nothing for such refusals, so the third room is wanted again at once.
    world.set_interest(&Interest {
        detail: vec![mine, third],
        ..Interest::default()
    });
    let mut updates = 0;
    until(&mut world, "updates of the third room", |_, event| {
        match event {
            WorldEvent::Detail(room, DetailState::Refused(error)) => {
                panic!("{room} refused: {error}")
            }
            WorldEvent::Detail(room, DetailState::Live(_)) if *room == third => updates += 1,
            _ => {}
        }
        updates == 5
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn a_viewed_room_updates_its_overview_and_portals_at_once() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let user = overseer.user_id().expect("signed in").to_owned();
    let mut world = overseer.map().world(None).await.expect("the world opens");
    let knowledge = world.knowledge();
    let bounds = world.bounds();
    let mine = room(&player.room);

    // Viewing the player's room: its live update is the room's overview and portals, at
    // once, in the handle the UI reads too.
    world.set_interest(&Interest {
        stats: Some(bounds),
        detail: vec![mine],
        terrain: vec![mine],
        ..Interest::default()
    });
    let mut live_level = None;
    until(&mut world, "the live room's overview", |world, event| {
        if let WorldEvent::Detail(room, DetailState::Live(snapshot)) = event
            && *room == mine
        {
            live_level = snapshot.objects.iter().find_map(|object| match object {
                overseer::core::api::types::GameRoomObject::Controller(controller) => {
                    Some(controller.level)
                }
                _ => None,
            });
        }
        live_level.is_some()
            && world.stat(mine).is_some_and(|stat| {
                stat.owner.as_ref().map(|owner| owner.id.as_str()) == Some(user.as_str())
            })
    })
    .await;
    let stat = knowledge.stat(mine).expect("an overview");
    let owner = stat.owner.as_ref().expect("checked above");
    assert_eq!(Some(u64::from(owner.level)), live_level);
    assert_eq!(
        owner.username.as_deref(),
        Some(player.account.username.as_str())
    );
    assert_eq!(knowledge.portals(mine).as_deref(), Some(&[] as &[_]));
    assert!(knowledge.revision() > 0);

    // The whole world's overviews arrive meanwhile without undoing it.
    until(&mut world, "every overview", |world, _| {
        world.progress().stats_known == bounds.len()
    })
    .await;
    let owner = knowledge.stat(mine).and_then(|stat| stat.owner.clone());
    assert_eq!(owner.map(|owner| owner.id), Some(user));
}
