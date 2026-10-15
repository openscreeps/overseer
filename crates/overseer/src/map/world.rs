//! The continuous world map: what a map screen needs of every room, fetched and subscribed as
//! the view moves.

mod detail;
mod stats;
mod terrain;

pub use stats::{STATS_BATCH_SECTORS, STATS_TTL};

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
    /// New or changed overviews, nearest the centre of [`Interest::stats`] first.
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

/// The world of one shard as one continuous map: the overview, terrain, live minimap and live
/// detail of the rooms the UI needs, as the view moves.
///
/// Created by [`Map::world`]. The app tells it what it needs with [`Self::set_interest`]
/// (usually from the renderer's camera), which never blocks: it diffs subscriptions and queues
/// requests, and the work happens while the app awaits [`Self::next`], which returns every
/// change. The latest data stays readable with [`Self::terrain`], [`Self::stat`],
/// [`Self::minimap`] and [`Self::detail`].
///
/// # Policies
///
/// - **Terrain** (`POST /api/game/rooms`): the wanted rooms neither known nor in flight are
///   requested nearest first, one request at a time, in batches of at least 600 rooms and at
///   most 3 requests per interest (the official world map's batching). Terrain never changes:
///   it is kept for the session and [`Self::insert_terrain`] seeds it from the app's cache
///   (those rooms are never requested). Rooms the server does not know are not asked again.
///   A failed request is retried after 10 s.
/// - **Overviews** (`POST /api/game/map-stats`, `owner0`): like the official world map, the
///   stale rooms of [`Interest::stats`] inside the world are asked for by whole sectors
///   (10 × 10 rooms), the sectors nearest its centre first, at most [`STATS_BATCH_SECTORS`]
///   (2 500 rooms) per request, one request at a time: a whole screeps.com shard fills in
///   about 6 requests. An overview is used for [`STATS_TTL`] (10 minutes) before it is asked
///   again. The requests
///   follow the endpoint's rate limit ([`net::Client::rate_limit`](crate::net::Client::rate_limit),
///   screeps.com allows 60 per hour per token): at least 1 s apart; once fewer than half of
///   the window's requests are left, the rest are spread evenly until the reset; none goes
///   out while none is left, and after HTTP 429 the next waits for the reset (60 s when the
///   server-wide limit refused it). Other failures are retried after 30 s.
/// - **Minimaps** (`roomMap2`): the first [`MINIMAP_CAP`] rooms of [`Interest::minimaps`]
///   are subscribed through a [`MapView`]; rooms that leave are released.
/// - **Detail** (`room`): the rooms of [`Interest::detail`] are subscribed in priority order.
///   The server sends each user at most 2 rooms per update across all connections (pop-out
///   windows included) and refuses the others (`subscribe limit reached`): the least
///   important subscribed room is then released and reported [`DetailState::Refused`] once,
///   and not subscribed again until [`Interest::detail`] changes. Rooms that leave are
///   released ([`WorldEvent::DetailGone`]).
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
#[derive(Default)]
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
    /// Polls every request, subscription and the timer; ready once something changed.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        client: &overseer_net::Client,
    ) -> Poll<Result<(), Closed>> {
        let mut changed = self.terrain.poll(cx, &mut self.events)?;
        changed |= self.stats.poll(cx, client, &mut self.events)?;
        changed |= self.details.poll(cx, &mut self.events)?;
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
            work: Work::default(),
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
        work.terrain.want(&interest.terrain);
        if let Some(view) = &mut self.minimaps {
            let rooms = unique(&interest.minimaps);
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
            unique(&interest.detail),
            rooms.as_ref(),
            self.shard.as_deref(),
            &mut work.events,
        );
        self.interest = interest.clone();
    }

    /// Seeds the terrain of `room` from the app's persistent cache: it is never requested and
    /// no [`WorldEvent::Terrain`] reports it.
    pub fn insert_terrain(&mut self, room: RoomCoord, terrain: Arc<Terrain>) {
        self.work.terrain.insert(room, terrain);
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
        let terrain = work.terrain.plan(&self.map, shard, now);
        let stats = work
            .stats
            .plan(&self.map, self.overseer.client(), shard, now);
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

    /// The overview of `room`, when fetched.
    #[must_use]
    pub fn stat(&self, room: RoomCoord) -> Option<Arc<RoomStat>> {
        self.work.stats.get(room)
    }

    /// The latest minimap of `room`, while it is live.
    #[must_use]
    pub fn minimap(&self, room: RoomCoord) -> Option<Arc<RoomMap>> {
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
