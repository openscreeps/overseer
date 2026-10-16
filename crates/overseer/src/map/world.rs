//! The continuous world map: what a map screen needs of every room, fetched and subscribed as
//! the view moves, viewed rooms first, visible rooms next, the rest of the world in the
//! background.

mod detail;
mod focus;
mod portals;
mod stats;
mod terrain;

pub use detail::DETAIL_FIRST;
pub use focus::PREFETCH_RING;
pub use portals::{
    PORTAL_BACKGROUND_IN_FLIGHT, PORTAL_DECAYING_TTL, PORTAL_FETCH_TIMEOUT, PORTAL_HELD_SCANS,
    PORTAL_IN_FLIGHT, PORTAL_QUIET_TTL, PORTAL_SCAN_SETTLE, PORTAL_SCAN_TIMEOUT, PORTAL_TTL,
};
pub use stats::{
    STATS_BATCH_SECTORS, STATS_FAR_SHARE, STATS_FAR_TTL, STATS_IN_FLIGHT, STATS_TTL,
    STATS_VIEWED_FRESH,
};
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
use super::{Map, MapView, Portal, RoomCoord, RoomRect, RoomStat, WorldKnowledge};
use crate::refresh::closed;
use crate::{Closed, ConnectionStatus, Error, Overseer, RoomSnapshot};

use detail::Details;
use focus::{Budget, Focus, Server};
use portals::Portals;
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
///
/// It also tells [`WorldMap`] what comes first: the rooms of [`Self::detail`] are the viewed
/// rooms (the focused room, then the rooms of drawers and pop-out windows), those of
/// [`Self::stats`] (else [`Self::portals`], else [`Self::terrain`]) the visible ones, and the
/// first room of [`Self::terrain`] the centre of the view.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Interest {
    /// Rooms whose overview (status, owner, level, reservation, safe mode) is wanted.
    pub stats: Option<RoomRect>,
    /// Rooms whose terrain is wanted (nearest the centre first).
    pub terrain: Vec<RoomCoord>,
    /// Rooms whose live minimap (`roomMap2`) is wanted (nearest first; the library keeps the
    /// first [`MINIMAP_CAP`]).
    pub minimaps: Vec<RoomCoord>,
    /// Rooms wanted live in full detail, most important first: the viewed rooms.
    pub detail: Vec<RoomCoord>,
    /// Rooms whose portals are wanted: all of them are scanned, those where portals [usually
    /// stand](super::may_have_portals) first (see [`WorldMap`]).
    pub portals: Option<RoomRect>,
    /// Rooms whose portal destinations are wanted now, most important first: the hovered and
    /// the selected room, then the rooms whose portals the map draws arcs for. They are
    /// fetched first; all others fill in in the background.
    pub portal_destinations: Vec<RoomCoord>,
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
    /// Overviews with a new [`RoomStat::fetched_at`]: all those one request brought (changed
    /// or not), nearest first, or the one a live room changed. The app persists them.
    Stats(Vec<(RoomCoord, Arc<RoomStat>)>),
    /// The rate limit started holding back wanted overviews, until its reset (milliseconds
    /// since the Unix epoch), or stopped (`None`): the change of
    /// [`WorldProgress::stats_paused_until`], which carries no data of its own. See
    /// [`WorldMap::recheck_rate_limit`].
    StatsPaused(Option<u64>),
    /// The portals of a room, newly scanned or shown by the live room (not seeded with
    /// [`WorldMap::insert_portals`]): empty when it has none. Reported again when the room is
    /// scanned again or its live room shows other portals (see [`WorldMap`]); the app
    /// persists it with the time it arrived.
    Portals(RoomCoord, Arc<[Portal]>),
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
    /// While overview requests wait for the reset of the endpoint's rate limit (none left,
    /// or only the near share left while far rooms are wanted): when it resets, in
    /// milliseconds since the Unix epoch. `None` when not paused.
    pub stats_paused_until: Option<u64>,
    /// Rooms of [`Interest::portals`] whose portals (or their absence) are known, possibly
    /// stale; their destinations may be pending.
    pub portals_scanned: usize,
    /// Rooms of [`Interest::portals`] inside the world (but those the server has no terrain
    /// for), while a user is signed in, else none: every room is scanned.
    pub portals_wanted: usize,
    /// Rooms with portals whose destinations are pending (fetched or inferred in the
    /// background); not part of [`Self::is_complete`].
    pub portals_pending: usize,
}

impl WorldProgress {
    /// Whether every wanted terrain, overview and portal room is known (or known not to
    /// exist); portal destinations may still be pending.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.terrain_known + self.terrain_absent >= self.terrain_wanted
            && self.stats_known >= self.stats_wanted
            && self.portals_scanned >= self.portals_wanted
    }
}

/// The world of one shard as one continuous map: the overview, terrain, portals, live minimap
/// and live detail of the rooms the UI needs, as the view moves, and of the rest of the world
/// in the background.
///
/// Created by [`Map::world`] or [`Map::world_with_bounds`]. The app tells it what it needs with
/// [`Self::set_interest`] (usually from the renderer's camera), which never blocks: it diffs
/// subscriptions and queues requests, and the work happens while the app awaits
/// [`Self::next`], which returns every change. The latest data stays readable with
/// [`Self::terrain`], [`Self::stat`], [`Self::portals`], [`Self::minimap`] and
/// [`Self::detail`]; overviews and portals also through [`Self::knowledge`], a handle
/// readable outside the task that owns the world map: the one source of what is known of each
/// room, live rooms included.
///
/// # Tiers
///
/// Every kind of work (terrain, overviews, portal scans, portal destinations) goes by tier,
/// recomputed at each [`Self::set_interest`] (a pan reorders what waits; requests and scans
/// in flight carry on):
///
/// | Tier | Rooms | Order |
/// |------|-------|-------|
/// | Viewed | [`Interest::detail`]: the focused room, drawers and pop-out windows | as listed |
/// | Visible | [`Interest::stats`] (else [`Interest::portals`], else [`Interest::terrain`]) and [`PREFETCH_RING`] rooms around | nearest the first room of [`Interest::terrain`] |
/// | Background | the rest of the world, while anything of the kind is wanted | nearest the same centre |
///
/// Background work only takes what higher tiers leave: one of the [`TERRAIN_IN_FLIGHT`]
/// terrain requests, the scan subscriptions left after the viewed and visible rooms queued
/// first, and the smallest share of the request budget (below). Portal destinations go by
/// what is wanted now ([`Interest::portal_destinations`], see **Portals**). Viewed rooms
/// refresh at once regardless of their lifetime, then stay fresh while viewed: their live
/// room (see **Live rooms**) and one overview request (see **Overviews**). Their live rooms
/// are subscribed first, and while one waits for its first update the scans wait (see
/// **Detail**).
///
/// # Pacing per server
///
/// A server shows rate limits when it lists shards (screeps.com and its seasonal server:
/// their server-wide limit of 120 requests per minute per token sends no headers) or once any
/// response carried `X-RateLimit-*` headers or was refused with HTTP 429
/// ([`net::Client::has_rate_limits`](crate::net::Client::has_rate_limits)). There every
/// request of every kind first asks the session's request budget
/// ([`net::Client::budget_ready_at`](crate::net::Client::budget_ready_at)), shared with the
/// app's badge loader: every request of the client counts over a sliding minute, and
/// requests for viewed rooms may go out while fewer than
/// [`REQUEST_BUDGET`](crate::net::REQUEST_BUDGET) (110) are in it, visible ones 105,
/// background ones 100, so the user's own requests (which never wait) keep 10 of
/// screeps.com's 120; a sliding window bounds every fixed minute too. No other gap applies.
/// After HTTP 429 without the endpoint's limit every background request waits 60 s, and
/// [`net::Client::rate_limited_count`](crate::net::Client::rate_limited_count) changes, which
/// the app answers by offering to lift the limit. While the token is exempt from rate limits
/// ([`Self::set_rate_limit_exempt_until`], screeps.com's `noRatelimitUntil`) nothing is paced:
/// in-flight caps only, as on servers without rate limits. Every kind honours an endpoint's
/// limit that reports no request left (until its reset) and HTTP 429 (until the endpoint's
/// reset, 60 s after a server-wide refusal) on every server, exempt or not.
///
/// | Kind | Server with rate limits | Without (private servers), or exempt |
/// |------|-------------------------|--------------------------------------|
/// | Terrain (`POST /api/game/rooms`) | [`TERRAIN_IN_FLIGHT`] at a time, within the request budget | [`TERRAIN_IN_FLIGHT`] at a time |
/// | Overviews (`POST /api/game/map-stats`) | [`STATS_IN_FLIGHT`] at a time (one until the endpoint's limit is known and while it spreads requests), within the request budget and the endpoint's hourly budget (see below) | [`STATS_IN_FLIGHT`] at a time, no hourly budget |
/// | Portal scans (`roomMap2`) | no HTTP; within [`MINIMAP_CAP`] subscriptions | the same |
/// | Portal destinations (`GET /api/game/room-objects`) | [`PORTAL_IN_FLIGHT`] at a time ([`PORTAL_BACKGROUND_IN_FLIGHT`] in the background), within the request budget | the same, without the budget |
///
/// # Policies
///
/// - **Terrain** (`POST /api/game/rooms`): [`Interest::terrain`] may list every room of the
///   world. The viewed rooms, then the wanted rooms, then the rest of the prefetch ring,
///   neither known, absent nor in flight, are requested in that order in batches of
///   [`TERRAIN_BATCH`] rooms (measured on screeps.com: a whole 182 × 182 shard takes 17
///   requests), at most [`TERRAIN_IN_FLIGHT`] requests at a time; while any terrain is wanted
///   and they are all known, the rest of the world follows in the background, one batch at a
///   time, nearest the centre first. A room is requested at most once per session, whatever
///   the interest changes: terrain never changes, so it is kept, and [`Self::insert_terrain`]
///   seeds it from the app's cache (those rooms are never requested). Rooms the server has no
///   terrain for are reported once ([`WorldEvent::NoTerrain`]) and never asked again;
///   [`Self::insert_no_terrain`] seeds them. After other failures than HTTP 429 requests wait
///   10 s.
/// - **Overviews** (`POST /api/game/map-stats`, `owner0`): while [`Interest::stats`] is set,
///   every room of the world is wanted and arrives progressively within the budget. Near
///   rooms (the sectors of [`Interest::minimaps`] and the sectors around them) are used for
///   [`STATS_TTL`] (10 minutes), all others for [`STATS_FAR_TTL`] (60 minutes), before they
///   are asked again; [`Self::insert_stats`] seeds overviews from the app's cache, stale
///   after the same time since their [`RoomStat::fetched_at`]. A room that becomes viewed is
///   asked again at once (a live room cannot tell its status), unless it was asked less than
///   [`STATS_VIEWED_FRESH`] (1 minute) before. Viewed rooms and stale near rooms go first,
///   the near rooms by whole sectors (10 × 10 rooms, like the official world map) nearest the
///   first minimap room, with the stale far rooms in view ([`Interest::stats`]) after them
///   while the far share allows, so a view wider than the near rooms fills in the same
///   request; then stale far rooms, by whole sectors, those touching the visible rooms first,
///   nearest the centre. At most [`STATS_BATCH_SECTORS`] sectors (5 000 rooms)
///   per request: a whole screeps.com shard takes 7 requests per hour. Up to
///   [`STATS_IN_FLIGHT`] (3) requests go out at once, each taking the most important rooms
///   not already asked for, so the viewed, near and visible rooms are in the first round. On
///   servers with rate limits the endpoint's rate limit
///   ([`net::Client::rate_limit`](crate::net::Client::rate_limit), screeps.com allows 60 per
///   hour per token) governs, with the requests in flight counted as spent: the first request
///   goes out alone, its response telling how many are left; then up to three at once while
///   at least half of the window's requests are left (and the request budget allows); once
///   fewer than half are left, one at a time, spread evenly until the reset; far rooms may
///   use only [`STATS_FAR_SHARE`] percent of the window (42 of 60), so viewed and near rooms
///   always refresh; none goes out while none is left, and after HTTP 429 (of any request in
///   flight) the next waits for the reset (60 s when the server-wide limit refused it), the
///   others in flight completing meanwhile. Other failures are retried after 30 s. While the
///   limit holds back wanted overviews, [`WorldProgress::stats_paused_until`]
///   says until when ([`WorldEvent::StatsPaused`] reports each change). The user can lift
///   the limit early (screeps.com's reset page); [`Self::recheck_rate_limit`] then resumes
///   without waiting for the reset. It sends nothing of its own and does nothing while not
///   paused. While the token is exempt from rate limits
///   ([`Self::set_rate_limit_exempt_until`]), the budget does not apply: requests go out
///   at once, far rooms included, and nothing pauses; HTTP 429 still waits for the reset. At
///   the exemption's end the budget applies again.
/// - **Minimaps** (`roomMap2`): the first [`MINIMAP_CAP`] rooms of [`Interest::minimaps`]
///   are subscribed through a [`MapView`]; rooms that leave are released.
/// - **Portals**: no endpoint lists a world's portals, and worlds differ in where they stand
///   (sector centres such as `W5N5` and highway crossroads such as `W10N0` on both
///   screeps.com and `OpenScreeps`; also anywhere on screeps.com's highways, such as
///   `W42N0` ↔ `W42N10` on `shard0`). So while [`Interest::portals`] is set, every room of the
///   world whose portals are not known (or stale), but rooms the server has no terrain for,
///   is scanned, by tier (those of [`Interest::portals`] count as visible), rooms where
///   portals usually stand ([`may_have_portals`](super::may_have_portals): centres and
///   highways, 7 204 of the 33 124 rooms of `shard0`) first in the visible and background
///   tiers, each tier nearest the centre first. A scan makes no HTTP request: the room is
///   subscribed to `roomMap2`, whose first update (sent as soon as the server processes the
///   subscription) lists portal positions (`p`), and released when it arrives (or after
///   [`PORTAL_SCAN_TIMEOUT`], 30 s; scanned again 30 s later). Scans and live minimaps
///   together use up to [`MINIMAP_CAP`] subscriptions, all of them for scans while nothing
///   else needs them: each answer frees a subscription for the next room, a steady rate the
///   server sets by how fast it answers (scans waiting on a reconnect are sent again with the
///   reconnect's subscriptions, at most the cap; none starts over). Measured on screeps.com
///   `shard0` (October 2026): all 33 124 rooms scanned in 68 s with 256 in flight (about 490
///   rooms per second, half a second per answer, 5.8 MB over the socket), without a refusal or
///   a disconnect; a disposable `OpenScreeps` world of 441 rooms in 4 s. A live minimap
///   counts as a scan of its room, or of any room of the world whose portals appear or
///   vanish.
///
///   A scan tells where a room's portals stand, not where they lead: the room is reported
///   at once with its portals' tiles and pending destinations ([`Portal::destination`]
///   `None`), which is all the map needs to draw them, and counts as scanned
///   ([`WorldProgress::portals_scanned`]; [`WorldProgress::portals_pending`] counts those
///   waiting). One `GET /api/game/room-objects` per room brings the destinations: first the
///   viewed rooms and those of [`Interest::portal_destinations`] (the hovered and selected
///   room, the rooms whose arcs the map draws), in their order at the viewed priority of the
///   request budget; then all others in the background, at the background priority and at
///   most [`PORTAL_BACKGROUND_IN_FLIGHT`] of the [`PORTAL_IN_FLIGHT`] requests (so a hovered
///   room's request goes out at once), the rooms whose fetch tells the most portal tiles not
///   known otherwise first. The
///   endpoint has no limit of its own on screeps.com. A request that fails or
///   takes longer than [`PORTAL_FETCH_TIMEOUT`] holds back only its room: it is asked again
///   after 30 s, doubling with each failure in a row up to an hour, while the other rooms go
///   on. Paired portals save requests: on screeps.com every intra-shard portal lands exactly
///   on a portal of its destination room that leads back to it, with the same
///   [`Portal::expires_at`] (sampled on `shard0`, October 2026: 144 of 144 in `W42N0`,
///   `E31S40` and their partners, and the sector-centre pairs such as `E55N45` ↔ `E55N15`);
///   `OpenScreeps` generates such pairs too. So when exactly one known portal of the shard
///   (fetched, live or seeded with its destination, never inter-shard ones) lands on each
///   portal tile of a room's map view, that room's portals are inferred to lead back to them
///   (decay included), and it is not fetched; a room only partly accounted for is fetched.
///   To let one fetch tell its partner rather than race it, background destinations wait
///   while scans of rooms where portals usually stand are in flight, at most
///   [`PORTAL_SCAN_SETTLE`] each, and a room whose portal tiles have the shape of those of a
///   room being fetched waits for that fetch. A live room or a later fetch replaces inferred
///   portals ([`Self::portals_inferred`] tells them apart). Each scan is reported
///   ([`WorldEvent::Portals`]) and used for [`PORTAL_TTL`] (24 hours), [`PORTAL_QUIET_TTL`]
///   (7 days) for no portals in a room where they rarely stand (neither a sector centre nor
///   a highway), [`PORTAL_DECAYING_TTL`] (1 hour) for a room with a decaying portal, or until
///   a portal becomes unstable ([`Portal::expires_at`]), before the room is scanned again;
///   live minimaps and live rooms correct any room at once. [`Self::insert_portals`] seeds
///   scans from the app's cache (pending destinations included, fetched in the
///   background), stale after the same time since they were made.
/// - **Live rooms**: each update of a room wanted in detail updates what is known of it at
///   once, with no request: its overview (owner and level, reservation, safe mode and
///   mineral, see [`RoomStat`]; the status stays the known one) when it tells something
///   else ([`WorldEvent::Stats`] of that room, with a new [`RoomStat::fetched_at`]), and its
///   portals with their destinations, candidate or not ([`WorldEvent::Portals`] when they
///   differ). Both are then fresh: no overview or portal request goes out for a live room
///   but its viewed refresh. Data requested before a live update never overwrites it: a
///   later overview response only brings the status, and scans and room-objects responses
///   are dropped. Live minimaps update portal presence (above); they cannot tell owners (a
///   user's positions mix creeps and structures).
/// - **Detail** (`room`): the rooms of [`Interest::detail`] are subscribed in priority order
///   at once, with no request: their terrain comes from the terrain above (they are wanted
///   first, ahead of [`Interest::terrain`]) and is given to the subscription when it arrives
///   (the server's updates meanwhile become a snapshot then); a room the server does not
///   know is refused. While a subscribed room waits for its first update (or a refusal), at
///   most [`DETAIL_FIRST`] (5 s), background portal scans wait and at most
///   [`PORTAL_HELD_SCANS`] (4) are in flight: otherwise the whole-world scan's subscriptions
///   (up to [`MINIMAP_CAP`]) and their answers queue ahead of the room on the server and on
///   the socket, and a first visit's room took seconds longer to show.
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
/// Overviews, portals, minimaps and detail need a signed-in user: without one,
/// [`Interest::stats`], [`Interest::portals`] and [`Interest::minimaps`] are ignored and
/// detail rooms are refused.
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
    knowledge: WorldKnowledge,
    focus: Focus,
    terrain: Terrains,
    stats: Stats,
    portals: Portals,
    details: Details,
    /// The live updates of the latest poll, for the overviews and portals they show.
    live: Vec<(RoomCoord, Arc<RoomSnapshot>)>,
    /// Whether to plan again: something changed since the last plan.
    dirty: bool,
    /// Wakes the planning when a retry or an expiry is due.
    timer: Option<(Instant, Pin<Box<Sleep>>)>,
}

impl Work {
    /// Nothing known or wanted yet of the world `bounds`.
    fn new(bounds: RoomRect) -> Self {
        let knowledge = WorldKnowledge::new();
        Self {
            events: VecDeque::new(),
            focus: Focus::new(bounds),
            terrain: Terrains::new(bounds),
            stats: Stats::new(bounds, knowledge.clone()),
            portals: Portals::new(bounds, knowledge.clone()),
            knowledge,
            details: Details::default(),
            live: Vec::new(),
            dirty: false,
            timer: None,
        }
    }

    /// Polls every request, subscription and the timer; ready once something changed. Live
    /// rooms go first, so a response that arrives with a live update counts as older.
    fn poll(
        &mut self,
        cx: &mut Context<'_>,
        client: &overseer_net::Client,
    ) -> Poll<Result<(), Closed>> {
        let mut changed = self
            .details
            .poll(cx, &self.terrain, &mut self.events, &mut self.live)?;
        for (room, snapshot) in self.live.drain(..) {
            self.stats.live(room, &snapshot, &mut self.events);
            self.portals.live(room, &snapshot, &mut self.events);
        }
        changed |= self.terrain.poll(cx, &mut self.events)?;
        changed |= self.stats.poll(cx, client, &mut self.events)?;
        changed |= self.portals.poll(cx, &mut self.events)?;
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
        let work = Work::new(bounds);
        Self {
            status: overseer.status(),
            overseer,
            map,
            shard,
            bounds,
            interest: Interest::default(),
            minimaps,
            shown: HashSet::new(),
            work,
        }
    }

    /// What [`WorldMap`] knows of each room (overviews and portals), as a handle readable
    /// outside the task that owns the world map: the same data as [`Self::stat`] and
    /// [`Self::portals`], with a revision bumped at every change.
    #[must_use]
    pub fn knowledge(&self) -> WorldKnowledge {
        self.work.knowledge.clone()
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
    /// detail rooms, and queues terrain, overview and portal work; [`Self::next`] does the
    /// work and reports the changes (including the [`WorldEvent::MinimapGone`],
    /// [`WorldEvent::DetailGone`] and [`DetailState::Pending`] this queues).
    pub fn set_interest(&mut self, interest: &Interest) {
        if *interest == self.interest {
            return;
        }
        let user = self.overseer.user_id().is_some();
        let wanted = if user && self.minimaps.is_some() {
            &interest.portal_destinations[..]
        } else {
            &[]
        };
        if self.work.portals.set_wanted(wanted) {
            self.work.dirty = true;
        }
        // Only the destinations wanted changed (the hovered room, portals arriving in view):
        // nothing else to redo.
        let old = &self.interest;
        let same_but_destinations = interest.stats == old.stats
            && interest.portals == old.portals
            && interest.detail == old.detail
            && interest.minimaps == old.minimaps
            && interest.terrain == old.terrain;
        if same_but_destinations {
            self.interest
                .portal_destinations
                .clone_from(&interest.portal_destinations);
            return;
        }
        let work = &mut self.work;
        work.dirty = true;
        work.stats.rect = interest
            .stats
            .filter(|_| user)
            .and_then(|rect| rect.intersection(&self.bounds));
        let detail = unique(&interest.detail);
        let viewed = if user { detail.clone() } else { Vec::new() };
        work.stats.set_viewed(&viewed);
        work.focus = Focus::of(interest, viewed, self.bounds);
        let map = !interest.terrain.is_empty();
        if user {
            work.terrain.want(
                detail.iter().chain(&interest.terrain).copied(),
                &work.focus,
                map,
            );
        } else {
            work.terrain
                .want(interest.terrain.iter().copied(), &work.focus, map);
        }
        let minimaps = unique(&interest.minimaps);
        let live = minimaps.len().min(MINIMAP_CAP);
        work.stats.set_near(&minimaps[..live]);
        work.portals.set_rect(
            interest
                .portals
                .filter(|_| user && self.minimaps.is_some())
                .and_then(|rect| rect.intersection(&self.bounds)),
        );
        work.portals.refocus();
        work.portals.set_minimaps(live);
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

    /// Seeds the portals of `room` (empty: it has none) scanned at `scanned_at` (milliseconds
    /// since the Unix epoch, when its [`WorldEvent::Portals`] arrived) from the app's
    /// persistent cache: no [`WorldEvent::Portals`] reports them, and the room is scanned
    /// again once they are stale (see [`WorldMap`]). Ignored when a newer scan is known.
    pub fn insert_portals(&mut self, room: RoomCoord, portals: Arc<[Portal]>, scanned_at: u64) {
        self.work.portals.insert(room, portals, scanned_at);
        self.work.dirty = true;
    }

    /// After the user lifted the overviews' rate limit (screeps.com's reset page): while
    /// overview loading is paused ([`WorldProgress::stats_paused_until`]), forgets the stale
    /// limit ([`net::Client::forget_rate_limit`](crate::net::Client::forget_rate_limit)) and
    /// the wait for its reset, so the next request goes out at once regardless of the
    /// budget; its response's headers record the real limit, and loading resumes when it
    /// succeeds (it pauses again when the limit still holds). Returns whether it was paused;
    /// while not paused nothing changes and nothing is sent.
    pub fn recheck_rate_limit(&mut self) -> bool {
        let paused = self.work.stats.recheck(self.overseer.client());
        if paused {
            self.work.dirty = true;
        }
        paused
    }

    /// Tells the world that the session's token is exempt from rate limits until `until`
    /// (milliseconds since the Unix epoch; screeps.com's `noRatelimitUntil`, see
    /// [`Account::rate_limit_exemption`](crate::Account::rate_limit_exemption)), or not
    /// (`None`, or a time already past), and tells the session's client
    /// ([`net::Client::set_rate_limit_exempt_until`](crate::net::Client::set_rate_limit_exempt_until)),
    /// whose request budget then lets everything go (badges included). While exempt, nothing
    /// is paced (see [`WorldMap`]): no request budget, overviews ignore their hourly budget
    /// (no pause, no share kept for near rooms), each kind only within its in-flight cap;
    /// HTTP 429 still waits. A pause ends at once. From `until` on, the pacing applies again.
    /// The same `until` again changes nothing.
    pub fn set_rate_limit_exempt_until(&mut self, until: Option<u64>) {
        self.overseer.client().set_rate_limit_exempt_until(until);
        if self.work.stats.set_exempt_until(until, Instant::now()) {
            self.work.dirty = true;
        }
    }

    /// How much of what is wanted is known.
    #[must_use]
    pub fn progress(&self) -> WorldProgress {
        let (terrain_known, terrain_absent, terrain_wanted) = self.work.terrain.progress();
        let (stats_known, stats_wanted) = self.work.stats.progress();
        let (portals_scanned, portals_wanted) = self.work.portals.progress(&self.work.terrain);
        WorldProgress {
            terrain_known,
            terrain_absent,
            terrain_wanted,
            stats_known,
            stats_wanted,
            stats_paused_until: self.work.stats.paused(),
            portals_scanned,
            portals_wanted,
            portals_pending: self.work.portals.pending(),
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
            // Planning reports pauses of its own.
            if !self.work.events.is_empty() {
                continue;
            }
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
                        work.events.push_back(WorldEvent::Minimap(room, Arc::clone(&map)));
                        work.dirty |= work.portals.minimap(room, &map, &mut work.events);
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
        let mut budget = Budget::new(client);
        let at = (now, &work.focus, Server::of(client));
        let terrain = work.terrain.plan(&self.map, shard, at, &mut budget);
        let stats = work.stats.plan(
            &self.map,
            client,
            shard,
            at,
            (&mut budget, &mut work.events),
        );
        // A room wanted in detail and waiting for its first update holds scans back.
        let held = work.details.holding(now);
        let portals = work.portals.plan(
            &self.map,
            &self.overseer,
            shard,
            at,
            (&mut budget, &work.terrain),
            held.is_some(),
        );
        // The pacing changes back when the exemption ends.
        let exempt = work.stats.exempt_end(now);
        let due = [terrain, stats, portals, exempt, held]
            .into_iter()
            .flatten()
            .min();
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

    /// The overview of `room`, when fetched, seeded or shown by its live room (the same as
    /// [`WorldKnowledge::stat`]).
    #[must_use]
    pub fn stat(&self, room: RoomCoord) -> Option<Arc<RoomStat>> {
        self.work.knowledge.stat(room)
    }

    /// The portals of `room` (empty: it has none), when scanned, seeded or shown by its live
    /// room (the same as [`WorldKnowledge::portals`]).
    #[must_use]
    pub fn portals(&self, room: RoomCoord) -> Option<Arc<[Portal]>> {
        self.work.knowledge.portals(room)
    }

    /// Whether the known portals of `room` were inferred from the paired portals that lead to
    /// it, rather than fetched or shown by its live room (see [`WorldMap`]).
    #[must_use]
    pub fn portals_inferred(&self, room: RoomCoord) -> bool {
        self.work.portals.is_inferred(room)
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
    /// whose terrain stays cached), both at once: screeps.com shards are centred on them, the
    /// upstream default world and `OpenScreeps` worlds lie north-west of them (see
    /// [`RoomRect`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when a request fails.
    pub async fn world(&self, shard: Option<&str>) -> Result<WorldMap, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        // Both at once: the probe does not depend on the size.
        let (size, centre) = futures_util::future::try_join(
            self.world_size(shard.as_deref()),
            self.terrain(CENTRE, shard.as_deref()),
        )
        .await?;
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
