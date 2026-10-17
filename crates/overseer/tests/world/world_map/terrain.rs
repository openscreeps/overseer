//! Whole-world terrain: batches nearest first, once per session, and cancel safety of `next`.

use std::collections::BTreeSet;
use std::sync::Arc;

use futures_util::FutureExt;
use overseer::map::{Interest, RoomCoord, TERRAIN_BATCH, WorldEvent};

use super::{Fake, event, map_stats, quiet, rect, requested, room};
use crate::connect;

#[tokio::test]
async fn whole_world_terrain_is_batched_nearest_first_once_per_session() {
    // 60 × 60 rooms on the server; the interest asks for 70 × 70.
    let fake = Fake::new(60, rect("W29N29", "E29S29"));
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    // The probe's terrain is cached by the client, not seeded: it is reported, never sent.
    let centre = ["W0N0", "E0N0", "W0S0", "E0S0"].map(room);

    let seeded = room("W5N5");
    let terrain = overseer.map().terrain(["W0N0"], None).await.unwrap()["W0N0"].clone();
    world.insert_terrain(seeded, Arc::clone(&terrain));
    assert!(Arc::ptr_eq(&world.terrain(seeded).unwrap(), &terrain));
    let seeded_absent = room("W34N34");
    world.insert_no_terrain(seeded_absent);
    assert!(world.has_no_terrain(seeded_absent));

    let origin = room("W0N0");
    let distance = |room: &RoomCoord| {
        let (dx, dy) = (room.x - origin.x, room.y - origin.y);
        (dx * dx + dy * dy, room.y, room.x)
    };
    let mut nearest = rect("W34N34", "E34S34").iter().collect::<Vec<_>>();
    nearest.sort_by_key(distance);
    let mut wanted = nearest.clone();
    wanted.extend([nearest[3], nearest[2500], seeded]);
    world.set_interest(&Interest {
        terrain: wanted.clone(),
        ..Interest::default()
    });
    let progress = world.progress();
    assert_eq!(
        (
            progress.terrain_known,
            progress.terrain_absent,
            progress.terrain_wanted
        ),
        (1, 1, 4900)
    );
    assert!(!progress.is_complete());

    // Every room once: 3 599 terrains and 1 299 rooms without one, each reported once. Midway
    // the interest turns around: rooms known or in flight are not asked again.
    let (mut fetched, mut absent) = (BTreeSet::new(), BTreeSet::new());
    let mut turned = false;
    while fetched.len() + absent.len() < 4898 {
        match event(&mut world).await {
            WorldEvent::Terrain(room, _) => assert!(fetched.insert(room), "{room} twice"),
            WorldEvent::NoTerrain(room) => assert!(absent.insert(room), "{room} twice"),
            other => panic!("unexpected {other:?}"),
        }
        if !turned && fetched.len() + absent.len() >= 1000 {
            turned = true;
            let mut reversed = wanted.clone();
            reversed.reverse();
            world.set_interest(&Interest {
                terrain: reversed,
                ..Interest::default()
            });
        }
    }
    assert_eq!((fetched.len(), absent.len()), (3599, 1299));
    assert!(!fetched.contains(&seeded) && !absent.contains(&seeded_absent));
    assert!(absent.iter().all(|&room| !fake.rooms.contains(room)));
    let progress = world.progress();
    assert_eq!(
        (
            progress.terrain_known,
            progress.terrain_absent,
            progress.terrain_wanted
        ),
        (3600, 1300, 4900)
    );
    assert!(progress.is_complete());

    let queue = nearest
        .iter()
        .copied()
        .filter(|&room| room != seeded && room != seeded_absent)
        .collect::<Vec<_>>();
    let as_sent = |rooms: &[RoomCoord]| {
        rooms
            .iter()
            .filter(|room| !centre.contains(room))
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>()
    };
    let batches = requested(&server, "/api/game/rooms");
    assert_eq!(batches.len(), 4, "the probe and three batches");
    // The first two batches (in flight together) take the 4 000 nearest rooms.
    let first_two = batches[1..3]
        .iter()
        .map(|batch| batch.iter().cloned().collect::<BTreeSet<_>>())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        first_two,
        [
            as_sent(&queue[..TERRAIN_BATCH]),
            as_sent(&queue[TERRAIN_BATCH..2 * TERRAIN_BATCH])
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
    // The rest follows the turned interest: farthest first now.
    let last = batches[3].iter().map(|name| room(name)).collect::<Vec<_>>();
    assert_eq!(
        last.iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>(),
        as_sent(&queue[2 * TERRAIN_BATCH..])
    );
    assert!(distance(&last[0]) > distance(&last[last.len() - 1]));
    let sent = batches[1..].iter().map(Vec::len).sum::<usize>();
    assert_eq!(sent, queue.len() - centre.len(), "no room sent twice");

    // Known rooms and rooms without terrain are never requested again.
    world.set_interest(&Interest {
        terrain: nearest,
        ..Interest::default()
    });
    quiet(&mut world).await;
    assert_eq!(requested(&server, "/api/game/rooms").len(), 4);
}

#[tokio::test]
async fn next_is_cancel_safe() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let rooms = [room("W1N1"), room("W2N2")];
    world.set_interest(&Interest {
        terrain: rooms.to_vec(),
        stats: Some(rect("W1N1", "W1N1")),
        ..Interest::default()
    });

    // The first poll starts both requests; dropping it loses neither: driving `next` again
    // completes them instead of starting new ones.
    assert!(world.next().now_or_never().is_none());
    let mut terrain = Vec::new();
    let mut stats = 0;
    while terrain.len() < 2 || stats == 0 {
        match event(&mut world).await {
            WorldEvent::Terrain(room, _) => terrain.push(room),
            WorldEvent::Stats(changed) => stats += changed.len(),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(terrain, rooms);
    // Overviews cover the whole world.
    assert_eq!(stats, world.bounds().len());
    assert_eq!(map_stats(&server).len(), 1);
    // The wanted rooms go in one batch with the prefetch ring (the background sweep may run
    // alongside); no room is asked twice.
    let batches = requested(&server, "/api/game/rooms");
    let foreground = batches[1..]
        .iter()
        .find(|batch| batch.iter().any(|name| name == "W1N1"))
        .unwrap();
    assert!(foreground.iter().any(|name| name == "W2N2"));
    let all = batches[1..].iter().flatten().collect::<Vec<_>>();
    assert_eq!(all.iter().collect::<BTreeSet<_>>().len(), all.len());
}
