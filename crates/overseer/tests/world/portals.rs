//! Portals on the world map: candidate rooms scanned nearest first within the subscription cap,
//! destinations of the rooms with portals only (intra- and inter-shard), no rescans, seeding,
//! expiry, live minimaps counting as scans, timed-out scans retried.
//!
//! Tests on paused time drive [`WorldMap::next`] by yielding (see [`drive`]): a runtime that
//! never idles never advances paused time on its own, so only `tokio::time::advance` moves it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use overseer::map::{
    Interest, MINIMAP_CAP, PORTAL_GAP, PORTAL_SCAN_TIMEOUT, PORTAL_TTL, Portal, PortalDestination,
    RoomCoord, RoomRect, WorldEvent, WorldMap, may_have_portals,
};
use serde_json::Value;

use crate::server::{self, Request, Server, Ws};
use crate::{PATIENCE, connect, query, terrain_document};

fn room(name: &str) -> RoomCoord {
    name.parse().unwrap()
}

fn rect(a: &str, b: &str) -> RoomRect {
    RoomRect::new(room(a), room(b))
}

/// Rooms whose map view shows portals; the server's room objects hold them.
const PORTAL_ROOMS: [&str; 4] = ["W0N0", "E5S5", "W0S0", "W1N1"];

/// The room objects of `room`: the portals of [`PORTAL_ROOMS`], none elsewhere.
fn objects(room: &str) -> String {
    let objects = match room {
        // Inter-shard: the destination names only the shard and room.
        "W0N0" => vec![
            r#"{"_id":"a","type":"portal","room":"W0N0","x":20,"y":21,"destination":{"room":"W0N0","shard":"shard1"}}"#,
        ],
        // Intra-shard, unstable from a date on.
        "E5S5" => vec![
            r#"{"_id":"b","type":"portal","room":"E5S5","x":30,"y":31,"unstableDate":4102444800000,"destination":{"room":"W15N15","x":10,"y":11}}"#,
            r#"{"_id":"c","type":"portal","room":"E5S5","x":30,"y":32,"unstableDate":4102444800000,"destination":{"room":"W15N15","x":10,"y":12}}"#,
        ],
        "W0S0" => vec![
            r#"{"_id":"d","type":"portal","room":"W0S0","x":5,"y":6,"destination":{"room":"E5S5","x":1,"y":2}}"#,
        ],
        "W1N1" => vec![
            r#"{"_id":"e","type":"portal","room":"W1N1","x":7,"y":8,"destination":{"room":"E5S5","x":3,"y":4}}"#,
        ],
        _ => Vec::new(),
    };
    format!(
        r#"{{"ok":1,"objects":[{}],"users":{{}}}}"#,
        objects.join(",")
    )
}

/// A 40 × 40 world (`W19N19`..`E19S19`) and the room objects of [`objects`].
fn routes(request: &Request) -> (u16, String) {
    match request.path.as_str() {
        "/api/game/world-size" => (200, r#"{"ok":1,"width":40,"height":40}"#.to_owned()),
        "/api/game/rooms" => {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            let documents = body["rooms"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .map(terrain_document)
                .collect::<Vec<_>>();
            (
                200,
                format!(r#"{{"ok":1,"rooms":[{}]}}"#, documents.join(",")),
            )
        }
        "/api/game/room-objects" => (200, objects(query(request, "room").unwrap())),
        _ => (404, r#"{"error":"not found"}"#.to_owned()),
    }
}

/// The rooms of each `GET /api/game/room-objects` so far.
fn room_objects(server: &Server) -> Vec<String> {
    server
        .requests()
        .iter()
        .filter(|request| request.path == "/api/game/room-objects")
        .map(|request| query(request, "room").unwrap().to_owned())
        .collect()
}

/// Now, in milliseconds since the Unix epoch.
fn epoch_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// The server's side of the socket: answers each `roomMap2` subscription at once (a map view
/// listing a portal for [`PORTAL_ROOMS`], an empty one elsewhere) unless the room is `silent`,
/// and records the subscriptions.
struct Socket {
    websocket: Ws,
    silent: BTreeSet<String>,
    /// Rooms subscribed now.
    live: BTreeSet<String>,
    /// Every subscription, in order.
    subscribed: Vec<String>,
    /// Most rooms subscribed at once.
    most: usize,
}

impl Socket {
    fn new(websocket: Ws, silent: impl IntoIterator<Item = RoomCoord>) -> Self {
        Self {
            websocket,
            silent: silent.into_iter().map(|room| room.to_string()).collect(),
            live: BTreeSet::new(),
            subscribed: Vec::new(),
            most: 0,
        }
    }

    async fn handle(&mut self, frame: &str) {
        if let Some(room) = frame.strip_prefix("subscribe roomMap2:") {
            self.live.insert(room.to_owned());
            self.subscribed.push(room.to_owned());
            self.most = self.most.max(self.live.len());
            if !self.silent.contains(room) {
                self.send(room).await;
            }
        } else if let Some(room) = frame.strip_prefix("unsubscribe roomMap2:") {
            assert!(self.live.remove(room), "{room} was not subscribed");
        }
    }

    /// Sends the map view of `room`.
    async fn send(&mut self, room: &str) {
        let view = if PORTAL_ROOMS.contains(&room) {
            r#"{"p":[[20,21]]}"#
        } else {
            "{}"
        };
        server::send(
            &mut self.websocket,
            &format!(r#"["roomMap2:{room}",{view}]"#),
        )
        .await;
    }

    /// The subscriptions of rooms that are not silent.
    fn scanned(&self) -> Vec<&str> {
        self.subscribed
            .iter()
            .filter(|room| !self.silent.contains(*room))
            .map(String::as_str)
            .collect()
    }
}

/// Drives `next` and the socket (without letting the runtime idle) until `done` holds for the
/// events so far, which it returns.
async fn drive(
    world: &mut WorldMap,
    socket: &mut Socket,
    mut done: impl FnMut(&[WorldEvent]) -> bool,
) -> Vec<WorldEvent> {
    let start = std::time::Instant::now();
    let mut events = Vec::new();
    while !done(&events) {
        assert!(
            start.elapsed() < PATIENCE,
            "done in time; so far {events:?}, subscribed {:?}",
            socket.subscribed
        );
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

/// Drives `next` and the socket for a while without moving the clock; returns the events.
async fn drive_for(world: &mut WorldMap, socket: &mut Socket) -> Vec<WorldEvent> {
    let until = std::time::Instant::now() + Duration::from_millis(300);
    drive(world, socket, |_| std::time::Instant::now() >= until).await
}

/// Moves the paused clock by `duration` once nothing more happens; fails on portal events
/// and new subscriptions meanwhile.
async fn advance(world: &mut WorldMap, socket: &mut Socket, duration: Duration) {
    let before = socket.subscribed.len();
    let events = drive_for(world, socket).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, WorldEvent::Portals(..))),
        "unexpected {events:?}"
    );
    assert_eq!(socket.subscribed[before..], [] as [String; 0]);
    tokio::time::advance(duration).await;
}

/// The portal events among `events`.
fn portals(events: &[WorldEvent]) -> Vec<(RoomCoord, Arc<[Portal]>)> {
    events
        .iter()
        .filter_map(|event| match event {
            WorldEvent::Portals(room, portals) => Some((*room, Arc::clone(portals))),
            _ => None,
        })
        .collect()
}

fn count(events: &[WorldEvent]) -> usize {
    portals(events).len()
}

/// A portal of `room` leading to `E5S5`.
fn portal(room_name: &str, expires_at: Option<u64>) -> Portal {
    Portal {
        room: room(room_name),
        x: 5,
        y: 6,
        destination: PortalDestination {
            room: room("E5S5"),
            shard: None,
            x: Some(1),
            y: Some(2),
        },
        expires_at,
        decay_time: None,
    }
}

#[tokio::test]
async fn scans_take_the_nearest_candidates_within_the_cap_and_fetch_only_rooms_with_portals() {
    let (server, overseer, websocket) = connect(routes).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let bounds = world.bounds();
    assert_eq!(bounds, rect("W19N19", "E19S19"));
    let candidates = bounds
        .iter()
        .filter(|&room| may_have_portals(room))
        .map(|room| room.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(candidates.len(), 32);
    // Live minimaps (never answered) leave room for 3 scans.
    let minimaps = bounds
        .iter()
        .filter(|&room| !may_have_portals(room))
        .take(MINIMAP_CAP - 3)
        .collect::<Vec<_>>();
    let mut socket = Socket::new(websocket, minimaps.iter().copied());
    let interest = Interest {
        portals: Some(rect("W30N30", "E30S30")),
        minimaps: minimaps.clone(),
        ..Interest::default()
    };
    world.set_interest(&interest);
    let progress = world.progress();
    assert_eq!((progress.portals_scanned, progress.portals_wanted), (0, 32));
    assert!(!progress.is_complete());

    let start = std::time::Instant::now();
    let events = drive(&mut world, &mut socket, |events| count(events) == 32).await;
    // Every candidate once, nearest the centre (W0N0) first, never more than the cap.
    let scanned = socket.scanned();
    assert_eq!(scanned[..3], ["W0N0", "E0N0", "W0S0"]);
    assert_eq!(scanned.len(), 32);
    assert_eq!(
        scanned
            .iter()
            .map(|&room| room.to_owned())
            .collect::<BTreeSet<_>>(),
        candidates
    );
    assert_eq!(socket.most, MINIMAP_CAP);
    assert_eq!(count(&drive_for(&mut world, &mut socket).await), 0);
    assert_eq!(socket.live.len(), MINIMAP_CAP - 3, "scans are released");
    // One room-objects request per room with portals, nearest first, paced.
    assert_eq!(room_objects(&server), ["W0N0", "W0S0", "E5S5"]);
    assert!(start.elapsed() >= 2 * PORTAL_GAP);

    let found = portals(&events);
    let with_portals = found
        .iter()
        .filter(|(_, portals)| !portals.is_empty())
        .count();
    assert_eq!(with_portals, 3);
    let inter_shard = world.portals(room("W0N0")).unwrap();
    assert_eq!(
        *inter_shard,
        [Portal {
            room: room("W0N0"),
            x: 20,
            y: 21,
            destination: PortalDestination {
                room: room("W0N0"),
                shard: Some("shard1".to_owned()),
                x: None,
                y: None,
            },
            expires_at: None,
            decay_time: None,
        }]
    );
    let intra_shard = world.portals(room("E5S5")).unwrap();
    assert_eq!(intra_shard.len(), 2);
    assert_eq!(
        intra_shard[1].destination,
        PortalDestination {
            room: room("W15N15"),
            shard: None,
            x: Some(10),
            y: Some(12),
        }
    );
    assert_eq!(intra_shard[0].expires_at, Some(4_102_444_800_000));
    assert_eq!(
        world.portals(room("E0N0")).as_deref(),
        Some(&[] as &[Portal])
    );
    assert!(world.portals(room("W1N2")).is_none());
    let progress = world.progress();
    assert_eq!(
        (progress.portals_scanned, progress.portals_wanted),
        (32, 32)
    );
    assert!(progress.is_complete());

    // Known rooms are not scanned again, whatever the interest does.
    world.set_interest(&Interest {
        portals: Some(rect("W19N19", "W10N10")),
        ..Interest::default()
    });
    world.set_interest(&interest);
    let events = drive_for(&mut world, &mut socket).await;
    assert_eq!(count(&events), 0);
    assert_eq!(socket.scanned().len(), 32);
    assert_eq!(room_objects(&server).len(), 3);
}

/// Paused time moves by seconds only: the socket's keep-alive (30 s) would drop a connection
/// whose clock jumps further. Seeds are therefore dated so that they go stale within seconds;
/// the lifetimes themselves are unit-tested in the library.
#[tokio::test]
async fn seeded_scans_are_used_until_stale_or_a_portal_becomes_unstable() {
    let (server, overseer, websocket) = connect(routes).await;
    let mut world = overseer.map().world(None).await.unwrap();
    let mut socket = Socket::new(websocket, []);
    tokio::time::pause();
    let now = epoch_millis();
    let hour = 3_600_000;
    let ttl = u64::try_from(PORTAL_TTL.as_millis()).unwrap();
    // A day old in 20 s.
    let seeded: Arc<[Portal]> = Arc::from([portal("W0N0", None)]);
    world.insert_portals(room("W0N0"), Arc::clone(&seeded), now - ttl + 20_000);
    // Stale.
    world.insert_portals(room("E0N0"), Arc::from([]), now - 25 * hour);
    // An older scan than the one known is ignored.
    world.insert_portals(
        room("E0N0"),
        Arc::from([portal("E0N0", None)]),
        now - 26 * hour,
    );
    // Unstable in 10 s.
    world.insert_portals(
        room("W0S0"),
        Arc::from([portal("W0S0", Some(now + 10_000))]),
        now,
    );
    assert_eq!(world.portals(room("W0N0")), Some(seeded));
    assert_eq!(world.portals(room("E0N0")).unwrap().len(), 0);

    world.set_interest(&Interest {
        portals: Some(rect("W0N0", "E0S0")),
        ..Interest::default()
    });
    let progress = world.progress();
    assert_eq!((progress.portals_scanned, progress.portals_wanted), (3, 4));
    // Only the stale and unknown rooms are scanned; seeds are not reported.
    let events = drive(&mut world, &mut socket, |events| count(events) == 2).await;
    let mut found = portals(&events)
        .into_iter()
        .map(|(room, portals)| (room.to_string(), portals.len()))
        .collect::<Vec<_>>();
    found.sort();
    assert_eq!(found, [("E0N0".to_owned(), 0), ("E0S0".to_owned(), 0)]);
    assert_eq!(
        socket.scanned().into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from(["E0N0", "E0S0"])
    );
    assert_eq!(room_objects(&server), [] as [String; 0]);
    assert!(world.progress().is_complete());

    // The unstable portal's room once it becomes unstable.
    advance(&mut world, &mut socket, Duration::from_secs(9)).await;
    advance(&mut world, &mut socket, Duration::from_secs(2)).await;
    let events = drive(&mut world, &mut socket, |events| count(events) == 1).await;
    assert_eq!(socket.scanned()[2..], ["W0S0"]);
    assert_eq!(room_objects(&server), ["W0S0"]);
    let (room_name, portals) = portals(&events).remove(0);
    assert_eq!((room_name, portals[0].expires_at), (room("W0S0"), None));

    // The day-old seed once a day old.
    advance(&mut world, &mut socket, Duration::from_secs(8)).await;
    advance(&mut world, &mut socket, Duration::from_secs(2)).await;
    drive(&mut world, &mut socket, |events| count(events) == 1).await;
    assert_eq!(socket.scanned()[3..], ["W0N0"]);
    assert_eq!(room_objects(&server), ["W0S0", "W0N0"]);
    // Fresh scans are not repeated.
    advance(&mut world, &mut socket, Duration::from_secs(20)).await;
    assert_eq!(socket.scanned().len(), 4);
}

#[tokio::test]
async fn live_minimaps_count_as_scans_and_unanswered_scans_are_retried() {
    let (server, overseer, websocket) = connect(routes).await;
    let mut world = overseer.map().world(None).await.unwrap();
    // Minimaps are answered by hand; E0S0's scan goes unanswered at first.
    let mut socket = Socket::new(websocket, [room("W0N0"), room("W1N1"), room("E0S0")]);
    tokio::time::pause();
    world.set_interest(&Interest {
        portals: Some(rect("W2N2", "E1S1")),
        minimaps: vec![room("W0N0"), room("W1N1")],
        ..Interest::default()
    });
    // W0N0's scan shares its minimap's subscription.
    let events = drive(&mut world, &mut socket, |events| count(events) == 2).await;
    assert_eq!(
        socket
            .subscribed
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["W0N0", "W1N1", "E0N0", "W0S0", "E0S0"])
    );
    assert_eq!(socket.subscribed.len(), 5);
    let mut found = portals(&events)
        .into_iter()
        .map(|(room, portals)| (room.to_string(), portals.len()))
        .collect::<Vec<_>>();
    found.sort();
    assert_eq!(found, [("E0N0".to_owned(), 0), ("W0S0".to_owned(), 1)]);

    // A candidate's minimap answers its scan; a minimap showing portals in any other room of
    // the interest brings its destinations too.
    socket.send("W0N0").await;
    socket.send("W1N1").await;
    let events = drive(&mut world, &mut socket, |events| {
        events
            .iter()
            .filter(|event| matches!(event, WorldEvent::Minimap(..)))
            .count()
            == 2
    })
    .await;
    assert_eq!(count(&events), 0);
    tokio::time::advance(PORTAL_GAP).await;
    let mut events = drive(&mut world, &mut socket, |events| count(events) >= 1).await;
    tokio::time::advance(PORTAL_GAP).await;
    events.extend(drive(&mut world, &mut socket, |events| count(events) == 1).await);
    let fetched = portals(&events)
        .into_iter()
        .map(|(room, portals)| (room.to_string(), portals[0].destination.room))
        .collect::<Vec<_>>();
    assert_eq!(
        fetched,
        [
            ("W0N0".to_owned(), room("W0N0")),
            ("W1N1".to_owned(), room("E5S5"))
        ]
    );
    assert_eq!(room_objects(&server), ["W0S0", "W0N0", "W1N1"]);
    // Further minimap updates change nothing.
    socket.send("W1N1").await;
    let events = drive_for(&mut world, &mut socket).await;
    assert_eq!(count(&events), 0);

    // The unanswered scan is released after its timeout and tried again 30 s later (the clock
    // moves by 30 s at most, within the socket's keep-alive).
    assert!(socket.live.contains("E0S0"));
    tokio::time::advance(PORTAL_SCAN_TIMEOUT).await;
    let before = socket.subscribed.len();
    let start = std::time::Instant::now();
    while socket.live.contains("E0S0") {
        assert!(start.elapsed() < PATIENCE, "released in time");
        drive_for(&mut world, &mut socket).await;
    }
    socket.silent.remove("E0S0");
    // Timers fire on the millisecond after their deadline.
    for step in [20, 11] {
        advance(&mut world, &mut socket, Duration::from_secs(step)).await;
    }
    let events = drive(&mut world, &mut socket, |events| count(events) == 1).await;
    assert_eq!(socket.subscribed[before..], ["E0S0"]);
    assert_eq!(portals(&events)[0].0, room("E0S0"));
    let progress = world.progress();
    assert_eq!((progress.portals_scanned, progress.portals_wanted), (4, 4));
    assert_eq!(room_objects(&server).len(), 3);
}
