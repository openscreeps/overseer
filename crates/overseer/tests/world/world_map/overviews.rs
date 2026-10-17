//! Overview lifetimes and batches: near and far refresh, seeded overviews, capped batches.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use overseer::map::{
    Interest, RoomStat, RoomStatus, STATS_BATCH_SECTORS, STATS_FAR_TTL, STATS_TTL, WorldEvent,
};

use super::{
    Fake, advance, answer_pings, epoch_millis, event, map_stats, names_of, quiet, rect, room,
    rooms_of, stats,
};
use crate::connect;

#[tokio::test]
async fn near_overviews_refresh_after_10_minutes_and_far_ones_after_an_hour() {
    // 40 × 40 rooms: 16 sectors.
    let fake = Fake::limited(40, rect("W19N19", "E19S19"));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    let bounds = world.bounds();
    // A minimap in the sector W10N10..W19N19: it and the sectors around it are near (inside
    // the world: 4 sectors).
    let near = rect("W19N19", "W0N0");
    let far = bounds
        .iter()
        .filter(|&room| !near.contains(room))
        .collect::<BTreeSet<_>>();

    // A view over the world's corner (the near rooms): every room of the world is wanted.
    world.set_interest(&Interest {
        stats: Some(rect("W30N30", "W0N0")),
        minimaps: vec![room("W15N15")],
        ..Interest::default()
    });
    assert_eq!(world.progress().stats_wanted, 1600);
    let first = stats(&mut world).await;
    assert_eq!(
        rooms_of(first.iter().map(|(room, _)| room)),
        rooms_of(&near.iter().collect::<Vec<_>>())
    );
    // Nearest the minimap first.
    assert_eq!(first[0].0, room("W15N15"));
    let before = epoch_millis();
    assert!(
        first
            .iter()
            .all(|(_, stat)| stat.fetched_at <= before && stat.fetched_at + 60_000 > before)
    );
    let owner = world.stat(room("W1N1")).unwrap();
    assert_eq!(owner.owner.as_ref().unwrap().level, 3);

    // Then the far rooms, at once.
    let second = stats(&mut world).await;
    assert_eq!(rooms_of(second.iter().map(|(room, _)| room)), far);
    let progress = world.progress();
    assert_eq!((progress.stats_known, progress.stats_wanted), (1600, 1600));
    assert!(progress.is_complete());

    // Near rooms are asked again after 10 minutes, all reported (with a new `fetched_at`),
    // changes included.
    advance(
        &mut world,
        STATS_TTL.saturating_sub(Duration::from_secs(60)),
    )
    .await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);
    fake.level.store(4, Ordering::SeqCst);
    advance(&mut world, Duration::from_secs(61)).await;
    let third = stats(&mut world).await;
    assert_eq!(map_stats(&server).len(), 3);
    assert_eq!(
        names_of(&map_stats(&server)[2]),
        rooms_of(&near.iter().collect::<Vec<_>>())
    );
    assert_eq!(third.len(), 400);
    assert_eq!(
        world
            .stat(room("W1N1"))
            .unwrap()
            .owner
            .as_ref()
            .unwrap()
            .level,
        4
    );

    // Without minimaps every room is far: nothing until the far rooms' hour is up.
    world.set_interest(&Interest {
        stats: Some(bounds),
        ..Interest::default()
    });
    advance(
        &mut world,
        STATS_FAR_TTL.saturating_sub(STATS_TTL + Duration::from_secs(120)),
    )
    .await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 3);
    advance(&mut world, Duration::from_secs(120)).await;
    let fourth = stats(&mut world).await;
    assert_eq!(rooms_of(fourth.iter().map(|(room, _)| room)), far);
    assert_eq!(map_stats(&server).len(), 4);
}

#[tokio::test]
async fn seeded_overviews_are_asked_again_once_stale() {
    let fake = Fake::new(40, rect("W19N19", "E19S19"));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    let bounds = world.bounds();
    let near = rect("W19N19", "W0N0");

    // Every room cached half an hour ago: far rooms are fresh for another half hour.
    let half_hour_ago = epoch_millis() - 30 * 60 * 1000;
    let cached = |fetched_at| RoomStat {
        status: RoomStatus::Normal,
        status_until: None,
        owner: None,
        reservation: None,
        safe_mode: false,
        mineral: Some("H".to_owned()),
        mineral_density: None,
        fetched_at,
    };
    for room in bounds.iter() {
        world.insert_stats(room, Arc::new(cached(half_hour_ago)));
    }
    // An older one does not replace it.
    world.insert_stats(room("E5S5"), Arc::new(cached(half_hour_ago - 1)));
    assert_eq!(world.stat(room("E5S5")).unwrap().fetched_at, half_hour_ago);
    let progress = world.progress();
    assert_eq!((progress.stats_known, progress.stats_wanted), (1600, 0));

    // Only the stale near rooms are asked for.
    world.set_interest(&Interest {
        stats: Some(bounds),
        minimaps: vec![room("W15N15")],
        ..Interest::default()
    });
    assert!(world.progress().is_complete());
    let refreshed = stats(&mut world).await;
    assert_eq!(refreshed.len(), 400);
    advance(&mut world, Duration::from_secs(2)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 1);
    assert_eq!(
        names_of(&map_stats(&server)[0]),
        rooms_of(&near.iter().collect::<Vec<_>>())
    );
    assert_eq!(world.stat(room("E5S5")).unwrap().fetched_at, half_hour_ago);

    // The far ones when their hour is up.
    world.set_interest(&Interest {
        stats: Some(bounds),
        ..Interest::default()
    });
    advance(&mut world, Duration::from_mins(28)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 1);
    advance(&mut world, Duration::from_mins(2)).await;
    let far = stats(&mut world).await;
    assert_eq!(far.len(), 1200);
    assert!(far.iter().all(|(room, _)| !near.contains(*room)));
    assert!(world.stat(room("E5S5")).unwrap().fetched_at > half_hour_ago);
}

#[tokio::test]
async fn big_views_fill_in_capped_batches_of_the_nearest_sectors() {
    // 80 × 80 rooms: 64 sectors.
    let fake = Fake::limited(80, rect("W39N39", "E39S39"));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    let mut received = 0;
    for batch in 0..2 {
        let WorldEvent::Stats(stats) = event(&mut world).await else {
            panic!("overviews expected");
        };
        received += stats.len();
        assert_eq!(map_stats(&server).len(), batch + 1);
    }
    assert_eq!(received, 6400);
    let sizes = map_stats(&server).iter().map(Vec::len).collect::<Vec<_>>();
    assert_eq!(sizes, [5000, 1400]);
    // The first batch holds whole sectors around the centre (W0N0).
    let first = &map_stats(&server)[0];
    assert_eq!(first[0], "W0N0");
    let sectors = first
        .iter()
        .map(|name| {
            let room = room(name);
            (room.x.div_euclid(10), room.y.div_euclid(10))
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(sectors.len(), STATS_BATCH_SECTORS);
    assert!(
        sectors
            .iter()
            .all(|&(x, y)| (-4..=3).contains(&x) && (-4..=3).contains(&y))
    );
    assert!(sectors.contains(&(-1, -1)) && sectors.contains(&(0, 0)));
}
