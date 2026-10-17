//! Overview requests in flight: the cap, the budget they spend, their order and a 429.

use std::time::Duration;

use overseer::map::{Interest, STATS_BATCH_SECTORS};

use super::{
    Fake, answer_pings, drive_for, drive_until, lock, map_stats, names_of, rate_limit, rect, room,
    rooms_of,
};
use crate::connect;

const MAP_STATS: &str = "/api/game/map-stats";

#[tokio::test]
async fn overviews_go_three_at_a_time_once_the_limit_is_known_on_servers_with_rate_limits() {
    // 160 × 160 rooms, all far: six requests of the nearest sectors.
    let fake = Fake::limited(160, rect("W79N79", "E79S79"));
    lock(&fake.scripted).push_back((200, rate_limit(60, 59, Duration::from_hours(1))));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    server.hold(Some(MAP_STATS));
    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    // Before the endpoint's first response nothing tells how many requests are left: one.
    drive_until(&mut world, |_| map_stats(&server).len() == 1).await;
    drive_for(&mut world).await;
    assert_eq!(map_stats(&server).len(), 1);
    // 59 left: three in flight, no more.
    server.release();
    drive_until(&mut world, |_| map_stats(&server).len() == 4).await;
    drive_for(&mut world).await;
    assert_eq!(map_stats(&server).len(), 4);
    server.release();
    drive_until(&mut world, |_| map_stats(&server).len() == 6).await;
    server.hold(None);
    drive_until(&mut world, |world| world.progress().is_complete()).await;
    let sizes = map_stats(&server).iter().map(Vec::len).collect::<Vec<_>>();
    assert_eq!(sizes.iter().sum::<usize>(), 25_600);
    assert_eq!(sizes.len(), 6);
}

#[tokio::test]
async fn requests_in_flight_count_as_spent() {
    let fake = Fake::limited(160, rect("W79N79", "E79S79"));
    lock(&fake.scripted).extend([
        // 31 of 60 left.
        (200, rate_limit(60, 31, Duration::from_mins(30))),
        (200, rate_limit(60, 29, Duration::from_mins(30))),
        (200, rate_limit(60, 28, Duration::from_mins(30))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    server.hold(Some(MAP_STATS));
    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    drive_until(&mut world, |_| map_stats(&server).len() == 1).await;
    server.release();
    // With 31 left, the second request in flight leaves 29: fewer than half the window, so
    // the budget spreads the rest and no third request goes out.
    drive_until(&mut world, |_| map_stats(&server).len() == 3).await;
    drive_for(&mut world).await;
    assert_eq!(map_stats(&server).len(), 3);
    // Their responses (28 left) spread the next requests over the 30 minutes left: about a
    // minute apart.
    server.hold(None);
    drive_until(&mut world, |world| {
        world.progress().stats_known == 3 * STATS_BATCH_SECTORS * 100
    })
    .await;
    drive_for(&mut world).await;
    assert_eq!(map_stats(&server).len(), 3);
    tokio::time::advance(Duration::from_secs(70)).await;
    drive_until(&mut world, |_| map_stats(&server).len() == 4).await;
    drive_for(&mut world).await;
    assert_eq!(map_stats(&server).len(), 4);
}

#[tokio::test]
async fn the_first_round_brings_the_viewed_then_the_near_then_the_visible_rooms() {
    let fake = Fake::new(160, rect("W79N79", "E79S79"));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    server.hold(Some(MAP_STATS));
    let visible = rect("E10S10", "E29S29");
    world.set_interest(&Interest {
        stats: Some(visible),
        minimaps: vec![room("W75N75")],
        detail: vec![room("E70S70")],
        terrain: vec![room("E20S20")],
        ..Interest::default()
    });
    drive_until(&mut world, |_| map_stats(&server).len() == 3).await;
    drive_for(&mut world).await;
    let first = map_stats(&server);
    assert_eq!(first.len(), 3);
    // The viewed room leads a request with the near rooms (around the minimap), then the
    // rooms in view; the other two take the nearest far sectors.
    let near = rooms_of(&rect("W79N79", "W60N60").iter().collect::<Vec<_>>());
    let in_view = rooms_of(&visible.iter().collect::<Vec<_>>());
    let viewed = first
        .iter()
        .find(|request| request[0] == "E70S70")
        .expect("the viewed room");
    let viewed = names_of(viewed);
    assert!(near.is_subset(&viewed) && in_view.is_subset(&viewed));
    assert_eq!(viewed.len(), 1 + near.len() + in_view.len());
    server.hold(None);
    drive_until(&mut world, |world| world.progress().is_complete()).await;
}

#[tokio::test]
async fn a_429_among_requests_in_flight_holds_the_next_ones_until_its_reset() {
    let fake = Fake::limited(160, rect("W79N79", "E79S79"));
    lock(&fake.scripted).extend([
        (200, rate_limit(60, 59, Duration::from_hours(1))),
        // One of the three in flight is refused until the reset 120 s later (beyond the
        // client's own 60 s); the others go through.
        (429, rate_limit(60, 0, Duration::from_secs(120))),
        (200, rate_limit(60, 57, Duration::from_hours(1))),
        (200, rate_limit(60, 56, Duration::from_hours(1))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    server.hold(Some(MAP_STATS));
    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    drive_until(&mut world, |_| map_stats(&server).len() == 1).await;
    server.release();
    drive_until(&mut world, |_| map_stats(&server).len() == 4).await;
    server.hold(None);
    // The first request and two of the three in flight.
    drive_until(&mut world, |world| {
        world.progress().stats_known == 3 * STATS_BATCH_SECTORS * 100
    })
    .await;
    // Nothing more until the reset, whatever the other responses said.
    drive_for(&mut world).await;
    tokio::time::advance(Duration::from_secs(60)).await;
    drive_for(&mut world).await;
    assert_eq!(map_stats(&server).len(), 4);
    tokio::time::advance(Duration::from_secs(65)).await;
    drive_until(&mut world, |world| world.progress().is_complete()).await;
    // The refused rooms and the rest: three more requests.
    assert_eq!(map_stats(&server).len(), 7);
}
