//! The world map: bounds, interest diffing, terrain batching and seeding, overview lifetime and
//! budget, minimap cap, detail priority and refusals, cancel safety.
//!
//! Tests on paused time drive [`WorldMap::next`] by yielding (see [`event`]): a runtime that
//! never idles never advances paused time on its own, so only `tokio::time::advance` moves it.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{FutureExt, StreamExt};
use overseer::map::{
    DetailState, Interest, MINIMAP_CAP, RoomCoord, RoomRect, STATS_BATCH_SECTORS, STATS_TTL,
    WorldEvent, WorldMap,
};
use serde_json::{Value, json};

use crate::server::{self, Request, Response, Server, Ws};
use crate::{PATIENCE, connect, query, receive, terrain_document};

fn room(name: &str) -> RoomCoord {
    name.parse().unwrap()
}

fn rect(a: &str, b: &str) -> RoomRect {
    RoomRect::new(room(a), room(b))
}

/// Locks `mutex`, ignoring poisoning (like the server harness).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The status and headers of a scripted map-stats reply.
type Scripted = (u16, Vec<(&'static str, String)>);

/// A server whose world reports `size` and holds the rooms of `rooms`; room `W1N1` is owned by
/// `u1` (Alice) at `level`; map-stats replies take their status and headers from `scripted`
/// while it has entries.
#[derive(Clone)]
struct Fake {
    size: i64,
    rooms: RoomRect,
    level: Arc<AtomicI64>,
    scripted: Arc<Mutex<VecDeque<Scripted>>>,
}

impl Fake {
    fn new(size: i64, rooms: RoomRect) -> Self {
        Self {
            size,
            rooms,
            level: Arc::new(AtomicI64::new(3)),
            scripted: Arc::default(),
        }
    }

    fn routes(&self) -> impl Fn(&Request) -> Response + Send + Sync + 'static {
        let fake = self.clone();
        move |request| fake.reply(request)
    }

    /// The requested rooms (`rooms` of the JSON body) that exist.
    fn existing(&self, request: &Request) -> Vec<String> {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        body["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| self.rooms.contains(room(name)))
            .map(str::to_owned)
            .collect()
    }

    fn reply(&self, request: &Request) -> Response {
        match request.path.as_str() {
            "/api/game/world-size" => (
                200,
                format!(r#"{{"ok":1,"width":{0},"height":{0}}}"#, self.size),
            )
                .into(),
            "/api/game/rooms" => {
                let documents = self
                    .existing(request)
                    .iter()
                    .map(|name| terrain_document(name))
                    .collect::<Vec<_>>();
                (
                    200,
                    format!(r#"{{"ok":1,"rooms":[{}]}}"#, documents.join(",")),
                )
                    .into()
            }
            "/api/game/room-terrain" => {
                let name = query(request, "room").unwrap_or_default();
                (
                    200,
                    format!(r#"{{"ok":1,"terrain":[{}]}}"#, terrain_document(name)),
                )
                    .into()
            }
            "/api/game/map-stats" => {
                let level = self.level.load(Ordering::SeqCst);
                let entries = self
                    .existing(request)
                    .iter()
                    .map(|name| match name.as_str() {
                        "W1N1" => format!(
                            r#""W1N1":{{"status":"normal","own":{{"user":"u1","level":{level}}}}}"#
                        ),
                        name => format!(r#""{name}":{{"status":"normal"}}"#),
                    })
                    .collect::<Vec<_>>();
                let body = format!(
                    r#"{{"ok":1,"gameTime":7,"stats":{{{}}},"users":{{"u1":{{"_id":"u1","username":"Alice"}}}}}}"#,
                    entries.join(",")
                );
                let scripted = lock(&self.scripted).pop_front();
                let (status, headers) = scripted.unwrap_or((200, Vec::new()));
                let body = if status == 429 {
                    "Too Many Requests".to_owned()
                } else {
                    body
                };
                Response {
                    status,
                    headers,
                    body: body.into_bytes(),
                }
            }
            _ => (404, r#"{"error":"not found"}"#.to_owned()).into(),
        }
    }
}

/// `X-RateLimit-*` headers: `limit` per window, `remaining` left, reset `reset_in` from now.
fn rate_limit(limit: u32, remaining: u32, reset_in: Duration) -> Vec<(&'static str, String)> {
    let reset = (SystemTime::now() + reset_in)
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    vec![
        ("X-RateLimit-Limit", limit.to_string()),
        ("X-RateLimit-Remaining", remaining.to_string()),
        ("X-RateLimit-Reset", reset.to_string()),
    ]
}

/// The rooms of each request to `path` served so far.
fn requested(server: &Server, path: &str) -> Vec<Vec<String>> {
    server
        .requests()
        .iter()
        .filter(|request| request.path == path)
        .map(|request| {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            body["rooms"]
                .as_array()
                .unwrap()
                .iter()
                .map(|name| name.as_str().unwrap().to_owned())
                .collect()
        })
        .collect()
}

fn map_stats(server: &Server) -> Vec<Vec<String>> {
    requested(server, "/api/game/map-stats")
}

/// The next event, driving `next` without letting the runtime idle (paused time stays put);
/// every poll cancels `next` and starts it again.
async fn event(world: &mut WorldMap) -> WorldEvent {
    let start = std::time::Instant::now();
    loop {
        assert!(start.elapsed() < PATIENCE, "an event in time");
        tokio::select! {
            biased;
            event = world.next() => return event.unwrap(),
            () = tokio::task::yield_now() => {}
        }
    }
}

/// Drives `next` for a while without moving the clock and fails on any event but terrain.
async fn quiet(world: &mut WorldMap) {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(300) {
        tokio::select! {
            biased;
            event = world.next() => {
                let event = event.unwrap();
                assert!(matches!(event, WorldEvent::Terrain(..)), "unexpected {event:?}");
            }
            () = tokio::task::yield_now() => {}
        }
    }
}

/// Drives `next` (failing on events but terrain) until `done` holds.
async fn settle(world: &mut WorldMap, mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() {
        assert!(start.elapsed() < PATIENCE, "settled in time");
        tokio::select! {
            biased;
            event = world.next() => {
                let event = event.unwrap();
                assert!(matches!(event, WorldEvent::Terrain(..)), "unexpected {event:?}");
            }
            () = tokio::task::yield_now() => {}
        }
    }
}

/// Moves the paused clock by `duration` once `next` has reacted to what happened before.
async fn advance(world: &mut WorldMap, duration: Duration) {
    quiet(world).await;
    tokio::time::advance(duration).await;
}

/// Reads `websocket` in the background, so keep-alive pings are answered on paused time.
fn answer_pings(mut websocket: Ws) {
    tokio::spawn(async move { while websocket.next().await.is_some() {} });
}

/// No frame arrives for a while.
async fn no_frame(websocket: &mut Ws) {
    let frame = tokio::time::timeout(Duration::from_millis(300), server::receive(websocket, 1));
    assert!(frame.await.is_err(), "no frame");
}

fn frames<'a>(
    verb: &str,
    channel: &str,
    rooms: impl IntoIterator<Item = &'a RoomCoord>,
) -> BTreeSet<String> {
    rooms
        .into_iter()
        .map(|room| format!("{verb} {channel}:{room}"))
        .collect()
}

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
async fn terrain_is_batched_nearest_first_deduplicated_and_seeded() {
    let fake = Fake::new(40, rect("W19N19", "E19S19"));
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    // The probe's terrain is cached by the client, not seeded: it is still reported.
    let centre = ["W0N0", "E0N0", "W0S0", "E0S0"].map(room);

    let seeded = room("W5N5");
    let terrain = overseer.map().terrain(["W0N0"], None).await.unwrap()["W0N0"].clone();
    world.insert_terrain(seeded, Arc::clone(&terrain));
    assert!(Arc::ptr_eq(&world.terrain(seeded).unwrap(), &terrain));

    let origin = room("W0N0");
    let mut nearest = origin.around(15).collect::<Vec<_>>();
    nearest.sort_by_key(|room| {
        let (dx, dy) = (room.x - origin.x, room.y - origin.y);
        (dx * dx + dy * dy, room.y, room.x)
    });
    let unknown = room("W30N30");
    let mut wanted = nearest.clone();
    wanted.extend([nearest[3], nearest[700], unknown, seeded]);
    world.set_interest(&Interest {
        terrain: wanted.clone(),
        ..Interest::default()
    });

    // 961 rooms but the seeded one: two requests (at least 600 rooms each, at most 3).
    let mut reported = BTreeSet::new();
    while reported.len() < 960 {
        match event(&mut world).await {
            WorldEvent::Terrain(room, _) => assert!(reported.insert(room), "{room} twice"),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(!reported.contains(&seeded) && !reported.contains(&unknown));
    let queue = nearest
        .iter()
        .copied()
        .filter(|&room| room != seeded)
        .chain([unknown])
        .collect::<Vec<_>>();
    let as_sent = |rooms: &[RoomCoord]| {
        rooms
            .iter()
            .filter(|room| !centre.contains(room))
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>()
    };
    let batches = requested(&server, "/api/game/rooms");
    assert_eq!(batches.len(), 3, "the probe and two batches");
    assert_eq!(
        batches[1].iter().cloned().collect::<BTreeSet<_>>(),
        as_sent(&queue[..600])
    );
    assert_eq!(
        batches[2].iter().cloned().collect::<BTreeSet<_>>(),
        as_sent(&queue[600..])
    );

    // Known terrain and unknown rooms are never requested again.
    wanted.reverse();
    world.set_interest(&Interest {
        terrain: wanted,
        ..Interest::default()
    });
    quiet(&mut world).await;
    assert_eq!(requested(&server, "/api/game/rooms").len(), 3);
}

#[tokio::test]
async fn overviews_live_for_the_ttl_and_report_changes() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    // The interest is clipped to the world.
    world.set_interest(&Interest {
        stats: Some(rect("W20N20", "E5S5")),
        ..Interest::default()
    });
    let WorldEvent::Stats(stats) = event(&mut world).await else {
        panic!("overviews expected");
    };
    assert_eq!(stats.len(), 121);
    let requests = map_stats(&server);
    assert_eq!(requests.len(), 1);
    // Nearest the centre of the clipped interest (W5N5) first, its sector first.
    assert_eq!(requests[0][0], "W5N5");
    assert!(requests[0][..100].iter().all(|name| {
        let room = room(name);
        (-10..0).contains(&room.x) && (-10..0).contains(&room.y)
    }));
    let overview = world.stat(room("W1N1")).unwrap();
    let owner = overview.owner.as_ref().unwrap();
    assert_eq!((owner.username.as_deref(), owner.level), (Some("Alice"), 3));

    // Overviews are used for the TTL, then asked again; unchanged ones are not reported.
    advance(
        &mut world,
        STATS_TTL.saturating_sub(Duration::from_secs(60)),
    )
    .await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 1);
    advance(&mut world, Duration::from_secs(61)).await;
    settle(&mut world, || map_stats(&server).len() == 2).await;
    quiet(&mut world).await;

    fake.level.store(4, Ordering::SeqCst);
    // Timers round up to the next millisecond.
    advance(&mut world, STATS_TTL + Duration::from_millis(1)).await;
    let WorldEvent::Stats(stats) = event(&mut world).await else {
        panic!("overviews expected");
    };
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].0, room("W1N1"));
    assert_eq!(stats[0].1.owner.as_ref().unwrap().level, 4);
    assert_eq!(map_stats(&server).len(), 3);
}

#[tokio::test]
async fn big_views_fill_in_capped_batches_of_the_nearest_sectors() {
    // 80 × 80 rooms: 64 sectors.
    let fake = Fake::new(80, rect("W39N39", "E39S39"));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    let mut received = 0;
    for batch in 0..3 {
        let WorldEvent::Stats(stats) = event(&mut world).await else {
            panic!("overviews expected");
        };
        received += stats.len();
        assert_eq!(map_stats(&server).len(), batch + 1);
        advance(&mut world, Duration::from_secs(2)).await;
    }
    assert_eq!(received, 6400);
    let sizes = map_stats(&server).iter().map(Vec::len).collect::<Vec<_>>();
    assert_eq!(sizes, [2500, 2500, 1400]);
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

#[tokio::test]
async fn overviews_follow_the_rate_limit_and_wait_out_a_429() {
    let fake = Fake::new(12, rect("W10N10", "W0N0"));
    lock(&fake.scripted).extend([
        // 10 of 60 left, the window resets in 1000 s: one request per 100 s.
        (200, rate_limit(60, 10, Duration::from_secs(1000))),
        // Rejected until the reset 120 s later (beyond the client's own 60 s retry).
        (429, rate_limit(60, 0, Duration::from_secs(120))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    world.set_interest(&Interest {
        stats: Some(rect("W10N10", "W6N6")),
        ..Interest::default()
    });
    assert!(matches!(event(&mut world).await, WorldEvent::Stats(stats) if stats.len() == 25));

    // New rooms wait for the paced slot.
    world.set_interest(&Interest {
        stats: Some(rect("W5N5", "W0N0")),
        ..Interest::default()
    });
    advance(&mut world, Duration::from_secs(90)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 1);
    advance(&mut world, Duration::from_secs(15)).await;
    settle(&mut world, || map_stats(&server).len() == 2).await;

    // The 429: nothing until its reset.
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);
    advance(&mut world, Duration::from_secs(65)).await;
    let WorldEvent::Stats(stats) = event(&mut world).await else {
        panic!("overviews expected");
    };
    assert_eq!(stats.len(), 36);
    assert_eq!(map_stats(&server).len(), 3);
}

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

/// The next `count` detail events.
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
            WorldEvent::Terrain(..) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    details
}

async fn update(websocket: &mut Ws, room: RoomCoord, time: u64) {
    let frame = format!(r#"["room:{room}",{{"gameTime":{time},"objects":{{}},"users":{{}}}}]"#);
    server::send(websocket, &frame).await;
}

async fn refuse(websocket: &mut Ws, room: RoomCoord) {
    let frame = format!(r#"["err@room:{room}","subscribe limit reached"]"#);
    server::send(websocket, &frame).await;
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
                assert!(matches!(event, WorldEvent::Terrain(..)), "unexpected {event:?}");
            }
        }
    }
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
    refuse(&mut websocket, c).await;
    assert_eq!(details(&mut world, 1).await, [refused]);
    assert_eq!(receive(&mut websocket, 1).await, ["unsubscribe room:W3N3"]);

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
    assert!(world.detail(b).is_none());
    no_frame(&mut websocket).await;
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
    assert_eq!(stats, 1);
    assert_eq!(requested(&server, "/api/game/rooms").len(), 2);
    assert_eq!(map_stats(&server).len(), 1);
    assert_eq!(
        json!(requested(&server, "/api/game/rooms")[1]),
        json!(["W1N1", "W2N2"])
    );
}
