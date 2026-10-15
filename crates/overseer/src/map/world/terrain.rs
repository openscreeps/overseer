//! Terrain of the world map: batched, deduplicated, nearest first, kept for the session.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use overseer_net::core::api::{ApiError, Terrain, endpoints};
use overseer_net::rt::{BoxFuture, Instant, SystemTime, UNIX_EPOCH};
use overseer_net::{Client, Error as NetError};

use super::WorldEvent;
use crate::map::{Map, RoomCoord};
use crate::{Closed, Error};

/// Rooms per terrain request (`POST /api/game/rooms`).
///
/// Measured on screeps.com `shard0` (October 2026): a room's terrain is about 2 580 bytes of
/// JSON, 370 bytes gzipped on the wire. 100 rooms took 0.68 s, 500 1.06 s, 1 000 1.33 s,
/// 2 000 1.52 s and 4 000 2.85 s: past 2 000 rooms the time per room levels off (0.76 ms,
/// 0.71 ms at 4 000) while each response grows by 2.6 MB of JSON to decode. A whole
/// 182 × 182 shard takes 17 requests.
pub const TERRAIN_BATCH: usize = 2000;

/// Terrain requests in flight at once: one decodes while the next downloads.
pub const TERRAIN_IN_FLIGHT: usize = 2;

/// Wait after a failed request before its rooms are asked for again.
const RETRY: Duration = Duration::from_secs(10);

/// Wait after HTTP 429 without the endpoint's limit (the server-wide limit counts per minute).
const GLOBAL_RETRY: Duration = Duration::from_secs(60);

/// Shortest wait after HTTP 429 with the endpoint's limit.
const MIN_LIMIT_WAIT: Duration = Duration::from_secs(1);

/// A request in flight: its rooms, nearest first, and the request.
struct Fetch {
    rooms: Vec<RoomCoord>,
    request: BoxFuture<'static, Result<BTreeMap<String, Arc<Terrain>>, Error>>,
}

/// The terrain known, wanted and being fetched.
#[derive(Default)]
pub(super) struct Terrains {
    /// Every terrain fetched or seeded.
    pub(super) known: HashMap<RoomCoord, Arc<Terrain>>,
    /// Rooms the server has no terrain for (fetched or seeded).
    absent: HashSet<RoomCoord>,
    /// The rooms of the latest interest.
    wanted: HashSet<RoomCoord>,
    /// Wanted rooms not requested yet, nearest first. Rooms seeded meanwhile are skipped when
    /// a batch is taken, so seeding never scans it.
    queue: VecDeque<RoomCoord>,
    /// The rooms of the requests in flight.
    pending: HashSet<RoomCoord>,
    fetches: Vec<Fetch>,
    /// No request before this, after a failure or HTTP 429.
    not_before: Option<Instant>,
    /// Wanted rooms whose terrain is known.
    wanted_known: usize,
    /// Wanted rooms the server has no terrain for.
    wanted_absent: usize,
}

impl Terrains {
    /// Makes `rooms` (nearest first) the wanted rooms and queues those neither known, absent
    /// nor in flight.
    pub(super) fn want(&mut self, rooms: impl IntoIterator<Item = RoomCoord>) {
        self.wanted.clear();
        self.queue.clear();
        self.wanted_known = 0;
        self.wanted_absent = 0;
        for room in rooms {
            if !self.wanted.insert(room) {
                continue;
            }
            if self.known.contains_key(&room) {
                self.wanted_known += 1;
            } else if self.absent.contains(&room) {
                self.wanted_absent += 1;
            } else if !self.pending.contains(&room) {
                self.queue.push_back(room);
            }
        }
    }

    /// Records a terrain the app already has: it is never requested.
    pub(super) fn insert(&mut self, room: RoomCoord, terrain: Arc<Terrain>) {
        if self.known.insert(room, terrain).is_none() {
            let wanted = usize::from(self.wanted.contains(&room));
            self.wanted_known += wanted;
            if self.absent.remove(&room) {
                self.wanted_absent -= wanted;
            }
        }
    }

    /// Records that the server has no terrain for `room` (the app's cache says so): it is
    /// never requested. Ignored when its terrain is known.
    pub(super) fn insert_absent(&mut self, room: RoomCoord) {
        if !self.known.contains_key(&room)
            && self.absent.insert(room)
            && self.wanted.contains(&room)
        {
            self.wanted_absent += 1;
        }
    }

    /// Whether the server has no terrain for `room`.
    pub(super) fn is_absent(&self, room: RoomCoord) -> bool {
        self.absent.contains(&room)
    }

    /// The wanted rooms: with a known terrain, without one on the server, and in total.
    pub(super) fn progress(&self) -> (usize, usize, usize) {
        (self.wanted_known, self.wanted_absent, self.wanted.len())
    }

    /// Starts batches until [`TERRAIN_IN_FLIGHT`] are in flight; returns when to plan again
    /// while a failure or the rate limit holds requests back.
    pub(super) fn plan(
        &mut self,
        map: &Map,
        client: &Client,
        shard: Option<&str>,
        now: Instant,
    ) -> Option<Instant> {
        if self.fetches.len() >= TERRAIN_IN_FLIGHT || self.queue.is_empty() {
            return None;
        }
        let mut ready = self.not_before.filter(|&at| at > now);
        if let Some(limit) = client.rate_limit::<endpoints::GameRooms>()
            && limit.remaining == 0
        {
            let reset = reset_at(limit.reset, now);
            ready = Some(ready.map_or(reset, |at| at.max(reset)));
        }
        if let Some(ready) = ready.filter(|&at| at > now) {
            return Some(ready);
        }
        self.not_before = None;
        while self.fetches.len() < TERRAIN_IN_FLIGHT {
            let mut rooms = Vec::with_capacity(TERRAIN_BATCH.min(self.queue.len()));
            while rooms.len() < TERRAIN_BATCH
                && let Some(room) = self.queue.pop_front()
            {
                if !self.known.contains_key(&room)
                    && !self.absent.contains(&room)
                    && self.pending.insert(room)
                {
                    rooms.push(room);
                }
            }
            if rooms.is_empty() {
                break;
            }
            let names = rooms.iter().map(ToString::to_string).collect::<Vec<_>>();
            let (map, shard) = (map.clone(), shard.map(str::to_owned));
            let request = Box::pin(async move { map.terrain(names, shard.as_deref()).await });
            self.fetches.push(Fetch { rooms, request });
        }
        None
    }

    /// Completes the requests that finished: queues a [`WorldEvent::Terrain`] per new terrain
    /// and a [`WorldEvent::NoTerrain`] per room the server has no terrain for. Returns whether
    /// anything changed.
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        events: &mut VecDeque<WorldEvent>,
    ) -> Result<bool, Closed> {
        let mut changed = false;
        let mut index = 0;
        while index < self.fetches.len() {
            let Poll::Ready(result) = self.fetches[index].request.as_mut().poll(cx) else {
                index += 1;
                continue;
            };
            let rooms = self.fetches.remove(index).rooms;
            changed = true;
            for room in &rooms {
                self.pending.remove(room);
            }
            match result {
                Ok(terrains) => self.record(rooms, terrains, events),
                Err(Error::Closed) => return Err(Closed),
                Err(error) => self.retry(rooms, &error),
            }
        }
        Ok(changed)
    }

    /// Stores the terrains of a completed batch; the batch's other rooms are absent.
    fn record(
        &mut self,
        rooms: Vec<RoomCoord>,
        mut terrains: BTreeMap<String, Arc<Terrain>>,
        events: &mut VecDeque<WorldEvent>,
    ) {
        for room in rooms {
            let wanted = self.wanted.contains(&room);
            match terrains.remove(&room.to_string()) {
                Some(terrain) => {
                    if let Entry::Vacant(entry) = self.known.entry(room) {
                        entry.insert(Arc::clone(&terrain));
                        self.wanted_known += usize::from(wanted);
                        events.push_back(WorldEvent::Terrain(room, terrain));
                    }
                }
                None => {
                    if !self.known.contains_key(&room) && self.absent.insert(room) {
                        self.wanted_absent += usize::from(wanted);
                        events.push_back(WorldEvent::NoTerrain(room));
                    }
                }
            }
        }
    }

    /// Puts the rooms of a failed batch back at the front of the queue (those still wanted)
    /// and holds requests back: until the endpoint's reset after HTTP 429, [`GLOBAL_RETRY`]
    /// after a server-wide 429, [`RETRY`] after other failures.
    fn retry(&mut self, rooms: Vec<RoomCoord>, error: &Error) {
        for room in rooms.into_iter().rev() {
            if self.wanted.contains(&room) && !self.known.contains_key(&room) {
                self.queue.push_front(room);
            }
        }
        let now = Instant::now();
        let at = match error {
            Error::Net(NetError::Api(ApiError::RateLimited(Some(limit)))) => {
                reset_at(limit.reset, now).max(now + MIN_LIMIT_WAIT)
            }
            Error::Net(NetError::Api(ApiError::RateLimited(None))) => now + GLOBAL_RETRY,
            _ => now + RETRY,
        };
        self.not_before = Some(self.not_before.map_or(at, |before| before.max(at)));
    }
}

/// A rate-limit reset (seconds since the Unix epoch, plus a second for clock skew) as an
/// [`Instant`].
fn reset_at(reset: u64, now: Instant) -> Instant {
    let reset = UNIX_EPOCH + Duration::from_secs(reset.saturating_add(1));
    now + reset
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO)
}
