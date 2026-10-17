//! The bounds of the world map: measured from the world size and the central rooms, or given.

use futures_util::FutureExt;
use overseer::map::{Interest, WorldEvent};

use super::{Fake, event, map_stats, rect, requested, room};
use crate::{PATIENCE, connect};

#[tokio::test]
async fn bounds_come_from_the_world_size_and_the_central_rooms() {
    // OpenScreeps `--world-size 11` (like the upstream default world): W10N10..W0N0, reported
    // as 12 because the server counts the E0 column.
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let world = overseer.map().world(None).await.unwrap();
    assert_eq!(world.bounds(), rect("W10N10", "W0N0"));
    assert_eq!(world.bounds().len(), 121);
    assert_eq!(world.shard(), None);
    // One probe of the central rooms.
    assert_eq!(
        requested(&server, "/api/game/rooms"),
        [["E0N0", "E0S0", "W0N0", "W0S0"]]
    );

    // The size and the probe go out together: neither waits for the other.
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    server.hold(Some("/api/game/world-size"));
    let map = overseer.map();
    let opening = map.world(None);
    tokio::pin!(opening);
    let start = std::time::Instant::now();
    while requested(&server, "/api/game/rooms").is_empty() {
        assert!(start.elapsed() < PATIENCE, "the probe in time");
        assert!((&mut opening).now_or_never().is_none());
        tokio::task::yield_now().await;
    }
    server.hold(None);
    assert_eq!(opening.await.unwrap().bounds(), rect("W10N10", "W0N0"));

    // `--world-size 10`: an odd reported size.
    let fake = Fake::new(11, rect("W9N9", "W0N0"));
    let (_server, overseer, _websocket) = connect(fake.routes()).await;
    let world = overseer.map().world(None).await.unwrap();
    assert_eq!(world.bounds(), rect("W9N9", "W0N0"));

    // Centred worlds (screeps.com): even and odd sizes.
    for (size, west, east) in [(12, "W5N5", "E5S5"), (11, "W5N5", "E4S4")] {
        let fake = Fake::new(size, rect(west, east));
        let (_server, overseer, _websocket) = connect(fake.routes()).await;
        let world = overseer.map().world(None).await.unwrap();
        assert_eq!(world.bounds(), rect(west, east), "size {size}");
    }
}

#[tokio::test]
async fn known_bounds_make_no_request() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let before = server.requests().len();
    let bounds = rect("W10N10", "W0N0");
    let mut world = overseer.map().world_with_bounds(None, bounds).unwrap();
    assert_eq!(world.bounds(), bounds);
    assert_eq!(world.shard(), None);
    assert_eq!(
        server.requests().len(),
        before,
        "no world-size or probe request"
    );

    // The given bounds clip the overviews like measured ones.
    world.set_interest(&Interest {
        stats: Some(rect("W20N20", "E5S5")),
        ..Interest::default()
    });
    let WorldEvent::Stats(stats) = event(&mut world).await else {
        panic!("overviews expected");
    };
    assert_eq!(stats.len(), 121);
    assert!(
        map_stats(&server)[0]
            .iter()
            .all(|name| bounds.contains(room(name)))
    );

    overseer.shutdown().await;
    assert!(matches!(
        overseer.map().world_with_bounds(None, bounds),
        Err(overseer::Error::Closed)
    ));
}
