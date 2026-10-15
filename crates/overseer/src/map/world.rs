//! The continuous world map: what a map screen needs of every room, fetched and subscribed as
//! the view moves.

mod detail;
mod stats;
mod terrain;

pub use stats::{STATS_BATCH_SECTORS, STATS_FAR_SHARE, STATS_FAR_TTL, STATS_TTL};
pub use terrain::{TERRAIN_BATCH, TERRAIN_IN_FLIGHT};

use std::collections::{HashSet, VecDeque};
use std::fmt;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use overseer_net::core::api::Terrain;
use overseer_net::core::socket::types::RoomMap;
use overseer_net::rt::{self, Instant, Sleep};
use tokio::sync::watch;

use super::rect::Sides;
use super::{Map, MapView, RoomCoord, RoomRect, RoomStat};
use crate::refresh::closed;
use crate::{Closed, ConnectionStatus, Error, Overseer, RoomSnapshot};

use detail::Details;
use stats::Stats;
use terrain::Terrains;

/// Most rooms with a live minimap at once; [`Interest::minimaps`] beyond it are ignored.
///
/// Each live minimap is one socket update per tick to receive and decode, so the count is
/// bounded whatever the size of the view; the renderer asks for the nearest rooms first.
pub const MINIMAP_CAP: usize = 256;

/// The central rooms whose existence tells which sides of the zero lines hold rooms.
const CENTRE: [&str; 4] = ["W0N0", "E0N0", "W0S0", "E0S0"];

/// What the UI currently needs; the renderer computes it from its camera.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interest {
    /// Rooms whose overview (status, owner, level, reservation, safe mode) is wanted.
    pub stats: Option<RoomRect>,
    /// Rooms whose terrain is wanted (nearest the centre first).
    pub terrain: Vec<RoomCoord>,
    /// Rooms whose live minimap (`roomMap2`) is wanted (nearest first; the library keeps the
    /// first [`MINIMAP_CAP`]).
    pub minimaps: Vec<RoomCoord>,
    /// Rooms wanted live in full detail, most important first.
    pub detail: Vec<RoomCoord>,
}

/// The live state of a room wanted in detail.
#[derive(Clone, Debug)]
pub enum DetailState {
    /// Subscribing; no update yet.
    Pending,
    /// The latest update.
    Live(Arc<RoomSnapshot>),
    /// The server refused the subscription (for example `subscribe limit reached`) or it
    /// failed; not retried until [`Interest::detail`] changes.
    Refused(String),
}

/// A change of the world map, from [`WorldMap::next`].
#[derive(Clone, Debug)]
pub enum WorldEvent {
    /// A newly fetched terrain (not one seeded with [`WorldMap::insert_terrain`]): the app
    /// persists it.
    Terrain(RoomCoord, Arc<Terrain>),
    /// The server has no terrain for a room (it is outside its world), reported once (not for
    /// rooms seeded with [`WorldMap::insert_no_terrain`]): the app persists it.
    NoTerrain(RoomCoord),
    /// The overviews one request brought, each with a new [`RoomStat::fetched_at`] (changed
    /// or not): the app persists them. Nearest first.
    Stats(Vec<(RoomCoord, Arc<RoomStat>)>),
    /// The latest minimap of a room.
    Minimap(RoomCoord, Arc<RoomMap>),
    /// A room's minimap is no longer live (it left [`Interest::minimaps`]).
    MinimapGone(RoomCoord),
    /// A room wanted in detail: pending, each live update, refused.
    Detail(RoomCoord, DetailState),
    /// A room left [`Interest::detail`] and was released.
    DetailGone(RoomCoord),
}

/// How much of what is wanted is known, from [`WorldMap::progress`]: for a loading indicator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WorldProgress {
    /// Rooms of [`Interest::terrain`] (and [`Interest::detail`]) whose terrain is known.
    pub terrain_known: usize,
    /// Rooms of [`Interest::terrain`] (and [`Interest::detail`]) the server has no terrain for.
    pub terrain_absent: usize,
    /// Rooms of [`Interest::terrain`] (and [`Interest::detail`]), without repetitions.
    pub terrain_wanted: usize,
    /// Rooms of the world with an overview (possibly stale; it is refreshed meanwhile).
    pub stats_known: usize,
    /// Rooms whose overview is wanted: every room of the world while [`Interest::stats`] is
    /// set and a user is signed in, else none.
    pub stats_wanted: usize,
}

impl WorldProgress {
    /// Whether every wanted terrain and overview is known (or known not to exist).
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.terrain_known + self.terrain_absent >= self.terrain_wanted
            && self.stats_known >= self.stats_wanted
    }
}

/// The world of one shard as one continuous map: the overview, terrain, live minimap and live
/// detail of the rooms the UI needs, as the view moves.
///
/// Created by [`Map::world`] or [`Map::world_with_bounds`]. The app tells it what it needs with
/// [`Self::set_interest`] (usually from the renderer's camera), which never blocks: it diffs
/// subscriptions and queues requests, and the work happens while the app awaits
/// [`Self::next`], which returns every change. The latest data stays readable with
/// [`Self::terrain`], [`Self::stat`], [`Self::minimap`] and [`Self::detail`].
///
/// # Policies
///
/// - **Terrain** (`POST /api/game/rooms`): [`Interest::terrain`] may list every room of the
///   world. The wanted rooms neither known, absent nor in flight are requested nearest first
///   in batches of [`TERRAIN_BATCH`] rooms (measured on screeps.com: a whole 182 × 182 shard
///   takes 17 requests), at most [`TERRAIN_IN_FLIGHT`] requests at a time. A room is
///   requested at most once per session, whatever the interest changes: terrain never
///   changes, so it is kept, and [`Self::insert_terrain`] seeds it from the app's cache (those
///   rooms are never requested). Rooms the server has no terrain for are reported once
///   ([`WorldEvent::NoTerrain`]) and never asked again; [`Self::insert_no_terrain`] seeds
///   them. The endpoint has only the server-wide rate limit on screeps.com (120 requests per
///   minute): once a limit reports no request left, requests wait for its reset; after HTTP
///   429 they wait for the endpoint's reset (60 s when the server-wide limit refused it),
///   after other failures 10 s.
/// - **Overviews** (`POST /api/game/map-stats`, `owner0`): while [`Interest::stats`] is set,
///   every room of the world is wanted and arrives progressively within the budget. Near
///   rooms (the sectors of [`Interest::minimaps`] and the sectors around them) are used for
///   [`STATS_TTL`] (10 minutes), all others for [`STATS_FAR_TTL`] (60 minutes), before they
///   are asked again; [`Self::insert_stats`] seeds overviews from the app's cache, stale
///   after the same time since their [`RoomStat::fetched_at`]. Stale near rooms go first, by
///   whole sectors (10 × 10 rooms, like the official world map) nearest the first minimap
///   room; then stale far rooms, by whole sectors nearest the centre of [`Interest::stats`].
///   At most [`STATS_BATCH_SECTORS`] sectors (2 500 rooms) per request, one request at a
///   time: a whole screeps.com shard takes 14 requests per hour. The requests follow the
///   endpoint's rate limit ([`net::Client::rate_limit`](crate::net::Client::rate_limit),
///   screeps.com allows 60 per hour per token): at least 1 s apart; once fewer than half of
///   the window's requests are left, the rest are spread evenly until the reset; far rooms
///   may use only [`STATS_FAR_SHARE`] percent of the window (42 of 60), so near rooms always
///   refresh; none goes out while none is left, and after HTTP 429 the next waits for the
///   reset (60 s when the server-wide limit refused it). Other failures are retried after
///   30 s.
/// - **Minimaps** (`roomMap2`): the first [`MINIMAP_CAP`] rooms of [`Interest::minimaps`]
///   are subscribed through a [`MapView`]; rooms that leave are released.
/// - **Detail** (`room`): the rooms of [`Interest::detail`] are subscribed in priority order.
///   Their terrain comes from the terrain above (they are wanted first, ahead of
///   [`Interest::terrain`]), so a seeded or fetched room subscribes without any request; a
///   room the server does not know is refused.
///   The server sends each user at most 2 rooms per update across all connections (pop-out
///   windows included) and refuses the others (`subscribe limit reached`, chosen at random,
///   on every update): the least important subscribed room is then released and reported
///   [`DetailState::Refused`] once, and not subscribed again until [`Interest::detail`]
///   changes. Rooms that leave are released ([`WorldEvent::DetailGone`]).
///   A refusal releases a room only when it reflects the current subscriptions: the server
///   keeps refusing over-limit rooms until it processes a release (several updates when its
///   command queue is busy), and a refusal sent before a release can arrive after the same
///   room is subscribed again. Every subscription or release of detail rooms is therefore
///   followed by a barrier: a `roomMap2` subscription of a room that does not exist
///   (`overseer-barrier-<n>`), which the server answers at once and in order (screeps.com and
///   `OpenScreeps`), then released. Refusals that arrive before its answer, or are found
///   waiting when it arrives, release nothing and change no state (the server repeats those
///   that still hold on its next update); each later round of refusals releases one room and
///   starts a new barrier. Should a server never answer, rooms are never released for its
///   refusals and the server keeps choosing which ones to skip.
///
/// Overviews, minimaps and detail need a signed-in user: without one, [`Interest::stats`] and
/// [`Interest::minimaps`] are ignored and detail rooms are refused.
///
/// Works natively and in browsers: requests and timers run on [`net::rt`](crate::net::rt).
pub struct WorldMap {
    overseer: Overseer,
    map: Map,
    shard: Option<String>,
    bounds: RoomRect,
    interest: Interest,
    status: watch::Receiver<ConnectionStatus>,
    /// `None` without a signed-in user.
    minimaps: Option<MapView>,
    /// Rooms whose minimap was reported and not yet gone.
    shown: HashSet<RoomCoord>,
    work: Work,
}

/// The requests and subscriptions [`WorldMap::next`] drives.
struct Work {
    events: VecDeque<WorldEvent>,
    terrain: Terrains,
    stats: Stats,
    details: Details,
    /// Whether to plan again: something changed since the last plan.
    dirty: bool,
    /// Wakes the planning when a retry or an expiry is due.
    timer: Option<(Instant, Pin<Box<Sleep>>)>,
}

impl Work {
    /// Nothing known or wanted yet of the world `bounds`.
    fn new(bounds: RoomRect) -> Self {
        Self {
            events: VecDeque::new(),
            terrain: Terrains::default(),
            stats: Stats::new(bounds),
            details: Details::default(),
            dirty: false,
            timer: None,
        }
    }

    /// Polls every request, subscription and the timer; ready once something changed.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        client: &overseer_net::Client,
    ) -> Poll<Result<(), Closed>> {
        let mut changed = self.terrain.poll(cx, &mut self.events)?;
        changed |= self.stats.poll(cx, client, &mut self.events)?;
        changed |= self.details.poll(cx, &self.terrain, &mut self.events)?;
        if let Some((_, sleep)) = &mut self.timer
            && sleep.as_mut().poll(cx).is_ready()
        {
            self.timer = None;
            changed = true;
        }
        if changed {
            self.dirty = true;
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}

impl WorldMap {
    fn new(overseer: Overseer, shard: Option<String>, bounds: RoomRect) -> Self {
        let map = overseer.map();
        let minimaps = map.view(shard.as_deref()).ok();
        Self {
            status: overseer.status(),
            overseer,
            map,
            shard,
            bounds,
            interest: Interest::default(),
            minimaps,
            shown: HashSet::new(),
            work: Work::new(bounds),
        }
    }

    /// The shard; `None` on single-world servers.
    #[must_use]
    pub fn shard(&self) -> Option<&str> {
        self.shard.as_deref()
    }

    /// Every room of the world.
    #[must_use]
    pub const fn bounds(&self) -> RoomRect {
        self.bounds
    }

    /// Makes `interest` what the UI needs. Never blocks: subscribes and releases minimaps and
    /// detail rooms, and queues terrain and overview requests; [`Self::next`] does the work and
    /// reports the changes (including the [`WorldEvent::MinimapGone`],
    /// [`WorldEvent::DetailGone`] and [`DetailState::Pending`] this queues).
    pub fn set_interest(&mut self, interest: &Interest) {
        if *interest == self.interest {
            return;
        }
        let user = self.overseer.user_id().is_some();
        let work = &mut self.work;
        work.dirty = true;
        work.stats.rect = interest
            .stats
            .filter(|_| user)
            .and_then(|rect| rect.intersection(&self.bounds));
        let detail = unique(&interest.detail);
        if user {
            work.terrain
                .want(detail.iter().chain(&interest.terrain).copied());
        } else {
            work.terrain.want(interest.terrain.iter().copied());
        }
        let minimaps = unique(&interest.minimaps);
        work.stats
            .set_near(&minimaps[..minimaps.len().min(MINIMAP_CAP)]);
        if let Some(view) = &mut self.minimaps {
            let rooms = minimaps;
            let change = view.set_rooms(rooms.iter().take(MINIMAP_CAP).map(ToString::to_string));
            for room in change.removed {
                if let Ok(room) = room.parse()
                    && self.shown.remove(&room)
                {
                    work.events.push_back(WorldEvent::MinimapGone(room));
                }
            }
        }
        let rooms = user.then(|| self.overseer.rooms());
        work.details.set(
            detail,
            rooms.as_ref(),
            self.shard.as_deref(),
            &work.terrain,
            &mut work.events,
        );
        self.interest = interest.clone();
    }

    /// Seeds the terrain of `room` from the app's persistent cache: it is never requested and
    /// no [`WorldEvent::Terrain`] reports it.
    pub fn insert_terrain(&mut self, room: RoomCoord, terrain: Arc<Terrain>) {
        self.work.terrain.insert(room, terrain);
    }

    /// Seeds from the app's persistent cache that the server has no terrain for `room` (an
    /// earlier [`WorldEvent::NoTerrain`]): it is never requested and not reported again.
    /// Ignored when its terrain is known.
    pub fn insert_no_terrain(&mut self, room: RoomCoord) {
        self.work.terrain.insert_absent(room);
    }

    /// Seeds the overview of `room` from the app's persistent cache: no [`WorldEvent::Stats`]
    /// reports it, and it is requested again once stale (its [`RoomStat::fetched_at`] older
    /// than [`STATS_TTL`] for a near room, [`STATS_FAR_TTL`] for the others). Ignored when a
    /// newer overview is known.
    pub fn insert_stats(&mut self, room: RoomCoord, stat: Arc<RoomStat>) {
        self.work.stats.insert(room, stat);
        self.work.dirty = true;
    }

    /// How much of what is wanted is known.
    #[must_use]
    pub fn progress(&self) -> WorldProgress {
        let (terrain_known, terrain_absent, terrain_wanted) = self.work.terrain.progress();
        let (stats_known, stats_wanted) = self.work.stats.progress();
        WorldProgress {
            terrain_known,
            terrain_absent,
            terrain_wanted,
            stats_known,
            stats_wanted,
        }
    }

    /// The next change. Cancel-safe: requests and subscriptions live in the `WorldMap`, so
    /// dropping the future loses nothing. Waits forever while nothing is wanted.
    ///
    /// # Errors
    ///
    /// Returns [`Closed`] once the session is closed.
    pub async fn next(&mut self) -> Result<WorldEvent, Closed> {
        loop {
            if let Some(event) = self.work.events.pop_front() {
                return Ok(event);
            }
            if *self.status.borrow() == ConnectionStatus::Closed {
                return Err(Closed);
            }
            self.plan();
            let Self {
                overseer,
                status,
                minimaps,
                shown,
                work,
                ..
            } = self;
            let minimap = async {
                match minimaps {
                    Some(view) => view.changed().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                () = closed(status) => return Err(Closed),
                result = poll_fn(|cx| work.poll(cx, overseer.client())) => result?,
                changed = minimap => {
                    let (room, map) = changed?;
                    if let Ok(room) = room.parse() {
                        shown.insert(room);
                        work.events.push_back(WorldEvent::Minimap(room, map));
                    }
                }
            }
        }
    }

    /// Starts the requests that are due and sets the timer for the next one.
    fn plan(&mut self) {
        let work = &mut self.work;
        if !work.dirty {
            return;
        }
        work.dirty = false;
        let now = Instant::now();
        let shard = self.shard.as_deref();
        let client = self.overseer.client();
        let terrain = work.terrain.plan(&self.map, client, shard, now);
        let stats = work.stats.plan(&self.map, client, shard, now);
        let due = match (terrain, stats) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (due, None) | (None, due) => due,
        };
        match due {
            Some(due) if work.timer.as_ref().is_none_or(|(at, _)| *at != due) => {
                work.timer = Some((due, Box::pin(rt::sleep_until(due))));
            }
            Some(_) => {}
            None => work.timer = None,
        }
    }

    /// The terrain of `room`, when fetched or seeded.
    #[must_use]
    pub fn terrain(&self, room: RoomCoord) -> Option<Arc<Terrain>> {
        self.work.terrain.known.get(&room).cloned()
    }

    /// Whether the server has no terrain for `room` (reported by [`WorldEvent::NoTerrain`] or
    /// seeded with [`Self::insert_no_terrain`]).
    #[must_use]
    pub fn has_no_terrain(&self, room: RoomCoord) -> bool {
        self.work.terrain.is_absent(room)
    }

    /// The overview of `room`, when fetched or seeded.
    #[must_use]
    pub fn stat(&self, room: RoomCoord) -> Option<Arc<RoomStat>> {
        self.work.stats.get(room)
    }

    /// The latest minimap of `room`, from its first [`WorldEvent::Minimap`] until its
    /// [`WorldEvent::MinimapGone`].
    #[must_use]
    pub fn minimap(&self, room: RoomCoord) -> Option<Arc<RoomMap>> {
        if !self.shown.contains(&room) {
            return None;
        }
        self.minimaps.as_ref()?.latest(&room.to_string())
    }

    /// The state of `room`, while it is wanted in detail.
    #[must_use]
    pub fn detail(&self, room: RoomCoord) -> Option<&DetailState> {
        self.work.details.state(room)
    }
}

impl fmt::Debug for WorldMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorldMap")
            .field("shard", &self.shard)
            .field("bounds", &self.bounds)
            .field("interest", &self.interest)
            .finish_non_exhaustive()
    }
}

impl Map {
    /// The world of `shard` (`None`: the default shard; ignored by single-world servers) as
    /// one continuous map; see [`WorldMap`].
    ///
    /// Its bounds come from the world size (`GET /api/game/world-size`) and which of the four
    /// central rooms exist (one `POST /api/game/rooms` for `W0N0`, `E0N0`, `W0S0` and `E0S0`,
    /// whose terrain stays cached): screeps.com shards are centred on them, the upstream
    /// default world and `OpenScreeps` worlds lie north-west of them (see [`RoomRect`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when a request fails.
    pub async fn world(&self, shard: Option<&str>) -> Result<WorldMap, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        let size = self.world_size(shard.as_deref()).await?;
        let centre = self.terrain(CENTRE, shard.as_deref()).await?;
        let exists = |room: &str| centre.contains_key(room);
        let sides = Sides {
            west: exists("W0N0") || exists("W0S0"),
            east: exists("E0N0") || exists("E0S0"),
            north: exists("W0N0") || exists("E0N0"),
            south: exists("W0S0") || exists("E0S0"),
        };
        let rooms = |count: i64| u32::try_from(count).unwrap_or(0);
        let bounds = RoomRect::world(rooms(size.width), rooms(size.height), sides);
        Ok(WorldMap::new(self.overseer.clone(), shard, bounds))
    }

    /// The world of `shard` as one continuous map whose bounds the caller already knows (for
    /// example [`WorldMap::bounds`] persisted by the app from an earlier session): unlike
    /// [`Self::world`] it makes no request. Bounds larger than the world only cost requests
    /// for rooms the server does not know; smaller ones hide rooms.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown).
    pub fn world_with_bounds(
        &self,
        shard: Option<&str>,
        bounds: RoomRect,
    ) -> Result<WorldMap, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        Ok(WorldMap::new(self.overseer.clone(), shard, bounds))
    }
}

/// `rooms` without repetitions, in order.
fn unique(rooms: &[RoomCoord]) -> Vec<RoomCoord> {
    let mut seen = HashSet::with_capacity(rooms.len());
    rooms
        .iter()
        .copied()
        .filter(|room| seen.insert(*room))
        .collect()
}
