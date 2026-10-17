//! Terrain of the world map: batched, deduplicated, viewed and visible rooms first, the rest
//! of the world swept in the background, kept for the session.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use overseer_net::Error as NetError;
use overseer_net::api::WorldOperation;
use overseer_net::core::api::ApiError;
use overseer_net::model::world::Terrain;
use overseer_net::rt::{BoxFuture, Instant, SystemTime, UNIX_EPOCH};

use super::WorldEvent;
use super::focus::{Budget, Focus, Server, Tier, distance};
use crate::map::{Map, RoomCoord, RoomRect};
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

/// A request in flight: its rooms, nearest first, whether it is the background sweep's, and
/// the request.
struct Fetch {
    rooms: Vec<RoomCoord>,
    background: bool,
    request: BoxFuture<'static, Result<BTreeMap<String, Arc<Terrain>>, Error>>,
}

/// The terrain known, wanted and being fetched.
pub(super) struct Terrains {
    /// Every terrain fetched or seeded.
    pub(super) known: HashMap<RoomCoord, Arc<Terrain>>,
    /// Rooms the server has no terrain for (fetched or seeded).
    absent: HashSet<RoomCoord>,
    /// The rooms of the latest interest.
    wanted: HashSet<RoomCoord>,
    /// Foreground rooms not requested yet: the viewed and wanted rooms in their order, then
    /// the prefetch ring nearest the centre. Rooms seeded meanwhile are skipped when a batch
    /// is taken, so seeding never scans it.
    queue: VecDeque<RoomCoord>,
    /// Every room of the world: swept in the background.
    world: RoomRect,
    /// While any terrain is wanted, the room the background sweep goes out from.
    sweep: Option<RoomCoord>,
    /// The sweep found no room left (until a failed request gives some back).
    swept: bool,
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
    /// Nothing known or wanted yet of the world `world`.
    pub(super) fn new(world: RoomRect) -> Self {
        Self {
            known: HashMap::new(),
            absent: HashSet::new(),
            wanted: HashSet::new(),
            queue: VecDeque::new(),
            world,
            sweep: None,
            swept: false,
            pending: HashSet::new(),
            fetches: Vec::new(),
            not_before: None,
            wanted_known: 0,
            wanted_absent: 0,
        }
    }

    /// Makes `rooms` (the viewed rooms, then the others nearest first) the wanted rooms and
    /// queues those neither known, absent nor in flight. While a map is shown (`map`: a
    /// terrain interest of its own, not only viewed rooms), the rest of the prefetch ring of
    /// `focus` follows, and the rest of the world is swept in the background, nearest the
    /// centre of `focus` first.
    pub(super) fn want(
        &mut self,
        rooms: impl IntoIterator<Item = RoomCoord>,
        focus: &Focus,
        map: bool,
    ) {
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
        if let Some(visible) = focus.visible().filter(|_| map) {
            let mut ring = visible
                .iter()
                .filter(|room| !self.wanted.contains(room) && self.unknown(*room))
                .collect::<Vec<_>>();
            ring.sort_unstable_by_key(|&room| focus.key(room));
            self.queue.extend(ring);
        }
        self.sweep = map.then(|| focus.centre());
    }

    /// Whether `room` is neither known, absent nor in flight.
    fn unknown(&self, room: RoomCoord) -> bool {
        !self.known.contains_key(&room)
            && !self.absent.contains(&room)
            && !self.pending.contains(&room)
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

    /// How many rooms the server has no terrain for.
    pub(super) fn absent_count(&self) -> usize {
        self.absent.len()
    }

    /// The wanted rooms: with a known terrain, without one on the server, and in total.
    pub(super) fn progress(&self) -> (usize, usize, usize) {
        (self.wanted_known, self.wanted_absent, self.wanted.len())
    }

    /// Starts batches until [`TERRAIN_IN_FLIGHT`] are in flight: the foreground queue first,
    /// then (while it is empty) one batch at a time of the background sweep, so a foreground
    /// batch always finds a free slot. Returns when to plan again while a failure or the rate
    /// limit holds requests back.
    pub(super) fn plan(
        &mut self,
        map: &Map,
        shard: Option<&str>,
        (now, focus, _): (Instant, &Focus, Server),
        budget: &mut Budget<'_>,
    ) -> Option<Instant> {
        if self.fetches.len() >= TERRAIN_IN_FLIGHT || (self.queue.is_empty() && !self.sweeps()) {
            return None;
        }
        let mut ready = self.not_before.filter(|&at| at > now);
        if let Some(limit) = WorldOperation::Terrains.rate_limit(budget.client())
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
            let tier = match self.queue.iter().find(|room| self.unknown(**room)) {
                Some(&room) => focus.tier(room).min(Tier::Visible),
                None if self.sweeps() => Tier::Background,
                None => break,
            };
            if let Err(at) = budget.take(tier) {
                return Some(at);
            }
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
            let background = rooms.is_empty();
            if background {
                if !self.sweeps() {
                    break;
                }
                rooms = self.background_batch();
                if rooms.is_empty() {
                    self.swept = true;
                    break;
                }
                self.pending.extend(rooms.iter().copied());
            }
            let names = rooms.iter().map(ToString::to_string).collect::<Vec<_>>();
            let (map, shard) = (map.clone(), shard.map(str::to_owned));
            let request = Box::pin(async move { map.terrain(names, shard.as_deref()).await });
            self.fetches.push(Fetch {
                rooms,
                background,
                request,
            });
        }
        None
    }

    /// Whether the background sweep may start a batch: it is on, has rooms left and none of
    /// its batches is in flight.
    fn sweeps(&self) -> bool {
        self.sweep.is_some() && !self.swept && !self.fetches.iter().any(|fetch| fetch.background)
    }

    /// The next batch of the background sweep: the [`TERRAIN_BATCH`] rooms of the world
    /// neither known, absent nor in flight nearest the sweep's centre, nearest first.
    fn background_batch(&self) -> Vec<RoomCoord> {
        let Some(centre) = self.sweep else {
            return Vec::new();
        };
        let key = |room: &RoomCoord| (distance(*room, centre), room.y, room.x);
        let mut rooms = self
            .world
            .iter()
            .filter(|&room| self.unknown(room))
            .collect::<Vec<_>>();
        if rooms.len() > TERRAIN_BATCH {
            rooms.select_nth_unstable_by_key(TERRAIN_BATCH, key);
            rooms.truncate(TERRAIN_BATCH);
        }
        rooms.sort_unstable_by_key(key);
        rooms
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
            let Fetch {
                rooms, background, ..
            } = self.fetches.remove(index);
            changed = true;
            for room in &rooms {
                self.pending.remove(room);
            }
            match result {
                Ok(terrains) => self.record(rooms, terrains, events),
                Err(Error::Closed) => return Err(Closed),
                Err(error) => self.retry(rooms, background, &error),
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

    /// Puts the rooms of a failed batch back at the front of the queue (those still wanted;
    /// the sweep finds the others again) and holds requests back: until the endpoint's reset
    /// after HTTP 429, [`GLOBAL_RETRY`] after a server-wide 429, [`RETRY`] after other
    /// failures.
    fn retry(&mut self, rooms: Vec<RoomCoord>, background: bool, error: &Error) {
        for room in rooms.into_iter().rev() {
            if !background && self.wanted.contains(&room) && !self.known.contains_key(&room) {
                self.queue.push_front(room);
            }
        }
        self.swept = false;
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
pub(super) fn reset_at(reset: u64, now: Instant) -> Instant {
    let reset = UNIX_EPOCH + Duration::from_secs(reset.saturating_add(1));
    now + reset
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO)
}
