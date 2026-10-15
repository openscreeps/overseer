//! Terrain of the world map: batched, deduplicated, nearest first, kept for the session.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use overseer_net::core::api::Terrain;
use overseer_net::rt::{BoxFuture, Instant};

use super::WorldEvent;
use crate::map::{Map, RoomCoord};
use crate::{Closed, Error};

/// Fewest rooms per `POST /api/game/rooms` (the official world map's `MIN_ROOMS_QUERY`).
const BATCH: usize = 600;

/// Requests the rooms wanted at once are split into, at most, unless that makes batches smaller
/// than [`BATCH`] (the official world map's `MAX_ROOMS_QUERIES`).
const REQUESTS: usize = 3;

/// Wait after a failed request before the rooms are asked for again.
const RETRY: Duration = Duration::from_secs(10);

/// The batch in flight: its rooms, nearest first, and the request.
struct Fetch {
    rooms: Vec<RoomCoord>,
    request: BoxFuture<'static, Result<BTreeMap<String, Arc<Terrain>>, Error>>,
}

/// The terrain known, wanted and being fetched.
#[derive(Default)]
pub(super) struct Terrains {
    /// Every terrain fetched or seeded.
    pub(super) known: HashMap<RoomCoord, Arc<Terrain>>,
    /// Rooms the server does not know.
    absent: HashSet<RoomCoord>,
    /// The rooms of the latest interest.
    wanted: HashSet<RoomCoord>,
    /// Wanted rooms neither known nor in flight, nearest first.
    queue: VecDeque<RoomCoord>,
    /// Rooms per request for the current interest.
    batch: usize,
    fetch: Option<Fetch>,
    /// No request before this, after a failure.
    retry_at: Option<Instant>,
}

impl Terrains {
    /// Makes `rooms` (nearest first) the wanted rooms: queues those neither known nor in
    /// flight and sizes the batches so they take at most [`REQUESTS`] requests.
    pub(super) fn want(&mut self, rooms: &[RoomCoord]) {
        let in_flight = self
            .fetch
            .as_ref()
            .map(|fetch| fetch.rooms.as_slice())
            .unwrap_or_default();
        self.wanted.clear();
        self.queue.clear();
        for &room in rooms {
            if self.wanted.insert(room)
                && !self.known.contains_key(&room)
                && !self.absent.contains(&room)
                && !in_flight.contains(&room)
            {
                self.queue.push_back(room);
            }
        }
        self.batch = BATCH.max(self.queue.len().div_ceil(REQUESTS));
    }

    /// Records a terrain the app already has: it is never requested.
    pub(super) fn insert(&mut self, room: RoomCoord, terrain: Arc<Terrain>) {
        self.known.insert(room, terrain);
        self.queue.retain(|&queued| queued != room);
    }

    /// Whether the server does not know `room`.
    pub(super) fn is_absent(&self, room: RoomCoord) -> bool {
        self.absent.contains(&room)
    }

    /// Requests the next batch unless one is in flight; returns when to plan again while a
    /// failure holds requests back.
    pub(super) fn plan(&mut self, map: &Map, shard: Option<&str>, now: Instant) -> Option<Instant> {
        if self.fetch.is_some() || self.queue.is_empty() {
            return None;
        }
        if let Some(retry_at) = self.retry_at.filter(|&at| at > now) {
            return Some(retry_at);
        }
        let count = self.batch.clamp(1, self.queue.len());
        let rooms = self.queue.drain(..count).collect::<Vec<_>>();
        let names = rooms.iter().map(ToString::to_string).collect::<Vec<_>>();
        let (map, shard) = (map.clone(), shard.map(str::to_owned));
        let request = Box::pin(async move { map.terrain(names, shard.as_deref()).await });
        self.fetch = Some(Fetch { rooms, request });
        None
    }

    /// Completes the batch in flight: queues a [`WorldEvent::Terrain`] per new terrain and
    /// remembers the rooms the server does not know. Returns whether anything changed.
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        events: &mut VecDeque<WorldEvent>,
    ) -> Result<bool, Closed> {
        let Some(fetch) = &mut self.fetch else {
            return Ok(false);
        };
        let Poll::Ready(result) = fetch.request.as_mut().poll(cx) else {
            return Ok(false);
        };
        let rooms = std::mem::take(&mut fetch.rooms);
        self.fetch = None;
        match result {
            Ok(mut terrains) => {
                self.retry_at = None;
                for room in rooms {
                    match terrains.remove(&room.to_string()) {
                        Some(terrain) => {
                            if let Entry::Vacant(entry) = self.known.entry(room) {
                                entry.insert(Arc::clone(&terrain));
                                events.push_back(WorldEvent::Terrain(room, terrain));
                            }
                        }
                        None => {
                            self.absent.insert(room);
                        }
                    }
                }
            }
            Err(Error::Closed) => return Err(Closed),
            Err(_) => {
                for room in rooms.into_iter().rev() {
                    if self.wanted.contains(&room) && !self.known.contains_key(&room) {
                        self.queue.push_front(room);
                    }
                }
                self.retry_at = Some(Instant::now() + RETRY);
            }
        }
        Ok(true)
    }
}
