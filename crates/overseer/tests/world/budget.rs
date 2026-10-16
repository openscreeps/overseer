//! The request budget the world map shares with everything else of the session (a sliding
//! minute of every request, user actions included; priorities; HTTP 429; lifted limits;
//! servers without limits), portals inferred from their pairs (exact pairs only), and portals
//! outside the usual rooms found at full zoom-out, the usual rooms scanned first.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use overseer::map::{DetailState, Interest, RoomCoord, RoomRect, WorldEvent, WorldMap};
use overseer::net::{Priority, REQUEST_WINDOW};
use serde_json::Value;

use crate::server::{self, Request, Response, Server, Ws};
use crate::{PATIENCE, connect, query, terrain_document};

fn room(name: &str) -> RoomCoord {
    name.parse().unwrap()
}

/// A portal object of `from` at `tile` leading to `to` at `at` (inter-shard with `shard`).
fn portal(
    from: &str,
    tile: (u8, u8),
    to: &str,
    at: Option<(u8, u8)>,
    shard: Option<&str>,
) -> String {
    let at = at.map_or_else(String::new, |(x, y)| format!(r#","x":{x},"y":{y}"#));
    let shard = shard.map_or_else(String::new, |shard| format!(r#","shard":"{shard}""#));
    let destination = format!(r#""room":"{to}"{at}{shard}"#);
    format!(
        r#"{{"_id":"{from}-{}-{}","type":"portal","room":"{from}","x":{},"y":{},"unstableDate":4102444800000,"destination":{{{destination}}}}}"#,
        tile.0, tile.1, tile.0, tile.1
    )
}

/// A `size` × `size` world (40: `W20N20`..`E19S19`) whose rooms hold `objects`; a `limited`
/// server shows
/// rate limits; the first room-objects request is refused with HTTP 429 (no headers) while
/// `refuse` is set.
#[derive(Clone)]
struct Fake {
    size: u32,
    limited: bool,
    objects: Arc<HashMap<String, Vec<String>>>,
    refuse: Arc<AtomicBool>,
}

impl Fake {
    fn routes(&self) -> impl Fn(&Request) -> Response + Send + Sync + 'static {
        let fake = self.clone();
        move |request| fake.reply(request)
    }

    fn reply(&self, request: &Request) -> Response {
        let ok = |body: String| Response {
            status: 200,
            headers: Vec::new(),
            body: body.into_bytes(),
        };
        match request.path.as_str() {
            "/api/game/world-size" => {
                let size = self.size;
                let mut response = ok(format!(r#"{{"ok":1,"width":{size},"height":{size}}}"#));
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
                let body: Value = serde_json::from_str(&request.body).unwrap();
                let documents = body["rooms"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(Value::as_str)
                    .map(terrain_document)
                    .collect::<Vec<_>>();
                ok(format!(r#"{{"ok":1,"rooms":[{}]}}"#, documents.join(",")))
            }
            "/api/game/room-objects" => {
                if self.refuse.swap(false, Ordering::SeqCst) {
                    return Response {
                        status: 429,
                        headers: Vec::new(),
                        body: b"Too Many Requests".to_vec(),
                    };
                }
                let name = query(request, "room").unwrap_or_default();
                let objects = self.objects.get(name).cloned().unwrap_or_default();
                ok(format!(
                    r#"{{"ok":1,"objects":[{}],"users":{{}}}}"#,
                    objects.join(",")
                ))
            }
            "/api/game/world-status" => ok(r#"{"ok":1,"status":"normal"}"#.to_owned()),
            _ => Response {
                status: 404,
                headers: Vec::new(),
                body: br#"{"error":"not found"}"#.to_vec(),
            },
        }
    }
}

/// The server's side of the socket: answers each `roomMap2` subscription with the portal
/// tiles of the room (`{}` without), except for `silent` rooms until [`Self::show`]; records
/// the rooms subscribed, in order.
struct Socket {
    websocket: Ws,
    tiles: HashMap<String, Vec<(u8, u8)>>,
    silent: HashSet<String>,
    subscribed: Vec<String>,
}

impl Socket {
    fn new(websocket: Ws, tiles: HashMap<String, Vec<(u8, u8)>>, silent: HashSet<String>) -> Self {
        Self {
            websocket,
            tiles,
            silent,
            subscribed: Vec::new(),
        }
    }

    async fn handle(&mut self, frame: &str) {
        if let Some(room) = frame.strip_prefix("subscribe roomMap2:") {
            self.subscribed.push(room.to_owned());
            if !self.silent.contains(room) {
                self.send(room).await;
            }
        }
    }

    async fn send(&mut self, room: &str) {
        let view = self.tiles.get(room).map_or_else(
            || "{}".to_owned(),
            |tiles| {
                let tiles = tiles
                    .iter()
                    .map(|(x, y)| format!("[{x},{y}]"))
                    .collect::<Vec<_>>();
                format!(r#"{{"p":[{}]}}"#, tiles.join(","))
            },
        );
        server::send(
            &mut self.websocket,
            &format!(r#"["roomMap2:{room}",{view}]"#),
        )
        .await;
    }

    /// Answers the subscription of a silent room now.
    async fn show(&mut self, room: &str) {
        self.silent.remove(room);
        self.send(room).await;
    }
}

/// Drives `next` and the socket (without letting the runtime idle) until `done` holds.
async fn drive(
    world: &mut WorldMap,
    socket: &mut Socket,
    mut done: impl FnMut(&WorldMap, &[WorldEvent]) -> bool,
) -> Vec<WorldEvent> {
    let start = std::time::Instant::now();
    let mut events = Vec::new();
    while !done(world, &events) {
        assert!(start.elapsed() < PATIENCE, "done in time");
        let mut frame = None;
        tokio::select! {
            biased;
            sent = server::receive(&mut socket.websocket, 1) => frame = sent.into_iter().next(),
            event = world.next() => events.push(event.unwrap()),
            () = tokio::task::yield_now() => {}
        }
        if let Some(frame) = frame {
            socket.handle(&frame).await;
        }
    }
    events
}

/// Drives `next` and the socket for a while without moving the clock.
async fn drive_for(world: &mut WorldMap, socket: &mut Socket) -> Vec<WorldEvent> {
    let until = std::time::Instant::now() + Duration::from_millis(300);
    drive(world, socket, |_, _| std::time::Instant::now() >= until).await
}

fn room_objects(server: &Server) -> Vec<String> {
    server
        .requests()
        .iter()
        .filter(|request| request.path == "/api/game/room-objects")
        .map(|request| query(request, "room").unwrap().to_owned())
        .collect()
}

/// Every room of the world that may have portals.
fn candidates(world: &WorldMap) -> Vec<RoomCoord> {
    world
        .bounds()
        .iter()
        .filter(|&room| overseer::map::may_have_portals(room))
        .collect()
}

/// A 100 × 100 world whose every candidate room (200) shows portal tiles (in 25 cluster shapes,
/// so fetches go in parallel) and has no portal objects, so each takes one room-objects
/// request: (server, world, socket).
async fn busy_world(limited: bool) -> (Server, overseer::Overseer, WorldMap, Socket) {
    let fake = Fake {
        size: 100,
        limited,
        objects: Arc::default(),
        refuse: Arc::default(),
    };
    busy_world_of(fake).await
}

async fn busy_world_of(fake: Fake) -> (Server, overseer::Overseer, WorldMap, Socket) {
    let (server, overseer, websocket) = connect(fake.routes()).await;
    let world = overseer.map().world(None).await.unwrap();
    let tiles = candidates(&world)
        .into_iter()
        .zip(0_usize..)
        .map(|(room, index)| {
            let shape = u8::try_from(index % 25).unwrap();
            (room.to_string(), vec![(20, 20), (21 + shape, 20)])
        })
        .collect();
    let socket = Socket::new(websocket, tiles, HashSet::new());
    (server, overseer, world, socket)
}

#[tokio::test]
async fn the_budget_counts_every_request_of_the_window_and_leaves_room_by_priority() {
    let (server, overseer, mut world, mut socket) = busy_world(true).await;
    tokio::time::pause();
    let client = overseer.client().clone();
    assert!(client.has_rate_limits());
    // 60 requests of the user first: they count, and never wait.
    for _ in 0..60 {
        overseer.map().world_size(None).await.unwrap();
    }
    let bounds = world.bounds();
    world.set_interest(&Interest {
        portals: Some(bounds),
        ..Interest::default()
    });
    let wanted = candidates(&world).len();
    assert!(wanted > Priority::Visible.budget());
    drive(&mut world, &mut socket, |_, _| {
        client.requests_in_window() >= Priority::Visible.budget()
    })
    .await;
    drive_for(&mut world, &mut socket).await;
    // Visible work stops at its share of the window, the user's requests included.
    assert_eq!(client.requests_in_window(), Priority::Visible.budget());
    let first = room_objects(&server).len();
    assert!(first < wanted);

    // A viewed room still gets its overview (the viewed share is larger).
    world.set_interest(&Interest {
        portals: Some(bounds),
        stats: Some(bounds),
        detail: vec![room("W1N1")],
        ..Interest::default()
    });
    let at = tokio::time::Instant::now();
    drive(&mut world, &mut socket, |_, _| {
        server
            .requests()
            .iter()
            .any(|request| request.path == "/api/game/map-stats")
    })
    .await;
    drive_for(&mut world, &mut socket).await;
    assert_eq!(room_objects(&server).len(), first);
    assert!(client.requests_in_window() <= Priority::Viewed.budget());

    // The user's requests go out at once beyond every share.
    for _ in 0..10 {
        overseer.map().world_size(None).await.unwrap();
    }
    assert_eq!(tokio::time::Instant::now(), at);
    assert!(client.requests_in_window() > Priority::Viewed.budget());

    // A minute later the window has room again.
    // (In steps the socket's keep-alive survives.)
    for _ in 0..6 {
        tokio::time::advance(REQUEST_WINDOW / 6).await;
        drive_for(&mut world, &mut socket).await;
    }
    tokio::time::advance(Duration::from_millis(1)).await;
    drive(&mut world, &mut socket, |_, _| {
        room_objects(&server).len() > first
    })
    .await;
    assert!(client.requests_in_window() <= Priority::Viewed.budget());
}

#[tokio::test]
async fn servers_without_rate_limits_have_no_budget() {
    let (server, overseer, mut world, mut socket) = busy_world(false).await;
    tokio::time::pause();
    assert!(!overseer.client().has_rate_limits());
    world.set_interest(&Interest {
        portals: Some(world.bounds()),
        ..Interest::default()
    });
    let wanted = candidates(&world).len();
    // The clock never moves.
    drive(&mut world, &mut socket, |_, _| {
        room_objects(&server).len() == wanted
    })
    .await;
    assert!(overseer.client().requests_in_window() > Priority::Viewed.budget());
}

#[tokio::test]
async fn a_server_wide_refusal_holds_the_budget_a_minute_and_lifted_limits_uncap_it() {
    let fake = Fake {
        size: 100,
        limited: true,
        objects: Arc::default(),
        refuse: Arc::new(AtomicBool::new(true)),
    };
    let (server, overseer, mut world, mut socket) = busy_world_of(fake).await;
    // The client retries the refused request once, a second later.
    tokio::time::pause();
    let client = overseer.client().clone();
    world.set_interest(&Interest {
        portals: Some(world.bounds()),
        ..Interest::default()
    });
    drive(&mut world, &mut socket, |_, _| {
        client.rate_limited_count() == 1
    })
    .await;
    drive_for(&mut world, &mut socket).await;
    tokio::time::advance(Duration::from_millis(1001)).await;
    drive_for(&mut world, &mut socket).await;
    // Only the requests already in flight (and the client's one retry) went out, and the
    // budget holds the rest for a minute.
    let held = room_objects(&server).len();
    assert!(held <= overseer::map::PORTAL_IN_FLIGHT + 1, "{held}");
    assert!(client.budget_ready_at(Priority::Viewed, 0).is_some());

    // Lifted: no budget at all, every room at once.
    let until = SystemTime::now() + Duration::from_hours(1);
    let until = u64::try_from(until.duration_since(UNIX_EPOCH).unwrap().as_millis()).unwrap();
    world.set_rate_limit_exempt_until(Some(until));
    assert!(client.budget_ready_at(Priority::Background, 1000).is_none());
    let wanted = candidates(&world).len();
    drive(&mut world, &mut socket, |_, _| {
        room_objects(&server).len() >= wanted
    })
    .await;
    assert!(client.requests_in_window() > Priority::Viewed.budget());
}

/// Portal objects by room.
type Objects = HashMap<String, Vec<String>>;

/// Portal tiles of the map views by room.
type Tiles = HashMap<String, Vec<(u8, u8)>>;

/// The pairs world: `W5N5` and `E5S5` paired exactly; `W5S5` leading next to `E5N5`'s portal
/// (the earlier `OpenScreeps` generator); `W15N15` accounting for one of `E15S15`'s two tiles;
/// `W15S15` leading to `E15N15` on another shard.
fn pairs() -> (Objects, Tiles) {
    let objects = HashMap::from([
        (
            "W5N5".to_owned(),
            vec![
                portal("W5N5", (20, 20), "E5S5", Some((30, 30)), None),
                portal("W5N5", (21, 20), "E5S5", Some((31, 30)), None),
            ],
        ),
        (
            "E5S5".to_owned(),
            vec![
                portal("E5S5", (30, 30), "W5N5", Some((20, 20)), None),
                portal("E5S5", (31, 30), "W5N5", Some((21, 20)), None),
            ],
        ),
        (
            "W5S5".to_owned(),
            vec![portal("W5S5", (10, 10), "E5N5", Some((12, 10)), None)],
        ),
        (
            "E5N5".to_owned(),
            vec![portal("E5N5", (10, 10), "W5S5", Some((12, 10)), None)],
        ),
        (
            "W15N15".to_owned(),
            vec![portal("W15N15", (5, 5), "E15S15", Some((6, 6)), None)],
        ),
        (
            "E15S15".to_owned(),
            vec![
                portal("E15S15", (6, 6), "W15N15", Some((5, 5)), None),
                portal("E15S15", (7, 7), "W15N15", Some((5, 5)), None),
            ],
        ),
        (
            "W15S15".to_owned(),
            vec![portal("W15S15", (5, 5), "E15N15", None, Some("shard1"))],
        ),
        (
            "E15N15".to_owned(),
            vec![portal("E15N15", (5, 5), "W15S15", None, Some("shard1"))],
        ),
    ]);
    let tiles = objects
        .iter()
        .map(|(room, portals)| {
            let tiles = portals
                .iter()
                .map(|portal| {
                    let value: Value = serde_json::from_str(portal).unwrap();
                    let at = |key: &str| u8::try_from(value[key].as_u64().unwrap()).unwrap();
                    (at("x"), at("y"))
                })
                .collect();
            (room.clone(), tiles)
        })
        .collect();
    (objects, tiles)
}

#[tokio::test]
async fn exact_pairs_are_inferred_and_nothing_else() {
    let (objects, tiles) = pairs();
    let fake = Fake {
        size: 40,
        limited: false,
        objects: Arc::new(objects),
        refuse: Arc::default(),
    };
    let (server, overseer, websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let later = ["E5S5", "E5N5", "E15S15", "E15N15"];
    let mut socket = Socket::new(
        websocket,
        tiles,
        later.iter().map(|&room| room.to_owned()).collect(),
    );
    world.set_interest(&Interest {
        portals: Some(world.bounds()),
        ..Interest::default()
    });
    // The first room of each pair is fetched, then the partners' map views arrive.
    let first = ["W5N5", "W5S5", "W15N15", "W15S15"];
    drive(&mut world, &mut socket, |world, _| {
        first
            .iter()
            .all(|&name| world.portals(room(name)).is_some())
    })
    .await;
    for name in later {
        socket.show(name).await;
    }
    drive(&mut world, &mut socket, |world, _| {
        later
            .iter()
            .all(|&name| world.portals(room(name)).is_some())
    })
    .await;
    drive_for(&mut world, &mut socket).await;

    // The exact pair: its partner is inferred, never fetched, leading back tile for tile.
    let asked = room_objects(&server);
    assert!(!asked.contains(&"E5S5".to_owned()), "{asked:?}");
    assert!(world.portals_inferred(room("E5S5")));
    let inferred = world.portals(room("E5S5")).unwrap();
    let leads = inferred
        .iter()
        .map(|portal| {
            (
                (portal.x, portal.y),
                portal.destination.room,
                (portal.destination.x, portal.destination.y),
                portal.expires_at,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        leads,
        [
            (
                (30, 30),
                room("W5N5"),
                (Some(20), Some(20)),
                Some(4_102_444_800_000)
            ),
            (
                (31, 30),
                room("W5N5"),
                (Some(21), Some(20)),
                Some(4_102_444_800_000)
            ),
        ]
    );
    // Landing next to a portal (OpenScreeps' earlier generator), partly accounted for, or
    // another shard: fetched.
    for name in ["E5N5", "E15S15", "E15N15"] {
        assert!(
            asked.contains(&name.to_owned()),
            "{name} not fetched: {asked:?}"
        );
        assert!(!world.portals_inferred(room(name)), "{name}");
    }
    assert_eq!(asked.len(), 7);

    // The live room replaces inferred portals, even when they agree.
    world.set_interest(&Interest {
        portals: Some(world.bounds()),
        detail: vec![room("E5S5")],
        ..Interest::default()
    });
    let objects = [
        portal("E5S5", (30, 30), "W5N5", Some((20, 20)), None),
        portal("E5S5", (31, 30), "W5N5", Some((21, 20)), None),
    ]
    .iter()
    .map(|portal| {
        let value: Value = serde_json::from_str(portal).unwrap();
        format!(r#""{}":{portal}"#, value["_id"].as_str().unwrap())
    })
    .collect::<Vec<_>>();
    let frame = format!(
        r#"["room:E5S5",{{"gameTime":9,"objects":{{{}}},"users":{{}}}}]"#,
        objects.join(",")
    );
    drive(&mut world, &mut socket, |world, _| {
        matches!(
            world.detail(room("E5S5")),
            Some(DetailState::Pending | DetailState::Live(_))
        )
    })
    .await;
    drive_for(&mut world, &mut socket).await;
    server::send(&mut socket.websocket, &frame).await;
    drive(&mut world, &mut socket, |world, _| {
        !world.portals_inferred(room("E5S5"))
    })
    .await;
    assert_eq!(world.portals(room("E5S5")).unwrap().len(), 2);
    assert_eq!(room_objects(&server).len(), 7);
}

#[tokio::test]
async fn partners_found_together_are_not_both_fetched() {
    let (objects, tiles) = pairs();
    let fake = Fake {
        size: 40,
        limited: true,
        objects: Arc::new(objects),
        refuse: Arc::default(),
    };
    let (server, overseer, websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let mut socket = Socket::new(websocket, tiles, HashSet::new());
    world.set_interest(&Interest {
        portals: Some(world.bounds()),
        ..Interest::default()
    });
    drive(&mut world, &mut socket, |world, _| {
        world.progress().portals_scanned == world.progress().portals_wanted
    })
    .await;
    drive_for(&mut world, &mut socket).await;
    // Both map views arrive at once; the partner with the same cluster shape waits for the
    // fetch of the nearer one, which tells its portals.
    let asked = room_objects(&server);
    assert!(asked.contains(&"W5N5".to_owned()), "{asked:?}");
    assert!(!asked.contains(&"E5S5".to_owned()), "{asked:?}");
    assert!(world.portals_inferred(room("E5S5")));
    assert_eq!(asked.len(), 7, "{asked:?}");
}

/// A 40 × 40 world with portals in a plain highway room (`W5N0`) and a room of no sector's
/// centre (`E3S3`), both leading to another shard.
fn unusual() -> Fake {
    Fake {
        size: 40,
        limited: false,
        objects: Arc::new(HashMap::from([
            (
                "W5N0".to_owned(),
                vec![portal("W5N0", (10, 10), "W5N0", None, Some("shard1"))],
            ),
            (
                "E3S3".to_owned(),
                vec![portal("E3S3", (30, 30), "E3S3", None, Some("shard2"))],
            ),
        ])),
        refuse: Arc::default(),
    }
}

fn unusual_tiles() -> Tiles {
    HashMap::from([
        ("W5N0".to_owned(), vec![(10, 10)]),
        ("E3S3".to_owned(), vec![(30, 30)]),
    ])
}

#[tokio::test]
async fn portals_anywhere_are_found_at_full_zoom_out() {
    let fake = unusual();
    let (server, overseer, websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let mut socket = Socket::new(websocket, unusual_tiles(), HashSet::new());
    let bounds = world.bounds();
    // The camera at its furthest: the whole world in view, no minimap, no live room.
    world.set_interest(&Interest {
        stats: Some(bounds),
        portals: Some(bounds),
        terrain: bounds.iter().collect(),
        ..Interest::default()
    });
    drive(&mut world, &mut socket, |world, _| {
        let progress = world.progress();
        progress.portals_wanted == bounds.len() && progress.portals_scanned == bounds.len()
    })
    .await;
    for name in ["W5N0", "E3S3"] {
        let portals = world.portals(room(name)).unwrap();
        assert_eq!(portals.len(), 1, "{name}");
        assert!(portals[0].destination.shard.is_some(), "{name}");
    }
    let mut asked = room_objects(&server);
    asked.sort();
    assert_eq!(asked, ["E3S3", "W5N0"]);
    // Each room once; the usual portal rooms (sector centres and highways) first.
    let scanned = &socket.subscribed;
    assert_eq!(scanned.len(), bounds.len());
    let usual = scanned
        .iter()
        .filter(|name| overseer::map::may_have_portals(room(name)))
        .count();
    assert!(
        scanned[..usual]
            .iter()
            .all(|name| overseer::map::may_have_portals(room(name)))
    );
}

#[tokio::test]
async fn background_scans_take_the_usual_portal_rooms_first_then_the_rest() {
    let fake = unusual();
    let (_server, overseer, websocket) = connect(fake.routes()).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let mut socket = Socket::new(websocket, unusual_tiles(), HashSet::new());
    let bounds = world.bounds();
    // Zoomed in on W12N12: its view and ring are visible, the rest of the world background.
    let view = RoomRect::new(room("W12N12"), room("W12N12"));
    world.set_interest(&Interest {
        portals: Some(view),
        terrain: vec![room("W12N12")],
        ..Interest::default()
    });
    drive(&mut world, &mut socket, |world, _| {
        bounds.iter().all(|room| world.portals(room).is_some())
    })
    .await;
    let ring = RoomRect::new(room("W15N15"), room("W9N9"));
    let order = socket.subscribed.clone();
    assert_eq!(order.len(), bounds.len());
    // The visible rooms (the view and its ring) first, then the background's usual rooms
    // (sector centres and highways), then the others.
    let visible = order
        .iter()
        .take_while(|name| ring.contains(room(name)))
        .count();
    assert_eq!(visible, ring.len());
    let background = &order[visible..];
    let usual = background
        .iter()
        .filter(|name| overseer::map::may_have_portals(room(name)))
        .count();
    assert!(
        background[..usual]
            .iter()
            .all(|name| overseer::map::may_have_portals(room(name)))
    );
    // Fresh rooms are not scanned again.
    world.set_interest(&Interest {
        portals: Some(bounds),
        ..Interest::default()
    });
    drive_for(&mut world, &mut socket).await;
    assert_eq!(socket.subscribed.len(), bounds.len());
}
