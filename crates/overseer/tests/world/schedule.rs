//! How the world map orders and paces its work: tiers (viewed, visible with the prefetch ring,
//! background), re-prioritizing on pans without dropping work in flight, live rooms updating
//! overviews and portals at once (and winning over older responses), viewed rooms refreshed
//! whatever their lifetime, pacing per server, no room asked twice.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{FutureExt as _, StreamExt as _};
use overseer::map::{
    DetailState, Interest, PREFETCH_RING, RoomCoord, RoomRect, RoomStat, RoomStatus, TERRAIN_BATCH,
    WorldEvent, WorldMap,
};
use serde_json::Value;

use crate::portals::{Socket, drive as drive_portals, room_objects, settled};
use crate::server::{self, Request, Response, Server, Ws};
use crate::{PATIENCE, connect, receive, terrain_document};

fn room(name: &str) -> RoomCoord {
    name.parse().unwrap()
}

fn rect(a: &str, b: &str) -> RoomRect {
    RoomRect::new(room(a), room(b))
}

/// The four central rooms the world's bounds are measured with: the client caches their
/// terrain, so they are never sent again.
const PROBE: [&str; 4] = ["W0N0", "E0N0", "W0S0", "E0S0"];

/// A `size` × `size` world centred on the zero lines. Room `W1N1` is owned by `u1` at
/// `level`; every other room is normal and unowned. A `limited` server shows rate limits
/// (`X-RateLimit-*` on its world-size reply).
#[derive(Clone)]
struct Fake {
    size: i32,
    limited: bool,
    level: Arc<AtomicI64>,
}

impl Fake {
    fn new(size: i32, limited: bool) -> Self {
        Self {
            size,
            limited,
            level: Arc::new(AtomicI64::new(3)),
        }
    }

    /// Every room of the world.
    const fn bounds(&self) -> RoomRect {
        let half = self.size / 2;
        RoomRect {
            min: RoomCoord { x: -half, y: -half },
            max: RoomCoord {
                x: self.size - half - 1,
                y: self.size - half - 1,
            },
        }
    }

    fn routes(&self) -> impl Fn(&Request) -> Response + Send + Sync + 'static {
        let fake = self.clone();
        move |request| fake.reply(request)
    }

    /// The requested rooms (`rooms` of the JSON body) inside the world.
    fn existing(&self, request: &Request) -> Vec<String> {
        rooms_of(request)
            .into_iter()
            .filter(|name| self.bounds().contains(room(name)))
            .collect()
    }

    fn reply(&self, request: &Request) -> Response {
        let ok = |body: String| Response {
            status: 200,
            headers: Vec::new(),
            body: body.into_bytes(),
        };
        match request.path.as_str() {
            "/api/game/world-size" => {
                let mut response = ok(format!(r#"{{"ok":1,"width":{0},"height":{0}}}"#, self.size));
                if self.limited {
                    let reset = (SystemTime::now() + Duration::from_mins(1))
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs();
                    response.headers = vec![
                        ("X-RateLimit-Limit", "120".to_owned()),
                        ("X-RateLimit-Remaining", "119".to_owned()),
                        ("X-RateLimit-Reset", reset.to_string()),
                    ];
                }
                response
            }
            "/api/game/rooms" => {
                let documents = self
                    .existing(request)
                    .iter()
                    .map(|name| terrain_document(name))
                    .collect::<Vec<_>>();
                ok(format!(r#"{{"ok":1,"rooms":[{}]}}"#, documents.join(",")))
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
                ok(format!(
                    r#"{{"ok":1,"gameTime":7,"stats":{{{}}},"users":{{"u1":{{"_id":"u1","username":"Alice"}}}}}}"#,
                    entries.join(",")
                ))
            }
            _ => Response {
                status: 404,
                headers: Vec::new(),
                body: br#"{"error":"not found"}"#.to_vec(),
            },
        }
    }
}

/// The `rooms` of a request's JSON body.
fn rooms_of(request: &Request) -> Vec<String> {
    let body: Value = serde_json::from_str(&request.body).unwrap();
    body["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// The rooms of each request to `path` so far, in the order the server received them.
fn requested(server: &Server, path: &str) -> Vec<Vec<String>> {
    server
        .requests()
        .iter()
        .filter(|request| request.path == path)
        .map(rooms_of)
        .collect()
}

/// Drives `next` without letting the runtime idle (paused time stays put) until `done` holds
/// for the events so far, which it returns.
async fn drive(
    world: &mut WorldMap,
    mut done: impl FnMut(&WorldMap, &[WorldEvent]) -> bool,
) -> Vec<WorldEvent> {
    let start = std::time::Instant::now();
    let mut events = Vec::new();
    while !done(world, &events) {
        assert!(start.elapsed() < PATIENCE, "done in time");
        tokio::select! {
            biased;
            event = world.next() => events.push(event.unwrap()),
            () = tokio::task::yield_now() => {}
        }
    }
    events
}

/// Drives `next` for a while without moving the clock; returns the events.
async fn drive_for(world: &mut WorldMap) -> Vec<WorldEvent> {
    let until = std::time::Instant::now() + Duration::from_millis(300);
    drive(world, |_, _| std::time::Instant::now() >= until).await
}

/// Whether every room of `rect` has a terrain.
fn terrain_complete(world: &WorldMap, rect: RoomRect) -> bool {
    rect.iter().all(|room| world.terrain(room).is_some())
}

/// The order of the visible and background rooms: nearest `centre`, ties north-west first.
fn nearest(centre: RoomCoord) -> impl Fn(&RoomCoord) -> (i64, i32, i32) {
    move |room| {
        let (dx, dy) = (i64::from(room.x - centre.x), i64::from(room.y - centre.y));
        (dx * dx + dy * dy, room.y, room.x)
    }
}

/// `rect` grown by [`PREFETCH_RING`] rooms on each side.
const fn ring(rect: RoomRect) -> RoomRect {
    RoomRect {
        min: RoomCoord {
            x: rect.min.x - PREFETCH_RING,
            y: rect.min.y - PREFETCH_RING,
        },
        max: RoomCoord {
            x: rect.max.x + PREFETCH_RING,
            y: rect.max.y + PREFETCH_RING,
        },
    }
}

/// Asserts that no room was sent twice across `batches`, and returns them all.
fn once(batches: &[Vec<String>]) -> BTreeSet<String> {
    let all = batches.iter().flatten().collect::<Vec<_>>();
    let unique = all
        .iter()
        .map(|name| (*name).clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), all.len(), "a room sent twice");
    unique
}

/// The terrain interest of `rect`, nearest its centre first (as the renderer lists it).
fn camera(rect: RoomRect, centre: RoomCoord) -> Vec<RoomCoord> {
    let mut rooms = rect.iter().collect::<Vec<_>>();
    rooms.sort_by_key(nearest(centre));
    rooms
}

#[tokio::test]
async fn terrain_goes_viewed_then_visible_then_the_ring_then_the_background() {
    let fake = Fake::new(60, false);
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let bounds = world.bounds();
    assert_eq!(bounds, fake.bounds());
    let (viewed, centre) = (room("E25S25"), room("E0S0"));
    let visible = rect("W2N2", "E1S1");
    let visible_rooms = camera(visible, centre);
    world.set_interest(&Interest {
        terrain: visible_rooms.clone(),
        detail: vec![viewed],
        ..Interest::default()
    });
    let events = drive(&mut world, |world, _| terrain_complete(world, bounds)).await;

    // The viewed room, the visible rooms in their order, then the ring nearest the centre:
    // one batch, reported in that order.
    let mut ring_rooms = ring(visible)
        .iter()
        .filter(|room| !visible.contains(*room))
        .collect::<Vec<_>>();
    ring_rooms.sort_by_key(nearest(centre));
    let foreground = [viewed]
        .into_iter()
        .chain(visible_rooms)
        .chain(ring_rooms)
        .collect::<Vec<_>>();
    let order = events
        .iter()
        .filter_map(|event| match event {
            WorldEvent::Terrain(room, _) if foreground.contains(room) => Some(*room),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(order, foreground);
    let batches = requested(&server, "/api/game/rooms")[1..].to_vec();
    let sent = once(&batches);
    let first = batches
        .iter()
        .find(|batch| batch.contains(&viewed.to_string()))
        .unwrap();
    let expected = foreground
        .iter()
        .map(ToString::to_string)
        .filter(|name| !PROBE.contains(&name.as_str()))
        .collect::<BTreeSet<_>>();
    assert_eq!(first.iter().cloned().collect::<BTreeSet<_>>(), expected);

    // Then the rest of the world in the background, in batches nearest the centre.
    let mut rest = bounds
        .iter()
        .filter(|room| !foreground.contains(room))
        .collect::<Vec<_>>();
    rest.sort_by_key(nearest(centre));
    let nearest_batch = rest[..TERRAIN_BATCH]
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    assert!(
        batches
            .iter()
            .any(|batch| batch.iter().cloned().collect::<BTreeSet<_>>() == nearest_batch)
    );
    assert!(batches.iter().all(|batch| batch.len() <= TERRAIN_BATCH));
    assert_eq!(sent.len() + PROBE.len(), bounds.len());
    // Each reported once (the last ones may still wait in the queue).
    let events = [events, drive_for(&mut world).await].concat();
    let reported = events
        .iter()
        .filter(|event| matches!(event, WorldEvent::Terrain(..)))
        .count();
    assert_eq!(reported, bounds.len());
}

#[tokio::test]
async fn a_pan_reprioritizes_without_dropping_work_in_flight() {
    // 100 × 100 rooms: the background takes five batches.
    let fake = Fake::new(100, false);
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let bounds = world.bounds();
    let (a, b) = (rect("W49N49", "W47N47"), rect("E47S47", "E49S49"));
    world.set_interest(&Interest {
        terrain: camera(a, room("W48N48")),
        ..Interest::default()
    });
    // The first poll starts the view's batch and the first background batch.
    let _ = world.next().now_or_never();
    // A pan to the opposite corner while both are in flight.
    world.set_interest(&Interest {
        terrain: camera(b, room("E48S48")),
        ..Interest::default()
    });
    drive(&mut world, |world, _| terrain_complete(world, bounds)).await;

    let batches = requested(&server, "/api/game/rooms")[1..].to_vec();
    once(&batches);
    assert!(batches.len() >= 6, "{} batches", batches.len());
    let holding = |rect: RoomRect| {
        batches
            .iter()
            .position(|batch| batch.contains(&rect.min.to_string()))
            .unwrap()
    };
    // Nothing in flight was dropped: the first view's rooms went out once, in the first two
    // requests; the new view's batch went out as soon as a request finished, ahead of the
    // rest of the background.
    assert!(holding(a) < 2);
    assert!(holding(b) < 4, "the new view waited for the background");
    let b_names = b.iter().map(|room| room.to_string()).collect::<Vec<_>>();
    assert!(
        b_names
            .iter()
            .all(|name| batches[holding(b)].contains(name))
    );
}

/// A live update of `room` at `tick`: its controller owned by `u2` (Bob) at `level` with safe
/// mode on, and one portal to `E0S0`.
fn live_room(room: RoomCoord, tick: u64, level: u64) -> String {
    format!(
        r#"["room:{room}",{{"gameTime":{tick},"objects":{{
            "c1":{{"_id":"c1","type":"controller","room":"{room}","x":10,"y":10,"level":{level},"user":"u2","safeMode":1000}},
            "p1":{{"_id":"p1","type":"portal","room":"{room}","x":25,"y":25,"destination":{{"room":"E0S0","x":5,"y":6}}}}}},
            "users":{{"u2":{{"_id":"u2","username":"Bob"}}}}}}]"#
    )
}

/// The overviews of `room` among `events`, in order.
fn stats_of(events: &[WorldEvent], room: RoomCoord) -> Vec<Arc<RoomStat>> {
    events
        .iter()
        .filter_map(|event| match event {
            WorldEvent::Stats(stats) => stats
                .iter()
                .find(|(of, _)| *of == room)
                .map(|(_, stat)| Arc::clone(stat)),
            _ => None,
        })
        .collect()
}

fn owner(stat: &RoomStat) -> Option<(&str, Option<&str>, u8)> {
    stat.owner
        .as_ref()
        .map(|owner| (owner.id.as_str(), owner.username.as_deref(), owner.level))
}

#[tokio::test]
async fn live_rooms_update_overviews_and_portals_at_once_and_win_over_older_responses() {
    let fake = Fake::new(12, false);
    let (server, overseer, mut websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let knowledge = world.knowledge();
    let mut revisions = knowledge.watch();
    let viewed = room("W5N5");
    let terrain = overseer.map().terrain(["W0N0"], None).await.unwrap()["W0N0"].clone();
    world.insert_terrain(viewed, terrain);
    world.set_interest(&Interest {
        stats: Some(RoomRect::new(viewed, viewed)),
        detail: vec![viewed],
        portals: Some(RoomRect::new(viewed, viewed)),
        ..Interest::default()
    });
    // The seeded room subscribes at once; its first update is waiting before the world map
    // sends its first request.
    assert_eq!(receive(&mut websocket, 1).await, ["subscribe room:W5N5"]);
    server::send(&mut websocket, &live_room(viewed, 10, 5)).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let events = drive(&mut world, |_, events| {
        stats_of(events, viewed).len() == 2
            && events
                .iter()
                .any(|event| matches!(event, WorldEvent::Portals(room, _) if *room == viewed))
    })
    .await;
    // The live room's overview first, at once: only that room.
    let live = events
        .iter()
        .find_map(|event| match event {
            WorldEvent::Stats(stats) => Some(stats),
            _ => None,
        })
        .unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(owner(&live[0].1), Some(("u2", Some("Bob"), 5)));
    assert!(live[0].1.safe_mode);
    // The overview requested (first, alone: the room became viewed) before the update arrived
    // says the room is unowned: it brings the status, the live room's owner stays.
    assert_eq!(
        requested(&server, "/api/game/map-stats")[0],
        ["W5N5".to_owned()]
    );
    let fetched = &stats_of(&events, viewed)[1];
    assert_eq!(owner(fetched), Some(("u2", Some("Bob"), 5)));
    assert_eq!(fetched.status, RoomStatus::Normal);
    let known = world.stat(viewed).unwrap();
    assert!(Arc::ptr_eq(&known, &knowledge.stat(viewed).unwrap()));
    assert_eq!(owner(&known), Some(("u2", Some("Bob"), 5)));
    // Its portals with their destinations, without any request.
    let portals = knowledge.portals(viewed).unwrap();
    assert_eq!(portals.len(), 1);
    assert_eq!(
        portals[0]
            .destination
            .as_ref()
            .map(|destination| destination.room),
        Some(room("E0S0"))
    );
    assert_eq!(world.portals(viewed), Some(portals));
    assert_eq!(room_objects(&server), [] as [String; 0]);
    assert!(revisions.has_changed().unwrap());

    // The same again changes nothing (once the rest of the world's overviews are in).
    let bounds = world.bounds();
    drive(&mut world, |world, _| {
        world.progress().stats_known == bounds.len()
    })
    .await;
    drive_for(&mut world).await;
    revisions.borrow_and_update();
    let revision = knowledge.revision();
    server::send(&mut websocket, &live_room(viewed, 11, 5)).await;
    let events = drive(&mut world, |_, events| {
        events
            .iter()
            .any(|event| matches!(event, WorldEvent::Detail(_, DetailState::Live(_))))
    })
    .await;
    let events = [events, drive_for(&mut world).await].concat();
    assert!(
        events
            .iter()
            .all(|event| matches!(event, WorldEvent::Detail(..))),
        "{events:?}"
    );
    assert_eq!(knowledge.revision(), revision);
    assert!(!revisions.has_changed().unwrap());

    // The controller levels up: the overview follows at once, without a request.
    let asked = requested(&server, "/api/game/map-stats").len();
    server::send(
        &mut websocket,
        r#"["room:W5N5",{"gameTime":12,"objects":{"c1":{"level":6}}}]"#,
    )
    .await;
    let events = drive(&mut world, |_, events| !stats_of(events, viewed).is_empty()).await;
    assert_eq!(
        owner(&stats_of(&events, viewed)[0]),
        Some(("u2", Some("Bob"), 6))
    );
    assert_eq!(owner(&knowledge.stat(viewed).unwrap()).unwrap().2, 6);
    assert!(revisions.has_changed().unwrap());
    assert_eq!(requested(&server, "/api/game/map-stats").len(), asked);
}

/// An unowned, normal overview fetched at `fetched_at`.
fn seed(fetched_at: u64) -> Arc<RoomStat> {
    Arc::new(RoomStat {
        status: RoomStatus::Normal,
        status_until: None,
        owner: None,
        reservation: None,
        safe_mode: false,
        mineral: None,
        mineral_density: None,
        fetched_at,
    })
}

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
async fn a_viewed_room_is_refreshed_at_once_whatever_its_lifetime() {
    let fake = Fake::new(12, false);
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let bounds = world.bounds();
    // Every overview two minutes old: fresh for another 8 (near) or 58 (far) minutes.
    let now = epoch_millis();
    for room in bounds.iter() {
        world.insert_stats(room, seed(now - 120_000));
    }
    // This one was asked half a minute ago.
    world.insert_stats(room("W2N2"), seed(now - 30_000));
    world.set_interest(&Interest {
        stats: Some(bounds),
        ..Interest::default()
    });
    drive_for(&mut world).await;
    assert_eq!(requested(&server, "/api/game/map-stats").len(), 0);

    // Viewing a room asks for its overview at once, and only for it.
    fake.level.store(7, Ordering::SeqCst);
    world.set_interest(&Interest {
        stats: Some(bounds),
        detail: vec![room("W1N1")],
        ..Interest::default()
    });
    let events = drive(&mut world, |_, events| {
        !stats_of(events, room("W1N1")).is_empty()
    })
    .await;
    assert_eq!(
        requested(&server, "/api/game/map-stats"),
        [vec!["W1N1".to_owned()]]
    );
    assert_eq!(
        owner(&stats_of(&events, room("W1N1"))[0]),
        Some(("u1", Some("Alice"), 7))
    );

    // A room asked less than a minute before it is viewed, or still viewed, is not asked again.
    world.set_interest(&Interest {
        stats: Some(bounds),
        detail: vec![room("W1N1"), room("W2N2")],
        ..Interest::default()
    });
    drive_for(&mut world).await;
    assert_eq!(requested(&server, "/api/game/map-stats").len(), 1);
}

#[tokio::test]
async fn overviews_go_without_pauses_on_servers_without_rate_limits() {
    // 120 × 120 rooms: three requests of up to 50 sectors.
    let fake = Fake::new(120, false);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    let bounds = world.bounds();
    world.set_interest(&Interest {
        stats: Some(bounds),
        ..Interest::default()
    });
    // The clock never moves: no pause between requests.
    drive(&mut world, |world, _| {
        world.progress().stats_known == bounds.len()
    })
    .await;
    let batches = requested(&server, "/api/game/map-stats");
    assert_eq!(batches.len(), 3);
    once(&batches);
}

#[tokio::test]
async fn overviews_go_one_at_a_time_without_gaps_on_servers_with_rate_limits() {
    let fake = Fake::new(120, true);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    let bounds = world.bounds();
    world.set_interest(&Interest {
        stats: Some(bounds),
        ..Interest::default()
    });
    // The clock never moves: the endpoint's budget (none reported, so one request at a
    // time) and the request budget leave the three requests no pause, one after the other.
    drive(&mut world, |world, _| {
        world.progress().stats_known == bounds.len()
    })
    .await;
    assert_eq!(requested(&server, "/api/game/map-stats").len(), 3);
}

#[tokio::test]
async fn portal_destinations_go_together_on_servers_without_rate_limits() {
    let (server, overseer, websocket) = connect(crate::portals::routes(40, false)).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let mut socket = Socket::new(websocket, []);
    tokio::time::pause();
    world.set_interest(&Interest {
        portals: Some(world.bounds()),
        ..Interest::default()
    });
    // The clock never moves: the four rooms with portals are asked without pauses.
    drive_portals(&mut world, &mut socket, |events| settled(events) == 1600).await;
    let mut asked = room_objects(&server);
    asked.sort();
    assert_eq!(asked, ["E5S5", "W0N0", "W0S0", "W1N1"]);
}

#[tokio::test]
async fn no_room_is_asked_twice_whatever_the_interests() {
    let fake = Fake::new(20, false);
    let (server, overseer, _websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let bounds = world.bounds();
    // Two subscribers' rooms, merged with repetitions; then pans back and forth while
    // requests are in flight.
    let here = Interest {
        stats: Some(rect("W3N3", "E2S2")),
        terrain: vec![room("W1N1"), room("W1N1"), room("W2N2"), room("W1N1")],
        detail: vec![room("W1N1"), room("W1N1")],
        portals: None,
        minimaps: Vec::new(),
        portal_destinations: Vec::new(),
    };
    let there = Interest {
        stats: Some(rect("E3S3", "E8S8")),
        terrain: vec![room("E5S5"), room("W1N1")],
        detail: vec![room("E5S5")],
        portals: None,
        minimaps: Vec::new(),
        portal_destinations: Vec::new(),
    };
    for interest in [&here, &there, &here, &there, &here] {
        world.set_interest(interest);
        let _ = world.next().now_or_never();
    }
    drive(&mut world, |world, _| {
        terrain_complete(world, bounds) && world.progress().stats_known == bounds.len()
    })
    .await;
    let terrain = once(&requested(&server, "/api/game/rooms")[1..]);
    assert_eq!(terrain.len() + PROBE.len(), bounds.len());
    let stats = once(&requested(&server, "/api/game/map-stats"));
    assert_eq!(stats.len(), bounds.len());
}

/// Reads `websocket` in the background, so keep-alive pings are answered on paused time.
fn answer_pings(mut websocket: Ws) {
    tokio::spawn(async move { while websocket.next().await.is_some() {} });
}

#[tokio::test]
async fn an_exempt_token_paces_nothing_on_servers_with_rate_limits() {
    let fake = Fake::new(120, true);
    let (server, overseer, websocket) = connect(fake.routes()).await;
    answer_pings(websocket);
    let mut world = overseer.map().world(None).await.unwrap();
    tokio::time::pause();
    let bounds = world.bounds();
    overseer
        .rate_limits()
        .set_exempt_until(Some(epoch_millis() + 3_600_000));
    world.set_interest(&Interest {
        stats: Some(bounds),
        ..Interest::default()
    });
    // The clock never moves: no pause between requests while exempt.
    drive(&mut world, |world, _| {
        world.progress().stats_known == bounds.len()
    })
    .await;
    let batches = requested(&server, "/api/game/map-stats");
    assert_eq!(batches.len(), 3);
    once(&batches);
}

#[tokio::test]
async fn a_failing_portal_room_backs_off_alone() {
    // `W0N0`'s room objects always fail; the other rooms with portals go on.
    let routes = crate::portals::routes(40, false);
    let (server, overseer, websocket) = connect(move |request: &Request| {
        if request.path == "/api/game/room-objects" && request.query.contains("room=W0N0") {
            return Response {
                status: 500,
                headers: Vec::new(),
                body: br#"{"error":"broken"}"#.to_vec(),
            };
        }
        routes(request)
    })
    .await;
    let mut world = overseer.map().world(None).await.unwrap();
    let mut socket = Socket::new(websocket, []);
    world.set_interest(&Interest {
        portals: Some(world.bounds()),
        ..Interest::default()
    });
    // Every other room is known although the nearest room with portals fails, which keeps
    // its pending destinations.
    drive_portals(&mut world, &mut socket, |events| settled(events) == 1599).await;
    let failing = world.portals(room("W0N0")).unwrap();
    assert!(failing.iter().all(|portal| portal.destination.is_none()));
    assert!(world.portals(room("E5S5")).is_some() && world.portals(room("W0S0")).is_some());
    let asked = |name: &str| {
        room_objects(&server)
            .iter()
            .filter(|room| *room == name)
            .count()
    };
    assert_eq!(asked("W0N0"), 1);
    assert_eq!(asked("E5S5") + asked("W0S0") + asked("W1N1"), 3);
    // Only that room waits (30 s, doubling with each failure in a row).
    let until = std::time::Instant::now() + Duration::from_millis(500);
    drive_portals(&mut world, &mut socket, |_| {
        std::time::Instant::now() >= until
    })
    .await;
    assert_eq!(asked("W0N0"), 1);
}
