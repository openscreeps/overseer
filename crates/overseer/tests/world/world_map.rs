//! The world map: bounds (measured and known), interest diffing, whole-world terrain batching,
//! absent rooms and seeding, progress, overview near/far lifetimes, seeding and budget split,
//! rate-limit pauses and rechecks, overview requests in flight (cap, budget, order, 429),
//! minimap cap, detail priority, refusals (stale ones behind
//! barriers) and terrain reuse, cancel safety.
//!
//! Tests on paused time drive [`WorldMap::next`] by yielding (see [`event`]): a runtime that
//! never idles never advances paused time on its own, so only `tokio::time::advance` moves it.
//!
//! This module holds the fake server and the helpers that drive the world map. The child
//! modules hold the tests, one topic each.

use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use overseer::map::{RoomCoord, RoomRect, RoomStat, WorldEvent, WorldMap};
use serde_json::Value;

use crate::server::{self, Request, Response, Server, Ws};
use crate::{PATIENCE, query, terrain_document};

mod bounds;
mod detail;
mod in_flight;
mod minimaps;
mod overviews;
mod rate_limits;
mod terrain;

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
/// while it has entries. A `limited` server shows rate limits (`X-RateLimit-*` on its
/// world-size reply), so the world map paces its requests as on screeps.com.
#[derive(Clone)]
struct Fake {
    size: i64,
    rooms: RoomRect,
    limited: bool,
    level: Arc<AtomicI64>,
    scripted: Arc<Mutex<VecDeque<Scripted>>>,
}

impl Fake {
    fn new(size: i64, rooms: RoomRect) -> Self {
        Self {
            size,
            rooms,
            limited: false,
            level: Arc::new(AtomicI64::new(3)),
            scripted: Arc::default(),
        }
    }

    /// A server showing rate limits.
    fn limited(size: i64, rooms: RoomRect) -> Self {
        Self {
            limited: true,
            ..Self::new(size, rooms)
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
            "/api/game/world-size" => Response {
                status: 200,
                headers: if self.limited {
                    rate_limit(120, 119, Duration::from_mins(1))
                } else {
                    Vec::new()
                },
                body: format!(r#"{{"ok":1,"width":{0},"height":{0}}}"#, self.size).into_bytes(),
            },
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

/// Drives `next` (without letting the runtime idle) until `done` holds; returns the events.
async fn drive_until(
    world: &mut WorldMap,
    mut done: impl FnMut(&WorldMap) -> bool,
) -> Vec<WorldEvent> {
    let start = std::time::Instant::now();
    let mut events = Vec::new();
    while !done(world) {
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
    drive_until(world, |_| std::time::Instant::now() >= until).await
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

async fn update(websocket: &mut Ws, room: RoomCoord, time: u64) {
    let frame = format!(r#"["room:{room}",{{"gameTime":{time},"objects":{{}},"users":{{}}}}]"#);
    server::send(websocket, &frame).await;
}

async fn refuse(websocket: &mut Ws, room: RoomCoord) {
    let frame = format!(r#"["err@room:{room}","subscribe limit reached"]"#);
    server::send(websocket, &frame).await;
}
