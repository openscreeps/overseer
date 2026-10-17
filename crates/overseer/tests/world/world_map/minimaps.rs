//! Minimaps and scans: the minimap cap and release, and scans held behind a detail room until
//! its first update or its deadline.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use overseer::map::{
    DETAIL_FIRST, DetailState, HELD_ROOM_MAPS, Interest, MINIMAP_CAP, RoomCoord, RoomRect,
    WorldEvent, WorldMap,
};

use super::{Fake, event, frames, no_frame, rect, refuse, room, rooms_of, update};
use crate::server::{self, Ws};
use crate::{PATIENCE, connect, receive};

#[tokio::test]
async fn minimaps_are_capped_nearest_first_and_released() {
    let fake = Fake::new(40, rect("W19N19", "E19S19"));
    let (_server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();

    let rooms = room("W0N0").around(9).collect::<Vec<_>>();
    assert!(rooms.len() > MINIMAP_CAP);
    let interest = Interest {
        minimaps: rooms.clone(),
        ..Interest::default()
    };
    world.set_interest(&interest);
    let sent = receive(&mut websocket, MINIMAP_CAP).await;
    assert_eq!(
        sent.into_iter().collect::<BTreeSet<_>>(),
        frames("subscribe", "roomMap2", &rooms[..MINIMAP_CAP])
    );
    no_frame(&mut websocket).await;
    // The same interest changes nothing.
    world.set_interest(&interest.clone());
    no_frame(&mut websocket).await;

    let first = rooms[0];
    server::send(
        &mut websocket,
        &format!(r#"["roomMap2:{first}",{{"s":[[10,20]]}}]"#),
    )
    .await;
    let WorldEvent::Minimap(room, map) = event(&mut world).await else {
        panic!("a minimap expected");
    };
    assert_eq!((room, map.sources().len()), (first, 1));
    assert!(world.minimap(first).is_some());

    // Moving by one room releases one and subscribes one.
    world.set_interest(&Interest {
        minimaps: rooms[1..].to_vec(),
        ..Interest::default()
    });
    let sent = receive(&mut websocket, 2).await;
    let mut expected = frames("unsubscribe", "roomMap2", [&first]);
    expected.extend(frames("subscribe", "roomMap2", [&rooms[MINIMAP_CAP]]));
    assert_eq!(sent.into_iter().collect::<BTreeSet<_>>(), expected);
    assert!(matches!(event(&mut world).await, WorldEvent::MinimapGone(room) if room == first));
    assert!(world.minimap(first).is_none());
}

/// The frames the client sends within 300 ms (real time; paused time stays put), driving
/// `next` meanwhile (any event).
async fn sent_for(world: &mut WorldMap, websocket: &mut Ws) -> Vec<String> {
    let until = std::time::Instant::now() + Duration::from_millis(300);
    let mut sent = Vec::new();
    while std::time::Instant::now() < until {
        tokio::select! {
            biased;
            frame = server::receive(websocket, 1) => sent.extend(frame),
            event = world.next() => drop(event.unwrap()),
            () = tokio::task::yield_now() => {}
        }
    }
    sent
}

/// The rooms of the `roomMap2` subscriptions among `frames` but the detail barriers'.
fn scans(frames: &[String]) -> Vec<RoomCoord> {
    frames
        .iter()
        .filter_map(|frame| frame.strip_prefix("subscribe roomMap2:"))
        .filter(|channel| !channel.starts_with("overseer-barrier-"))
        .map(room)
        .collect()
}

/// The rooms in view in [`detail_with_scans`] and the prefetch ring around them.
fn in_view_and_ring() -> RoomRect {
    rect("W10N10", "E0S0")
}

/// The interest of [`detail_with_scans`]: `W5N5` wanted in detail, every room's portals
/// (a few in view).
fn detail_interest() -> Interest {
    Interest {
        detail: vec![room("W5N5")],
        terrain: vec![room("W5N5")],
        portals: Some(rect("W7N7", "W3N3")),
        ..Interest::default()
    }
}

/// A world of 1 600 rooms whose portals are all wanted, with `W5N5` wanted in detail and its
/// terrain not known yet: the client's first frames.
async fn detail_with_scans() -> (WorldMap, Ws, Vec<String>) {
    let fake = Fake::new(40, rect("W19N19", "E19S19"));
    let (_server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    world.set_interest(&detail_interest());
    let sent = sent_for(&mut world, &mut websocket).await;
    (world, websocket, sent)
}

#[tokio::test]
async fn a_detail_room_subscribes_first_and_holds_scans_until_its_first_update() {
    let (mut world, mut websocket, sent) = detail_with_scans().await;
    // The room first, without waiting for its terrain, then its barrier; then a few scans
    // of the rooms in view (and the ring around them), the background waiting.
    assert_eq!(sent[0], "subscribe room:W5N5");
    assert!(
        sent[1].starts_with("subscribe roomMap2:overseer-barrier-"),
        "{sent:?}"
    );
    let held = scans(&sent);
    assert_eq!(held.len(), HELD_ROOM_MAPS, "{sent:?}");
    assert!(held.iter().all(|&scan| in_view_and_ring().contains(scan)));
    assert!(matches!(
        world.detail(room("W5N5")),
        Some(DetailState::Pending)
    ));

    // Its first update: live (with the terrain that arrived meanwhile), and the scans go on,
    // the background's too.
    update(&mut websocket, room("W5N5"), 9).await;
    let mut more = Vec::new();
    while !matches!(world.detail(room("W5N5")), Some(DetailState::Live(_))) {
        more.extend(sent_for(&mut world, &mut websocket).await);
    }
    more.extend(sent_for(&mut world, &mut websocket).await);
    let resumed = scans(&more);
    assert!(resumed.len() > 100, "{} scans", resumed.len());
    assert!(
        resumed
            .iter()
            .any(|&scan| !in_view_and_ring().contains(scan))
    );
}

#[tokio::test]
async fn a_refused_detail_room_releases_the_scans() {
    let (mut world, mut websocket, sent) = detail_with_scans().await;
    assert_eq!(scans(&sent).len(), HELD_ROOM_MAPS);
    let barrier = sent[1].strip_prefix("subscribe ").unwrap().to_owned();
    server::send(&mut websocket, &format!(r#"["{barrier}",{{}}]"#)).await;
    // The server refuses the room on every update until it is released.
    let mut more = Vec::new();
    while !matches!(world.detail(room("W5N5")), Some(DetailState::Refused(_))) {
        refuse(&mut websocket, room("W5N5")).await;
        more.extend(sent_for(&mut world, &mut websocket).await);
    }
    more.extend(sent_for(&mut world, &mut websocket).await);
    assert!(scans(&more).len() > 100, "{more:?}");
}

#[tokio::test]
async fn held_scans_go_on_after_the_detail_deadline() {
    let fake = Fake::new(40, rect("W19N19", "E19S19"));
    let (_server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    world.set_interest(&detail_interest());
    let mut sent = Vec::new();
    while scans(&sent).len() < HELD_ROOM_MAPS {
        sent.extend(sent_for(&mut world, &mut websocket).await);
    }
    // The server never answers the room: the scans wait until the deadline, no longer.
    tokio::time::advance(DETAIL_FIRST.saturating_sub(Duration::from_millis(100))).await;
    sent.extend(sent_for(&mut world, &mut websocket).await);
    assert_eq!(scans(&sent).len(), HELD_ROOM_MAPS);
    tokio::time::advance(Duration::from_millis(101)).await;
    sent.extend(sent_for(&mut world, &mut websocket).await);
    assert!(scans(&sent).len() > 100, "{} scans", scans(&sent).len());
    assert!(matches!(
        world.detail(room("W5N5")),
        Some(DetailState::Pending)
    ));
}

#[tokio::test]
async fn a_room_updated_before_its_terrain_arrives_shows_once_it_does() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    server.hold(Some("/api/game/rooms"));
    world.set_interest(&Interest {
        detail: vec![room("W5N5")],
        terrain: vec![room("W5N5")],
        ..Interest::default()
    });
    let sent = sent_for(&mut world, &mut websocket).await;
    assert_eq!(sent[0], "subscribe room:W5N5");
    // The server's first update comes before the terrain: kept, nothing shown yet.
    update(&mut websocket, room("W5N5"), 9).await;
    sent_for(&mut world, &mut websocket).await;
    assert!(matches!(
        world.detail(room("W5N5")),
        Some(DetailState::Pending)
    ));
    // The terrain arrives: the update shows at once, with it.
    server.hold(None);
    let start = std::time::Instant::now();
    while !matches!(world.detail(room("W5N5")), Some(DetailState::Live(_))) {
        assert!(start.elapsed() < PATIENCE, "live in time");
        sent_for(&mut world, &mut websocket).await;
    }
    let Some(DetailState::Live(snapshot)) = world.detail(room("W5N5")) else {
        unreachable!();
    };
    assert_eq!(snapshot.game_time, Some(9));
    assert!(Arc::ptr_eq(
        &snapshot.terrain,
        &world.terrain(room("W5N5")).unwrap()
    ));
}

#[tokio::test]
async fn a_detail_room_goes_ahead_of_the_minimaps_until_its_first_update() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (_server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let minimaps = rect("W9N9", "W1N1").iter().collect::<Vec<_>>();
    world.set_interest(&Interest {
        detail: vec![room("W5N5")],
        terrain: vec![room("W5N5")],
        minimaps: minimaps.clone(),
        ..Interest::default()
    });
    let sent = sent_for(&mut world, &mut websocket).await;
    // The room first; then only the nearest minimaps while it waits.
    assert_eq!(sent[0], "subscribe room:W5N5");
    assert_eq!(
        rooms_of(&scans(&sent)),
        rooms_of(&minimaps[..HELD_ROOM_MAPS])
    );
    update(&mut websocket, room("W5N5"), 9).await;
    let mut more = Vec::new();
    while !matches!(world.detail(room("W5N5")), Some(DetailState::Live(_))) {
        more.extend(sent_for(&mut world, &mut websocket).await);
    }
    more.extend(sent_for(&mut world, &mut websocket).await);
    // Then all of them.
    assert_eq!(scans(&more).len(), minimaps.len() - HELD_ROOM_MAPS);
    assert_eq!(
        rooms_of(&scans(&more)),
        rooms_of(&minimaps[HELD_ROOM_MAPS..])
    );
}

#[tokio::test]
async fn live_minimaps_stay_while_another_room_waits_for_its_first_update() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (_server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let minimaps = rect("W9N9", "W1N1").iter().collect::<Vec<_>>();
    let viewing = |room_name: &str| Interest {
        detail: vec![room(room_name)],
        terrain: vec![room(room_name)],
        minimaps: minimaps.clone(),
        ..Interest::default()
    };
    world.set_interest(&viewing("W5N5"));
    sent_for(&mut world, &mut websocket).await;
    update(&mut websocket, room("W5N5"), 9).await;
    while !matches!(world.detail(room("W5N5")), Some(DetailState::Live(_))) {
        sent_for(&mut world, &mut websocket).await;
    }
    sent_for(&mut world, &mut websocket).await;

    // A neighbour is opened: it waits for its first update, and the minimaps around it, all
    // live already, stay live meanwhile.
    world.set_interest(&viewing("W6N5"));
    let sent = sent_for(&mut world, &mut websocket).await;
    assert!(sent.contains(&"subscribe room:W6N5".to_owned()), "{sent:?}");
    let dropped: Vec<&String> = sent
        .iter()
        .filter(|frame| frame.starts_with("unsubscribe roomMap2:W"))
        .collect();
    assert!(dropped.is_empty(), "live minimaps dropped: {dropped:?}");
}
