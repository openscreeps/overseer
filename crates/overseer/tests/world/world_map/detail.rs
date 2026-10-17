//! Detail rooms: priority, refusals (stale ones behind barriers) and terrain reuse.

use std::collections::BTreeSet;
use std::sync::Arc;

use overseer::map::{DetailState, Interest, RoomCoord, WorldEvent, WorldMap};

use super::{Fake, event, frames, no_frame, quiet, rect, refuse, requested, room, update};
use crate::server::{self, Ws};
use crate::{connect, receive};

/// The next `count` detail events (skipping terrain and what live rooms show of their overviews
/// and portals).
async fn details(world: &mut WorldMap, count: usize) -> Vec<(RoomCoord, String)> {
    let mut details = Vec::new();
    while details.len() < count {
        match event(world).await {
            WorldEvent::Detail(room, state) => {
                let state = match state {
                    DetailState::Pending => "pending".to_owned(),
                    DetailState::Live(snapshot) => format!("live {}", snapshot.game_time.unwrap()),
                    DetailState::Refused(error) => format!("refused: {error}"),
                };
                details.push((room, state));
            }
            WorldEvent::DetailGone(room) => details.push((room, "gone".to_owned())),
            WorldEvent::Terrain(..) | WorldEvent::Stats(_) | WorldEvent::Portals(..) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    details
}

/// The next `count` frames, driving `next` meanwhile (detail rooms subscribe once their
/// terrain arrived); fails on events but terrain.
async fn subscribed(world: &mut WorldMap, websocket: &mut Ws, count: usize) -> Vec<String> {
    let frames = receive(websocket, count);
    tokio::pin!(frames);
    loop {
        tokio::select! {
            sent = &mut frames => return sent,
            event = world.next() => {
                let event = event.unwrap();
                assert!(
                    matches!(event, WorldEvent::Terrain(..) | WorldEvent::NoTerrain(..)),
                    "unexpected {event:?}"
                );
            }
        }
    }
}

/// Prefix of the detail barriers' channels.
const BARRIER: &str = "roomMap2:overseer-barrier-";

/// Receives the next frame, driving `next` meanwhile, and returns the barrier channel it
/// subscribes.
async fn barrier(world: &mut WorldMap, websocket: &mut Ws) -> String {
    let frame = subscribed(world, websocket, 1).await.remove(0);
    frame
        .strip_prefix("subscribe ")
        .filter(|channel| channel.starts_with(BARRIER))
        .unwrap_or_else(|| panic!("a barrier subscription expected, got {frame}"))
        .to_owned()
}

/// Answers `channel` like the server answers a new `roomMap2` subscription of a room that does
/// not exist (after processing every earlier command), and checks that it is released.
async fn answer(world: &mut WorldMap, websocket: &mut Ws, channel: &str) {
    server::send(websocket, &format!(r#"["{channel}",{{}}]"#)).await;
    assert_eq!(
        subscribed(world, websocket, 1).await,
        [format!("unsubscribe {channel}")]
    );
}

/// Receives the barrier subscribed next and answers it.
async fn pass_barrier(world: &mut WorldMap, websocket: &mut Ws) {
    let channel = barrier(world, websocket).await;
    answer(world, websocket, &channel).await;
}

/// Subscribes `W1N1`, `W2N2` and `W3N3` in that order, passes the barrier and delivers an update
/// of each at tick 9.
async fn three_live_rooms(world: &mut WorldMap, websocket: &mut Ws) -> [RoomCoord; 3] {
    let rooms = [room("W1N1"), room("W2N2"), room("W3N3")];
    world.set_interest(&Interest {
        detail: rooms.to_vec(),
        ..Interest::default()
    });
    let pending = rooms.map(|room| (room, "pending".to_owned()));
    assert_eq!(details(world, 3).await, pending);
    let sent = subscribed(world, websocket, 3).await;
    assert_eq!(
        sent.into_iter().collect::<BTreeSet<_>>(),
        frames("subscribe", "room", &rooms)
    );
    pass_barrier(world, websocket).await;
    for room in rooms {
        update(websocket, room, 9).await;
        assert_eq!(details(world, 1).await, [(room, "live 9".to_owned())]);
    }
    rooms
}

#[tokio::test]
async fn detail_follows_priority_and_retries_refusals_after_an_interest_change() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (_server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let (a, b, c) = (room("W1N1"), room("W2N2"), room("W3N3"));

    world.set_interest(&Interest {
        detail: vec![a, b, a, c],
        ..Interest::default()
    });
    let pending = [a, b, c].map(|room| (room, "pending".to_owned()));
    assert_eq!(details(&mut world, 3).await, pending);
    let sent = subscribed(&mut world, &mut websocket, 3).await;
    assert_eq!(
        sent.into_iter().collect::<BTreeSet<_>>(),
        frames("subscribe", "room", &[a, b, c])
    );
    pass_barrier(&mut world, &mut websocket).await;
    for room in [a, b, c] {
        update(&mut websocket, room, 9).await;
        assert_eq!(details(&mut world, 1).await, [(room, "live 9".to_owned())]);
    }

    // The server refuses one room at random (here the most important): the least important
    // one is released instead, and reported refused once.
    refuse(&mut websocket, a).await;
    let refused = (c, "refused: subscribe limit reached".to_owned());
    assert_eq!(details(&mut world, 1).await, std::slice::from_ref(&refused));
    assert_eq!(receive(&mut websocket, 1).await, ["unsubscribe room:W3N3"]);
    pass_barrier(&mut world, &mut websocket).await;
    assert!(matches!(world.detail(a), Some(DetailState::Live(_))));
    update(&mut websocket, a, 10).await;
    assert_eq!(details(&mut world, 1).await, [(a, "live 10".to_owned())]);

    // Another interest with the same detail rooms does not retry.
    world.set_interest(&Interest {
        detail: vec![a, b, c],
        terrain: vec![room("W30N30")],
        ..Interest::default()
    });
    quiet(&mut world).await;
    no_frame(&mut websocket).await;
    assert!(matches!(world.detail(c), Some(DetailState::Refused(_))));

    // A new order is a new interest: the refused room is tried again.
    world.set_interest(&Interest {
        detail: vec![b, a, c],
        ..Interest::default()
    });
    assert_eq!(details(&mut world, 1).await, [(c, "pending".to_owned())]);
    assert_eq!(
        subscribed(&mut world, &mut websocket, 1).await,
        ["subscribe room:W3N3"]
    );
    pass_barrier(&mut world, &mut websocket).await;
    refuse(&mut websocket, c).await;
    assert_eq!(details(&mut world, 1).await, [refused]);
    assert_eq!(receive(&mut websocket, 1).await, ["unsubscribe room:W3N3"]);
    pass_barrier(&mut world, &mut websocket).await;

    // Rooms that leave the interest are released.
    world.set_interest(&Interest {
        detail: vec![a],
        ..Interest::default()
    });
    assert_eq!(
        details(&mut world, 2).await,
        [(b, "gone".to_owned()), (c, "gone".to_owned())]
    );
    assert_eq!(receive(&mut websocket, 1).await, ["unsubscribe room:W2N2"]);
    pass_barrier(&mut world, &mut websocket).await;
    assert!(world.detail(b).is_none());
    no_frame(&mut websocket).await;
}

#[tokio::test]
async fn detail_ignores_refusals_the_server_sends_before_processing_a_release() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (_server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let [a, b, c] = three_live_rooms(&mut world, &mut websocket).await;

    // The server refuses one room at random: the least important one is released.
    refuse(&mut websocket, a).await;
    let refused = (c, "refused: subscribe limit reached".to_owned());
    assert_eq!(details(&mut world, 1).await, std::slice::from_ref(&refused));
    assert_eq!(receive(&mut websocket, 1).await, ["unsubscribe room:W3N3"]);
    let barrier = barrier(&mut world, &mut websocket).await;

    // Until it processes the release, the server still counts three rooms: on each update it
    // refuses one of them at random and serves the others. Those refusals release nothing.
    for (time, skipped) in [(10, b), (11, a), (12, c), (13, b)] {
        refuse(&mut websocket, skipped).await;
        for served in [a, b].into_iter().filter(|&room| room != skipped) {
            update(&mut websocket, served, time).await;
            assert_eq!(
                details(&mut world, 1).await,
                [(served, format!("live {time}"))]
            );
        }
    }
    assert!(matches!(world.detail(a), Some(DetailState::Live(_))));
    assert!(matches!(world.detail(b), Some(DetailState::Live(_))));
    assert!(matches!(world.detail(c), Some(DetailState::Refused(_))));

    // Then it answers the barrier and serves both remaining rooms.
    answer(&mut world, &mut websocket, &barrier).await;
    for room in [a, b] {
        update(&mut websocket, room, 14).await;
        assert_eq!(details(&mut world, 1).await, [(room, "live 14".to_owned())]);
    }
    no_frame(&mut websocket).await;
}

#[tokio::test]
async fn detail_ignores_a_refusal_crossing_a_new_subscription_of_the_room() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (_server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let [a, b, c] = three_live_rooms(&mut world, &mut websocket).await;
    refuse(&mut websocket, b).await;
    assert_eq!(
        details(&mut world, 1).await,
        [(c, "refused: subscribe limit reached".to_owned())]
    );
    assert_eq!(receive(&mut websocket, 1).await, ["unsubscribe room:W3N3"]);
    let first = barrier(&mut world, &mut websocket).await;

    // Before the server processed the release, the interest drops the second room and wants
    // the third again: it is subscribed again behind a new barrier.
    world.set_interest(&Interest {
        detail: vec![a, c],
        ..Interest::default()
    });
    assert_eq!(
        details(&mut world, 2).await,
        [(b, "gone".to_owned()), (c, "pending".to_owned())]
    );
    assert_eq!(
        subscribed(&mut world, &mut websocket, 3).await,
        [
            "unsubscribe room:W2N2".to_owned(),
            "subscribe room:W3N3".to_owned(),
            format!("unsubscribe {first}"),
        ]
    );
    let second = barrier(&mut world, &mut websocket).await;

    // Refusals of the third room (and of the first) sent before the server processed those
    // commands arrive after the new subscription: they neither refuse it nor release a room.
    refuse(&mut websocket, c).await;
    refuse(&mut websocket, a).await;
    update(&mut websocket, a, 10).await;
    assert_eq!(details(&mut world, 1).await, [(a, "live 10".to_owned())]);
    // The server answers the first barrier (released meanwhile) and the second, then serves
    // both rooms.
    server::send(&mut websocket, &format!(r#"["{first}",{{}}]"#)).await;
    answer(&mut world, &mut websocket, &second).await;
    for room in [a, c] {
        update(&mut websocket, room, 11).await;
        assert_eq!(details(&mut world, 1).await, [(room, "live 11".to_owned())]);
    }
    no_frame(&mut websocket).await;
}

#[tokio::test]
async fn detail_reuses_the_world_terrain() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let (seeded, fetched, unknown) = (room("W5N5"), room("W6N6"), room("W30N30"));
    let terrain = overseer.map().terrain(["W0N0"], None).await.unwrap()["W0N0"].clone();
    world.insert_terrain(seeded, Arc::clone(&terrain));
    let before = requested(&server, "/api/game/rooms").len();

    // A seeded room subscribes at once, with the seeded terrain and without any request.
    world.set_interest(&Interest {
        detail: vec![seeded],
        ..Interest::default()
    });
    assert_eq!(
        details(&mut world, 1).await,
        [(seeded, "pending".to_owned())]
    );
    assert_eq!(
        subscribed(&mut world, &mut websocket, 1).await,
        ["subscribe room:W5N5"]
    );
    pass_barrier(&mut world, &mut websocket).await;
    update(&mut websocket, seeded, 9).await;
    let WorldEvent::Detail(_, DetailState::Live(snapshot)) = event(&mut world).await else {
        panic!("a live update expected");
    };
    assert!(Arc::ptr_eq(&snapshot.terrain, &terrain));
    assert_eq!(requested(&server, "/api/game/rooms").len(), before);

    // Other rooms' terrain comes with the world's terrain batch, ahead of the terrain wanted;
    // a room the server does not know is refused.
    world.set_interest(&Interest {
        detail: vec![seeded, fetched, unknown],
        terrain: vec![room("W1N1")],
        ..Interest::default()
    });
    let mut states = Vec::new();
    let mut terrain_events = Vec::new();
    let mut absent = Vec::new();
    while states.len() < 3 {
        match event(&mut world).await {
            WorldEvent::Detail(room, DetailState::Pending) => states.push((room, "pending")),
            WorldEvent::Detail(room, DetailState::Refused(error)) => {
                assert_eq!(error, "the server has no room W30N30");
                states.push((room, "refused"));
            }
            WorldEvent::Terrain(room, _) => terrain_events.push(room),
            WorldEvent::NoTerrain(room) => absent.push(room),
            // What the seeded room's live update showed.
            WorldEvent::Stats(overviews) => assert_eq!(overviews[0].0, seeded),
            WorldEvent::Portals(room, _) => assert_eq!(room, seeded),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(absent, [unknown]);
    assert_eq!(
        states,
        [
            (fetched, "pending"),
            (unknown, "pending"),
            (unknown, "refused")
        ]
    );
    assert_eq!(
        subscribed(&mut world, &mut websocket, 1).await,
        ["subscribe room:W6N6"]
    );
    // The viewed rooms and the wanted one go in one batch with the prefetch ring (the
    // background sweep may run alongside; the client sends names sorted); no room twice.
    let batches = requested(&server, "/api/game/rooms");
    let foreground = batches[before..]
        .iter()
        .find(|batch| batch.iter().any(|name| name == "W6N6"))
        .unwrap();
    assert!(
        ["W30N30", "W1N1"]
            .iter()
            .all(|name| foreground.iter().any(|sent| sent == name))
    );
    let all = batches[before..].iter().flatten().collect::<Vec<_>>();
    assert_eq!(all.iter().collect::<BTreeSet<_>>().len(), all.len());
    assert!(
        server
            .requests()
            .iter()
            .all(|request| request.path != "/api/game/room-terrain"),
        "no room-terrain request"
    );
    assert!(matches!(world.detail(fetched), Some(DetailState::Pending)));
    assert!(world.terrain(fetched).is_some());
}
