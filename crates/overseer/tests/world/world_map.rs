//! The world map: bounds (measured and known), interest diffing, whole-world terrain batching,
//! absent rooms and seeding, progress, overview near/far lifetimes, seeding and budget split,
//! rate-limit pauses and rechecks, minimap cap, detail priority, refusals (stale ones behind
//! barriers) and terrain reuse, cancel safety.
//!
//! Tests on paused time drive [`WorldMap::next`] by yielding (see [`event`]): a runtime that
//! never idles never advances paused time on its own, so only `tokio::time::advance` moves it.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{FutureExt, StreamExt};
use overseer::core::api::endpoints;
use overseer::map::{
    DetailState, Interest, MINIMAP_CAP, RoomCoord, RoomRect, RoomStat, RoomStatus,
    STATS_BATCH_SECTORS, STATS_FAR_TTL, STATS_TTL, TERRAIN_BATCH, WorldEvent, WorldMap,
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

/// Whether `event` only reports a state: terrain (or its absence) and overview pauses.
const fn is_quiet(event: &WorldEvent) -> bool {
    matches!(
        event,
        WorldEvent::Terrain(..) | WorldEvent::NoTerrain(..) | WorldEvent::StatsPaused(_)
    )
}

/// Drives `next` for a while without moving the clock and fails on any event but terrain
/// (or its absence) and overview pauses.
async fn quiet(world: &mut WorldMap) {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(300) {
        tokio::select! {
            biased;
            event = world.next() => {
                let event = event.unwrap();
                assert!(is_quiet(&event), "unexpected {event:?}");
            }
            () = tokio::task::yield_now() => {}
        }
    }
}

/// Drives `next` (failing on events but terrain and overview pauses) until `done` holds.
async fn settle(world: &mut WorldMap, mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() {
        assert!(start.elapsed() < PATIENCE, "settled in time");
        tokio::select! {
            biased;
            event = world.next() => {
                let event = event.unwrap();
                assert!(is_quiet(&event), "unexpected {event:?}");
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

/// The next overviews, skipping terrain events and overview pauses.
async fn stats(world: &mut WorldMap) -> Vec<(RoomCoord, Arc<RoomStat>)> {
    loop {
        match event(world).await {
            WorldEvent::Stats(stats) => return stats,
            event if is_quiet(&event) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// The next pause change of overview loading, skipping terrain events.
async fn paused(world: &mut WorldMap) -> Option<u64> {
    loop {
        match event(world).await {
            WorldEvent::StatsPaused(until) => return until,
            WorldEvent::Terrain(..) | WorldEvent::NoTerrain(..) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// The rooms of `stats` (or of a request), as a set.
fn rooms_of<'a>(rooms: impl IntoIterator<Item = &'a RoomCoord>) -> BTreeSet<RoomCoord> {
    rooms.into_iter().copied().collect()
}

fn names_of(names: &[String]) -> BTreeSet<RoomCoord> {
    names.iter().map(|name| room(name)).collect()
}

/// Milliseconds since the Unix epoch.
fn epoch_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[tokio::test]
async fn near_overviews_refresh_after_10_minutes_and_far_ones_after_an_hour() {
    // 40 × 40 rooms: 16 sectors.
    let fake = Fake::new(40, rect("W19N19", "E19S19"));
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

    // A view larger than the world: every room of the world is wanted.
    world.set_interest(&Interest {
        stats: Some(rect("W30N30", "E5S5")),
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

    // Then the far rooms, after the pause between requests.
    advance(&mut world, Duration::from_secs(2)).await;
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
async fn far_overviews_leave_the_near_share_of_the_budget_and_a_429_waits_for_the_reset() {
    // 80 × 80 rooms: 64 sectors, the far ones take 3 requests.
    let fake = Fake::new(80, rect("W39N39", "E39S39"));
    lock(&fake.scripted).extend([
        // Near: 40 of 60 left, the window resets in 1000 s.
        (200, rate_limit(60, 40, Duration::from_secs(1000))),
        // Far: 18 left (other windows share the token), the near share (30 %): no more far
        // requests until the reset.
        (200, rate_limit(60, 18, Duration::from_secs(1000))),
        // Near again: rejected until the reset 120 s later (beyond the client's own 60 s).
        (429, rate_limit(60, 0, Duration::from_secs(120))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    let bounds = world.bounds();

    // Near: the minimap's sector and those around it inside the world (W39N39..W20N20).
    world.set_interest(&Interest {
        stats: Some(bounds),
        minimaps: vec![room("W35N35")],
        ..Interest::default()
    });
    let near = stats(&mut world).await;
    assert_eq!(
        rooms_of(near.iter().map(|(room, _)| room)),
        rooms_of(&rect("W39N39", "W20N20").iter().collect::<Vec<_>>())
    );
    advance(&mut world, Duration::from_secs(2)).await;
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);

    // The far rooms left wait for the window's reset.
    advance(&mut world, Duration::from_secs(300)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);
    assert!(world.progress().stats_paused_until.is_some());

    // New near rooms still go out: the 429 holds them until its reset.
    world.set_interest(&Interest {
        stats: Some(bounds),
        minimaps: vec![room("E35S35")],
        ..Interest::default()
    });
    settle(&mut world, || map_stats(&server).len() == 3).await;
    let corner = rooms_of(&rect("E20S20", "E39S39").iter().collect::<Vec<_>>());
    assert!(names_of(&map_stats(&server)[2]).is_subset(&corner));
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 3);
    assert!(world.progress().stats_paused_until.is_some());
    advance(&mut world, Duration::from_secs(65)).await;
    let near = stats(&mut world).await;
    assert!(rooms_of(near.iter().map(|(room, _)| room)).is_subset(&corner));
    assert_eq!(map_stats(&server).len(), 4);

    // After the reset the far rooms resume, until every room is known.
    while !world.progress().is_complete() {
        advance(&mut world, Duration::from_secs(2)).await;
        stats(&mut world).await;
    }
    assert_eq!(world.progress().stats_known, 6400);
    assert!(map_stats(&server).len() <= 6);
    quiet(&mut world).await;
    assert_eq!(world.progress().stats_paused_until, None);
}

/// The reset of `X-RateLimit-*` `headers` as [`WorldProgress::stats_paused_until`] reports
/// it: milliseconds since the Unix epoch, a second after the reset (clock skew).
///
/// [`WorldProgress::stats_paused_until`]: overseer::map::WorldProgress::stats_paused_until
fn paused_until(headers: &[(&'static str, String)]) -> u64 {
    let (_, reset) = headers
        .iter()
        .find(|(name, _)| *name == "X-RateLimit-Reset")
        .unwrap();
    (reset.parse::<u64>().unwrap() + 1) * 1000
}

#[tokio::test]
async fn a_paused_overview_load_says_until_when_and_a_recheck_after_a_reset_resumes() {
    // 80 × 80 rooms, all far (no minimap): three requests of the nearest sectors.
    let fake = Fake::new(80, rect("W39N39", "E39S39"));
    // 18 of 60 left after the first: only the near share, so far rooms wait for the reset in
    // half an hour.
    let exhausted = rate_limit(60, 18, Duration::from_mins(30));
    let until = paused_until(&exhausted);
    lock(&fake.scripted).push_back((200, exhausted));
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    assert_eq!(world.progress().stats_paused_until, None);
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    // Paused, and until when: the reset as the server's clock says it.
    assert_eq!(paused(&mut world).await, Some(until));
    let progress = world.progress();
    assert_eq!(progress.stats_paused_until, Some(until));
    assert!(!progress.is_complete());
    let left = until - epoch_millis();
    assert!((1_790_000..=1_802_000).contains(&left), "{left} ms left");
    advance(&mut world, Duration::from_secs(600)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 1);

    // A recheck while the limit still holds: one request, refused, and paused again.
    lock(&fake.scripted).push_back((429, rate_limit(60, 0, Duration::from_mins(20))));
    assert!(world.recheck_rate_limit());
    assert_eq!(paused(&mut world).await, None);
    let again = paused(&mut world).await;
    assert!(again.is_some_and(|again| again < until), "{again:?}");
    assert_eq!(world.progress().stats_paused_until, again);
    assert_eq!(map_stats(&server).len(), 2);
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);

    // The user lifted the limit: the recheck's request goes out at once, its headers record
    // the real limit, and loading resumes until every room is known.
    lock(&fake.scripted).push_back((200, rate_limit(60, 59, Duration::from_secs(3600))));
    assert!(world.recheck_rate_limit());
    assert_eq!(paused(&mut world).await, None);
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    assert_eq!(map_stats(&server).len(), 3);
    assert_eq!(
        overseer
            .client()
            .rate_limit::<endpoints::GameMapStats>()
            .map(|limit| limit.remaining),
        Some(59)
    );
    advance(&mut world, Duration::from_secs(2)).await;
    assert_eq!(stats(&mut world).await.len(), 1400);
    assert_eq!(map_stats(&server).len(), 4);
    let progress = world.progress();
    assert!(progress.is_complete());
    assert_eq!(progress.stats_paused_until, None);
}

#[tokio::test]
async fn a_recheck_while_not_paused_sends_nothing() {
    let fake = Fake::new(40, rect("W19N19", "E19S19"));
    // Few requests left, but nothing waits for them.
    lock(&fake.scripted).extend([
        (200, rate_limit(60, 40, Duration::from_mins(30))),
        (200, rate_limit(60, 10, Duration::from_mins(30))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    // Before any request.
    assert!(!world.recheck_rate_limit());
    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        minimaps: vec![room("W15N15")],
        ..Interest::default()
    });
    // Between two requests, while pacing.
    assert_eq!(stats(&mut world).await.len(), 400);
    assert!(!world.recheck_rate_limit());
    advance(&mut world, Duration::from_secs(2)).await;
    assert_eq!(stats(&mut world).await.len(), 1200);
    // Every room known and fresh: 10 left is no pause.
    quiet(&mut world).await;
    assert_eq!(world.progress().stats_paused_until, None);
    assert!(!world.recheck_rate_limit());
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);
    // The limit seen is kept.
    assert_eq!(
        overseer
            .client()
            .rate_limit::<endpoints::GameMapStats>()
            .map(|limit| limit.remaining),
        Some(10)
    );
}

#[tokio::test]
async fn an_exempt_token_ignores_the_budget_but_not_a_429() {
    // 80 × 80 rooms, all far (no minimap): three requests of the nearest sectors.
    let fake = Fake::new(80, rect("W39N39", "E39S39"));
    let exhausted = rate_limit(60, 18, Duration::from_mins(30));
    let until = paused_until(&exhausted);
    lock(&fake.scripted).extend([
        // Down to the near share: far rooms wait for the reset.
        (200, exhausted),
        // Refused (another client used the window) until the reset 120 s later.
        (429, rate_limit(60, 0, Duration::from_secs(120))),
        // 5 left: the budget would spread them over the window.
        (200, rate_limit(60, 5, Duration::from_mins(30))),
    ]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    assert_eq!(paused(&mut world).await, Some(until));

    // Lifted for two hours: the pause ends and the next request goes out after the gap.
    world.set_rate_limit_exempt_until(Some(epoch_millis() + 7_200_000));
    assert_eq!(paused(&mut world).await, None);
    advance(&mut world, Duration::from_secs(2)).await;
    settle(&mut world, || map_stats(&server).len() == 2).await;
    // The 429 still holds until its reset.
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);
    assert_eq!(world.progress().stats_paused_until, None);
    advance(&mut world, Duration::from_secs(65)).await;
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    // 5 left is no reason to wait: 1 s later the last rooms.
    advance(&mut world, Duration::from_secs(2)).await;
    assert_eq!(stats(&mut world).await.len(), 1400);
    assert_eq!(map_stats(&server).len(), 4);
    let progress = world.progress();
    assert!(progress.is_complete());
    assert_eq!(progress.stats_paused_until, None);
}

#[tokio::test]
async fn the_budget_applies_again_when_the_exemption_ends() {
    // 80 × 80 rooms, all far (no minimap): three requests of the nearest sectors.
    let fake = Fake::new(80, rect("W39N39", "E39S39"));
    let exhausted = rate_limit(60, 18, Duration::from_mins(30));
    let until = paused_until(&exhausted);
    lock(&fake.scripted).extend([(200, exhausted.clone()), (200, exhausted)]);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();

    // Exempt for 3 s: the first two requests go out despite the budget.
    world.set_rate_limit_exempt_until(Some(epoch_millis() + 3_000));
    world.set_interest(&Interest {
        stats: Some(world.bounds()),
        ..Interest::default()
    });
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    advance(&mut world, Duration::from_millis(1500)).await;
    assert_eq!(stats(&mut world).await.len(), STATS_BATCH_SECTORS * 100);
    assert_eq!(world.progress().stats_paused_until, None);

    // Then the near share holds the far rooms back until the reset.
    advance(&mut world, Duration::from_secs(2)).await;
    assert_eq!(paused(&mut world).await, Some(until));
    advance(&mut world, Duration::from_secs(60)).await;
    quiet(&mut world).await;
    assert_eq!(map_stats(&server).len(), 2);
    // A past exemption changes nothing.
    world.set_rate_limit_exempt_until(Some(epoch_millis() - 1));
    quiet(&mut world).await;
    assert_eq!(world.progress().stats_paused_until, Some(until));
    assert_eq!(map_stats(&server).len(), 2);
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
    let batches = requested(&server, "/api/game/rooms");
    assert_eq!(batches.len(), before + 1);
    assert_eq!(
        batches[before]
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["W1N1", "W30N30", "W6N6"])
    );
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
    assert_eq!(requested(&server, "/api/game/rooms").len(), 2);
    assert_eq!(map_stats(&server).len(), 1);
    assert_eq!(
        json!(requested(&server, "/api/game/rooms")[1]),
        json!(["W1N1", "W2N2"])
    );
}
