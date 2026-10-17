//! Portals of the world map: every room scanned through short-lived `roomMap2` subscriptions
//! (rooms where portals usually stand first in the background), then one room-objects request
//! per room with portals for their destinations, unless its paired portals tell them; viewed
//! and visible rooms first, the rest of the world in the background; live rooms show their
//! portals directly.

use std::cell::Cell;

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::StreamExt as _;
use overseer_net::api::WorldOperation;
use overseer_net::core::api::ApiError;
use overseer_net::core::socket::{Channel, Payload};
use overseer_net::model::world::RoomMap;
use overseer_net::rt::{self, BoxFuture, Instant};
use overseer_net::{Client, Error as NetError};

use super::focus::{Budget, Focus, Server, Tier, distance};
use super::stats::epoch_millis;
use super::terrain::{Terrains, reset_at};
use super::{HELD_ROOM_MAPS, MINIMAP_CAP, WorldEvent};
use crate::map::portal::portals_of;
use crate::map::{
    Map, Portal, PortalDestination, RoomCoord, RoomRect, WorldKnowledge, may_have_portals,
};
use crate::{Closed, Error, Overseer, RoomSnapshot, Watch};

/// How long the portals of a room (or their absence) are used before the room is scanned
/// again, unless one of them becomes unstable earlier ([`Portal::expires_at`]).
pub const PORTAL_TTL: Duration = Duration::from_hours(24);

/// How long the absence of portals is used in a room where portals rarely stand (neither a
/// sector centre nor a highway, see [`may_have_portals`]): such rooms rarely change, and a
/// live minimap or live room corrects them at once.
pub const PORTAL_QUIET_TTL: Duration = Duration::from_hours(24 * 7);

/// How long the portals of a room with a decaying portal ([`Portal::decay_time`]) are used:
/// its end is counted in game ticks, so the room is looked at again within the hour.
pub const PORTAL_DECAYING_TTL: Duration = Duration::from_hours(1);

/// How long a room-objects request for portal destinations may take; then it counts as failed
/// (the room is asked again later, see [`WorldMap`](super::WorldMap)).
pub const PORTAL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Room-objects requests for portal destinations in flight at once, on every server: enough
/// to reach the request budget at screeps.com's latencies (0.3 to 2 s each).
pub const PORTAL_IN_FLIGHT: usize = 6;

/// Of [`PORTAL_IN_FLIGHT`], the most background room-objects requests: the rest is kept for
/// the destinations wanted now (a hovered room), which then go out at once.
pub const PORTAL_BACKGROUND_IN_FLIGHT: usize = 4;

/// How long a scan subscription waits for the room's first `roomMap2` update before it is
/// released; the room is scanned again 30 s later. Servers answer a subscription as soon as
/// they process it: screeps.com within a fraction of a second, `OpenScreeps` through its state
/// queue, behind the tick being processed (0.5 to 0.9 s on an idle disposable world, seconds
/// when busy).
pub const PORTAL_SCAN_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the destinations of rooms with portals wait for scans still in flight of rooms of
/// the same tier (or a higher one), which may be their partners (see
/// [`WorldMap`](super::WorldMap)): screeps.com answers scans within a fraction of a second,
/// `OpenScreeps` within one, so a fetch learns its partner's map view first and the partner
/// is inferred instead of fetched too. Viewed rooms never wait.
pub const PORTAL_SCAN_SETTLE: Duration = Duration::from_secs(3);

/// Wait before a room whose scan timed out is scanned again.
const SCAN_RETRY: Duration = Duration::from_secs(30);

/// Wait after a room's room-objects request failed (for another reason than the rate limit)
/// before that room is asked again; it doubles with each failure in a row, up to
/// [`RETRY_MAX`]. Other rooms are asked meanwhile.
const RETRY: Duration = Duration::from_secs(30);

/// Longest wait before a room whose room-objects requests keep failing is asked again.
const RETRY_MAX: Duration = Duration::from_hours(1);

/// Wait after HTTP 429 without the endpoint's limit (the server-wide limit counts per minute).
const GLOBAL_RETRY: Duration = Duration::from_mins(1);

/// Shortest wait after HTTP 429 with the endpoint's limit.
const MIN_LIMIT_WAIT: Duration = Duration::from_secs(1);

/// Room-objects requests of one room that failed in a row.
struct Failure {
    attempts: u32,
    /// Not asked again before this.
    until: Instant,
}

/// When a room's portals were learnt and when they go stale (the portals themselves are in
/// the [`WorldKnowledge`]).
struct Known {
    /// Whether the room has portals.
    some: bool,
    /// Inferred from the portals of the rooms they lead to, not fetched or shown live.
    inferred: bool,
    /// When the room was scanned (or a live room showed them), in milliseconds since the Unix
    /// epoch.
    scanned_at: u64,
    /// When a live room last showed them, in milliseconds since the Unix epoch: scans and
    /// requests started before are dropped.
    live_at: Option<u64>,
    /// Scanned again from then on.
    stale_at: Instant,
}

/// A scan subscription waiting for the room's first update.
struct Scan {
    room: RoomCoord,
    /// When it was subscribed, in milliseconds since the Unix epoch.
    started_at: u64,
    watch: Watch<RoomMap>,
    deadline: Instant,
    /// Until then the fetches of rooms of its tier wait for it (see [`PORTAL_SCAN_SETTLE`]).
    settled: Instant,
}

/// A room-objects request in flight.
struct Fetch {
    room: RoomCoord,
    /// When it went out, in milliseconds since the Unix epoch.
    sent_at: u64,
    /// `None` when it took longer than [`PORTAL_FETCH_TIMEOUT`].
    request: BoxFuture<'static, Option<Result<Vec<Portal>, Error>>>,
}

/// A portal tile: `(x, y)`.
type Tile = (u8, u8);

/// The portals known, the scans in flight and the rooms waiting for their destinations.
pub(super) struct Portals {
    known: HashMap<RoomCoord, Known>,
    /// Where the portals are kept and read.
    knowledge: WorldKnowledge,
    /// Every room of the world: all are scanned (but those known to have no terrain) while
    /// [`Self::rect`] is set.
    world: RoomRect,
    /// The rooms whose portals are wanted, inside the world; `None` without a signed-in user
    /// (`roomMap2` needs one).
    rect: Option<RoomRect>,
    /// The rooms to scan, in order (see [`Self::plan`]); rooms known, absent or scanning
    /// meanwhile are skipped when taken.
    queue: VecDeque<RoomCoord>,
    /// The queue is to be rebuilt: the wanted rooms or the view changed.
    reorder: bool,
    /// When the queue is to be rebuilt for rooms going stale or due a retry.
    next_check: Option<Instant>,
    /// The latest [`Self::progress`] and the count of absent rooms it was computed with;
    /// cleared when what is wanted changes.
    progress: Cell<Option<(usize, (usize, usize))>>,
    /// Rooms whose portals became known since the latest [`Self::progress`]: it counts them
    /// then, rather than every wanted room again after each scan.
    learnt: Cell<Vec<RoomCoord>>,
    /// The live minimaps, which take their share of [`MINIMAP_CAP`].
    minimaps: usize,
    scans: Vec<Scan>,
    /// Rooms whose map view shows portals, waiting for their destinations.
    found: HashSet<RoomCoord>,
    /// Rooms whose destinations are wanted now, most important first (see
    /// [`Self::set_wanted`]).
    wanted: Vec<RoomCoord>,
    /// The portal tiles of each room's latest map view that showed some.
    tiles: HashMap<RoomCoord, Vec<Tile>>,
    /// The fetched or live portals of the shard (not inferred ones) by the room and tile they
    /// lead to.
    incoming: HashMap<RoomCoord, HashMap<Tile, Vec<Portal>>>,
    /// Rooms whose scan timed out, not scanned again before then.
    retry: HashMap<RoomCoord, Instant>,
    fetches: Vec<Fetch>,
    /// Rooms whose room-objects request failed (for another reason than the rate limit): how
    /// often in a row, and when to ask again.
    failed: HashMap<RoomCoord, Failure>,
    /// No room-objects request before this (the rate limit).
    not_before: Option<Instant>,
}

impl Portals {
    /// Nothing known or wanted yet of the world `world`; portals go to `knowledge`.
    pub(super) fn new(world: RoomRect, knowledge: WorldKnowledge) -> Self {
        Self {
            known: HashMap::new(),
            knowledge,
            world,
            rect: None,
            progress: Cell::new(None),
            learnt: Cell::new(Vec::new()),
            queue: VecDeque::new(),
            reorder: true,
            next_check: None,
            minimaps: 0,
            scans: Vec::new(),
            found: HashSet::new(),
            wanted: Vec::new(),
            tiles: HashMap::new(),
            incoming: HashMap::new(),
            retry: HashMap::new(),
            fetches: Vec::new(),
            failed: HashMap::new(),
            not_before: None,
        }
    }

    /// Makes `rect` the rooms whose portals are wanted (inside the world; `None`: none).
    pub(super) fn set_rect(&mut self, rect: Option<RoomRect>) {
        if self.rect != rect {
            self.rect = rect;
            self.progress.set(None);
            self.reorder = true;
        }
    }

    /// The view changed: the scans still to come follow its tiers.
    pub(super) const fn refocus(&mut self) {
        self.reorder = true;
    }

    /// Rebuilds the queue of rooms to scan: every room of the world neither known (fresh),
    /// absent (`terrain`), waiting for a retry, scanned nor waiting for its destinations, by
    /// `key`; and when to rebuild it again (a room goes stale or its retry is due).
    fn requeue<K: Ord>(&mut self, now: Instant, terrain: &Terrains, key: &impl Fn(RoomCoord) -> K) {
        let mut next = Due::default();
        let mut rooms = Vec::new();
        for room in self.world.iter() {
            if terrain.is_absent(room) {
                continue;
            }
            if let Some(known) = self.known.get(&room)
                && known.stale_at > now
            {
                next.at(known.stale_at);
            } else if let Some(&at) = self.retry.get(&room) {
                next.at(at);
            } else if !self.found.contains(&room) {
                rooms.push(room);
            }
        }
        rooms.sort_by_cached_key(|&room| key(room));
        self.queue = rooms.into();
        self.reorder = false;
        self.next_check = next.0;
    }

    /// Records how many minimaps are live: scans use the rest of [`MINIMAP_CAP`].
    pub(super) const fn set_minimaps(&mut self, count: usize) {
        self.minimaps = count;
    }

    /// Makes `rooms` (most important first) the rooms whose destinations are wanted now:
    /// they are fetched before all others but the viewed rooms. Returns whether they changed.
    pub(super) fn set_wanted(&mut self, rooms: &[RoomCoord]) -> bool {
        if self.wanted == rooms {
            return false;
        }
        rooms.clone_into(&mut self.wanted);
        true
    }

    /// How many rooms with portals wait for their destinations.
    pub(super) fn pending(&self) -> usize {
        self.found.len()
    }

    /// Whether the known portals of `room` were inferred from the rooms they lead to.
    pub(super) fn is_inferred(&self, room: RoomCoord) -> bool {
        self.known.get(&room).is_some_and(|known| known.inferred)
    }

    /// Seeds the portals of `room` scanned at `scanned_at` (milliseconds since the Unix epoch)
    /// from the app's cache. Ignored when a newer scan is known. Seeded portals whose
    /// destination is pending are fetched (or inferred) in the background; the others tell
    /// the portals they lead to.
    pub(super) fn insert(&mut self, room: RoomCoord, portals: Arc<[Portal]>, scanned_at: u64) {
        if self
            .known
            .get(&room)
            .is_some_and(|known| known.scanned_at >= scanned_at)
        {
            return;
        }
        let stale_at = stale_at(room, &portals, scanned_at, Instant::now(), epoch_millis());
        let learnt = self
            .known
            .insert(
                room,
                Known {
                    some: !portals.is_empty(),
                    inferred: false,
                    scanned_at,
                    live_at: None,
                    stale_at,
                },
            )
            .is_none();
        if portals.iter().any(|portal| portal.destination.is_none()) {
            self.tiles.insert(
                room,
                portals.iter().map(|portal| (portal.x, portal.y)).collect(),
            );
            self.found.insert(room);
        } else {
            self.found.remove(&room);
            for portal in portals.iter() {
                if let Some((to, tile)) = lands(portal) {
                    self.incoming
                        .entry(to)
                        .or_default()
                        .entry(tile)
                        .or_default()
                        .push(portal.clone());
                }
            }
        }
        self.knowledge.set_portals(room, portals);
        self.check_at(stale_at);
        if learnt {
            self.learn(room);
        }
    }

    /// Counts `room`, whose portals were not known before, at the next [`Self::progress`].
    fn learn(&self, room: RoomCoord) {
        let mut learnt = self.learnt.take();
        learnt.push(room);
        self.learnt.set(learnt);
    }

    /// The wanted rooms (but those `terrain` knows the server has no terrain for) whose
    /// portals are known (possibly stale), and all of them.
    pub(super) fn progress(&self, terrain: &Terrains) -> (usize, usize) {
        let learnt = self.learnt.take();
        let Some(rect) = self.rect else {
            return (0, 0);
        };
        let absent = terrain.absent_count();
        if let Some((seen, (known, wanted))) = self.progress.get()
            && seen == absent
        {
            let known = known
                + learnt
                    .iter()
                    .filter(|&&room| rect.contains(room) && !terrain.is_absent(room))
                    .count();
            self.progress.set(Some((absent, (known, wanted))));
            return (known, wanted);
        }
        let progress = rect.iter().filter(|&room| !terrain.is_absent(room)).fold(
            (0, 0),
            |(known, wanted), room| {
                (
                    known + usize::from(self.known.contains_key(&room)),
                    wanted + 1,
                )
            },
        );
        self.progress.set(Some((absent, progress)));
        progress
    }

    /// The live room `snapshot` of `room` arrived: its portals (with their destinations) are
    /// known at once, whether or not it is a candidate, and reported
    /// ([`WorldEvent::Portals`]) when they differ from the known ones (or were inferred).
    /// Scans and requests of the room in flight are dropped.
    pub(super) fn live(
        &mut self,
        room: RoomCoord,
        snapshot: &RoomSnapshot,
        events: &mut VecDeque<WorldEvent>,
    ) {
        let portals = portals_of(&snapshot.objects);
        self.found.remove(&room);
        self.scans.retain(|scan| scan.room != room);
        let millis = epoch_millis();
        let same = self
            .knowledge
            .portals(room)
            .is_some_and(|known| *known == *portals);
        match self.known.get_mut(&room) {
            Some(known) if same && !known.inferred => {
                known.live_at = Some(millis);
                known.stale_at =
                    known
                        .stale_at
                        .max(stale_at(room, &portals, millis, Instant::now(), millis));
            }
            _ => self.record(room, portals, Some(millis), events),
        }
    }

    /// A live minimap of `room` arrived: it counts as a scan of a room of the world not known
    /// yet (or whose portals appeared or vanished meanwhile), whether or not it is a candidate.
    /// Returns whether anything changed.
    pub(super) fn minimap(
        &mut self,
        room: RoomCoord,
        view: &RoomMap,
        events: &mut VecDeque<WorldEvent>,
    ) -> bool {
        if self.rect.is_none()
            || !self.world.contains(room)
            || self.found.contains(&room)
            || self.fetches.iter().any(|fetch| fetch.room == room)
        {
            return false;
        }
        let shows = shows_portals(view);
        let fresh = self
            .known
            .get(&room)
            .filter(|known| known.stale_at > Instant::now());
        match fresh {
            Some(known) if known.some == shows => return false,
            _ => {}
        }
        self.scans.retain(|scan| scan.room != room);
        self.scanned(room, view, events);
        true
    }

    /// Releases the scans that timed out, subscribes the rooms of the world neither known
    /// (fresh), absent (`terrain`), scanned nor waiting for their destinations, by tier
    /// (viewed, then those of the wanted rooms or visible, then the background), the rooms
    /// where portals usually stand ([`may_have_portals`]) first but for viewed ones, nearest
    /// the centre first, as far as [`MINIMAP_CAP`] allows (while `held`, the background
    /// waits and at most [`HELD_ROOM_MAPS`] are in flight); then requests destinations
    /// ([`Self::fetch`]). Returns when to plan again (a scan times out, a retry or a request
    /// is due, or portals go stale).
    pub(super) fn plan(
        &mut self,
        map: &Map,
        overseer: &Overseer,
        shard: Option<&str>,
        (now, focus, _): (Instant, &Focus, Server),
        (budget, terrain): (&mut Budget<'_>, &Terrains),
        held: bool,
    ) -> Option<Instant> {
        let Some(rect) = self.rect else {
            self.scans.clear();
            return None;
        };
        let tier = |room: RoomCoord| {
            let tier = focus.tier(room);
            if tier == Tier::Background && rect.contains(room) {
                Tier::Visible
            } else {
                tier
            }
        };
        let key = |room: RoomCoord| {
            let (_, rank, distance, y, x) = focus.key(room);
            let tier = tier(room);
            let unusual = tier != Tier::Viewed && !may_have_portals(room);
            (tier, rank, unusual, distance, y, x)
        };
        let mut due = Due::default();
        self.release_timed_out(now, &mut due);
        if self.reorder || self.next_check.is_some_and(|at| at <= now) {
            self.requeue(now, terrain, &key);
        }
        if let Some(at) = self.next_check {
            due.at(at);
        }
        let mut free = MINIMAP_CAP.saturating_sub(self.minimaps + self.scans.len());
        if held {
            free = free.min(HELD_ROOM_MAPS.saturating_sub(self.scans.len()));
        }
        let millis = epoch_millis();
        while free > 0
            && let Some(room) = self.queue.pop_front()
        {
            // The queue goes by tier: from the first background room on, all wait.
            if held && tier(room) == Tier::Background {
                self.queue.push_front(room);
                break;
            }
            let fresh = self
                .known
                .get(&room)
                .is_some_and(|known| known.stale_at > now);
            if fresh
                || terrain.is_absent(room)
                || self.found.contains(&room)
                || self.scans.iter().any(|scan| scan.room == room)
                || self.fetches.iter().any(|fetch| fetch.room == room)
            {
                continue;
            }
            let channel = Channel::RoomMap {
                shard: shard.map(str::to_owned),
                room: room.to_string(),
            };
            let watch = overseer.watch(channel, |payload| match payload {
                Payload::RoomMap(view) => Some(view),
                _ => None,
            });
            self.scans.push(Scan {
                room,
                started_at: millis,
                watch,
                deadline: now + PORTAL_SCAN_TIMEOUT,
                settled: now + PORTAL_SCAN_SETTLE,
            });
            due.at(now + PORTAL_SCAN_TIMEOUT);
            free -= 1;
        }
        if self.fetches.len() < PORTAL_IN_FLIGHT {
            let ready = self.ready_at(overseer.client(), now);
            if ready > now {
                due.at(ready);
            } else {
                self.not_before = None;
                self.fetch(map, shard, (now, millis, focus), budget, &mut due);
            }
        }
        due.0
    }

    /// Requests the destinations of rooms with portals, up to [`PORTAL_IN_FLIGHT`] at once,
    /// as far as the request `budget` allows: first the viewed rooms and the rooms whose
    /// destinations are wanted now ([`Self::set_wanted`]), in their order, at the viewed
    /// priority; then the others at the background priority, up to
    /// [`PORTAL_BACKGROUND_IN_FLIGHT`] in flight, those whose fetch tells the most
    /// portal tiles not known otherwise first, nearest the centre first. The others wait
    /// while scans of rooms where portals usually stand are in flight, at most
    /// [`PORTAL_SCAN_SETTLE`], and while a room with the same shape of portal tiles is being
    /// fetched (they may be partners, told by that fetch). A room whose fetch failed waits
    /// for its retry.
    fn fetch(
        &mut self,
        map: &Map,
        shard: Option<&str>,
        (now, millis, focus): (Instant, u64, &Focus),
        budget: &mut Budget<'_>,
        due: &mut Due,
    ) {
        let mut waiting = Due::default();
        let available = |portals: &Self, room: RoomCoord, waiting: &mut Due| {
            if !portals.found.contains(&room)
                || portals.fetches.iter().any(|fetch| fetch.room == room)
            {
                return false;
            }
            match portals.failed.get(&room) {
                Some(failure) if failure.until > now => {
                    waiting.at(failure.until);
                    false
                }
                _ => true,
            }
        };
        let mut urgent = Vec::new();
        for &room in focus.viewed().iter().chain(&self.wanted) {
            if !urgent.contains(&room) && available(self, room, &mut waiting) {
                urgent.push(room);
            }
        }
        for &room in &urgent {
            if self.fetches.len() >= PORTAL_IN_FLIGHT {
                return;
            }
            if let Err(at) = budget.take(Tier::Viewed) {
                due.at(at);
                return;
            }
            self.start(map, shard, room, millis);
        }
        if self.fetches.len() >= PORTAL_IN_FLIGHT {
            return;
        }
        if let Some(settled) = self
            .scans
            .iter()
            .filter(|scan| scan.settled > now && may_have_portals(scan.room))
            .map(|scan| scan.settled)
            .min()
        {
            due.at(settled);
            return;
        }
        let centre = focus.centre();
        if self.fetches.len() >= PORTAL_BACKGROUND_IN_FLIGHT || self.found.is_empty() {
            return;
        }
        // Planning runs after every scan: the rooms waiting are ordered only when one of them
        // may go out.
        if let Some(at) = budget.ready_at(Tier::Background) {
            due.at(at);
            return;
        }
        let mut rest = self
            .found
            .iter()
            .copied()
            .filter(|room| !urgent.contains(room))
            .collect::<Vec<_>>();
        rest.retain(|&room| available(self, room, &mut waiting));
        let mut rest = rest
            .into_iter()
            .map(|room| {
                let key = (
                    Reverse(self.untold(room)),
                    distance(room, centre),
                    room.y,
                    room.x,
                );
                (key, room)
            })
            .collect::<Vec<_>>();
        rest.sort_unstable();
        let mut flying = self
            .fetches
            .iter()
            .map(|fetch| self.shape(fetch.room))
            .collect::<Vec<_>>();
        for (_, room) in rest {
            if self.fetches.len() >= PORTAL_BACKGROUND_IN_FLIGHT {
                return;
            }
            let shape = self.shape(room);
            if flying.contains(&shape) {
                continue;
            }
            if let Err(at) = budget.take(Tier::Background) {
                due.at(at);
                return;
            }
            self.start(map, shard, room, millis);
            flying.push(shape);
        }
        if let Some(at) = waiting.0 {
            due.at(at);
        }
    }

    /// Requests the destinations of `room` now.
    fn start(&mut self, map: &Map, shard: Option<&str>, room: RoomCoord, millis: u64) {
        let (map, shard, name) = (map.clone(), shard.map(str::to_owned), room.to_string());
        let request = Box::pin(async move {
            rt::timeout(PORTAL_FETCH_TIMEOUT, map.portals(&name, shard.as_deref()))
                .await
                .ok()
        });
        self.fetches.push(Fetch {
            room,
            sent_at: millis,
            request,
        });
    }

    /// The shape of the portal tiles of `room`'s map view: the tiles moved to the origin, in
    /// order (paired clusters have the same shape).
    fn shape(&self, room: RoomCoord) -> Vec<Tile> {
        let tiles = self.tiles.get(&room).map_or(&[][..], Vec::as_slice);
        let x = tiles.iter().map(|tile| tile.0).min().unwrap_or(0);
        let y = tiles.iter().map(|tile| tile.1).min().unwrap_or(0);
        let mut shape = tiles
            .iter()
            .map(|tile| (tile.0 - x, tile.1 - y))
            .collect::<Vec<_>>();
        shape.sort_unstable();
        shape
    }

    /// Releases the scans that timed out (scanned again after [`SCAN_RETRY`]) and forgets
    /// the retries that are due; adds the deadlines left to `due`.
    fn release_timed_out(&mut self, now: Instant, due: &mut Due) {
        let mut index = 0;
        while index < self.scans.len() {
            let scan = &self.scans[index];
            if scan.deadline <= now {
                self.retry.insert(scan.room, now + SCAN_RETRY);
                self.check_at(now + SCAN_RETRY);
                self.scans.swap_remove(index);
            } else {
                due.at(scan.deadline);
                index += 1;
            }
        }
        self.retry.retain(|_, at| *at > now);
    }

    /// The portal tiles of `room`'s map view that no known portal leads to.
    fn untold(&self, room: RoomCoord) -> usize {
        let tiles = self.tiles.get(&room).map_or(&[][..], Vec::as_slice);
        let incoming = self.incoming.get(&room);
        tiles
            .iter()
            .filter(|tile| {
                incoming
                    .and_then(|incoming| incoming.get(*tile))
                    .is_none_or(|sources| sources.len() != 1)
            })
            .count()
    }

    /// Completes the scans that were answered (a room whose map view shows no portal is known
    /// to have none: [`WorldEvent::Portals`] with no portal) and the room-objects requests
    /// ([`WorldEvent::Portals`] with its portals); a live room's portals shown since win.
    /// Returns whether anything changed.
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        events: &mut VecDeque<WorldEvent>,
    ) -> Result<bool, Closed> {
        let mut changed = false;
        let mut index = 0;
        while index < self.scans.len() {
            match self.scans[index].watch.poll_next_unpin(cx) {
                Poll::Pending => index += 1,
                Poll::Ready(None) => return Err(Closed),
                Poll::Ready(Some(view)) => {
                    let scan = self.scans.swap_remove(index);
                    if !self.shown_live_since(scan.room, scan.started_at) {
                        self.scanned(scan.room, &view, events);
                    }
                    changed = true;
                }
            }
        }
        let mut index = 0;
        while index < self.fetches.len() {
            let Poll::Ready(result) = self.fetches[index].request.as_mut().poll(cx) else {
                index += 1;
                continue;
            };
            let Fetch { room, sent_at, .. } = self.fetches.swap_remove(index);
            changed = true;
            let now = Instant::now();
            let wait = match result {
                Some(Ok(portals)) => {
                    self.found.remove(&room);
                    self.failed.remove(&room);
                    if !self.shown_live_since(room, sent_at) {
                        self.record(room, portals, None, events);
                    }
                    None
                }
                Some(Err(Error::Closed)) => return Err(Closed),
                Some(Err(Error::Net(NetError::Api(ApiError::RateLimited(Some(limit)))))) => {
                    Some(reset_at(limit.reset, now).max(now + MIN_LIMIT_WAIT))
                }
                Some(Err(Error::Net(NetError::Api(ApiError::RateLimited(None))))) => {
                    Some(now + GLOBAL_RETRY)
                }
                // Timed out or failed: only this room waits, longer after each failure in a
                // row; the others go on.
                None | Some(Err(_)) => {
                    let failure = self.failed.entry(room).or_insert(Failure {
                        attempts: 0,
                        until: now,
                    });
                    failure.attempts = failure.attempts.saturating_add(1);
                    let backoff = RETRY
                        .saturating_mul(1 << failure.attempts.saturating_sub(1).min(7))
                        .min(RETRY_MAX);
                    failure.until = now + backoff;
                    None
                }
            };
            if let Some(wait) = wait {
                self.not_before = Some(self.not_before.map_or(wait, |before| before.max(wait)));
            }
        }
        Ok(changed)
    }

    /// Whether a live room showed the portals of `room` after `since` (milliseconds since the
    /// Unix epoch).
    fn shown_live_since(&self, room: RoomCoord, since: u64) -> bool {
        self.known
            .get(&room)
            .and_then(|known| known.live_at)
            .is_some_and(|live_at| live_at >= since)
    }

    /// A room's map view arrived. Without portals, the room has none. With portals, their
    /// tiles are known at once and reported with pending destinations (the map draws them),
    /// unless the known portals stand on the same tiles (their destinations stay shown until
    /// fetched again); the destinations are wanted (unless its paired portals tell them).
    fn scanned(&mut self, room: RoomCoord, view: &RoomMap, events: &mut VecDeque<WorldEvent>) {
        let mut tiles = view
            .p
            .iter()
            .flatten()
            .filter_map(|&[x, y]| Some((u8::try_from(x).ok()?, u8::try_from(y).ok()?)))
            .collect::<Vec<_>>();
        if tiles.is_empty() {
            self.tiles.remove(&room);
            self.record(room, Vec::new(), None, events);
            return;
        }
        tiles.sort_unstable_by_key(|&(x, y)| (y, x));
        tiles.dedup();
        let unchanged = self.knowledge.portals(room).is_some_and(|known| {
            known
                .iter()
                .map(|portal| (portal.x, portal.y))
                .eq(tiles.iter().copied())
        });
        self.tiles.insert(room, tiles.clone());
        self.found.insert(room);
        if !self.infer(room, events) && !unchanged {
            let pending = tiles
                .into_iter()
                .map(|(x, y)| Portal::pending(room, x, y))
                .collect();
            self.record(room, pending, None, events);
        }
    }

    /// Infers the portals of `room`, waiting for its destinations, when every portal tile of
    /// its map view is the destination of a known portal of the shard: each leads back to the
    /// portal that leads to it, unstable and decaying alike (screeps.com places portals in
    /// such pairs). Returns whether it did.
    fn infer(&mut self, room: RoomCoord, events: &mut VecDeque<WorldEvent>) -> bool {
        if !self.found.contains(&room) {
            return false;
        }
        let (Some(tiles), Some(incoming)) = (self.tiles.get(&room), self.incoming.get(&room))
        else {
            return false;
        };
        let Some(mut portals) = tiles
            .iter()
            .map(|tile| {
                // Exactly one known portal lands on the tile: two would not be a pair.
                let [source] = incoming.get(tile)?.as_slice() else {
                    return None;
                };
                Some(Portal {
                    room,
                    x: tile.0,
                    y: tile.1,
                    destination: Some(PortalDestination {
                        room: source.room,
                        shard: None,
                        x: Some(source.x),
                        y: Some(source.y),
                    }),
                    expires_at: source.expires_at,
                    decay_time: source.decay_time,
                })
            })
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        portals.sort_unstable_by_key(|portal| (portal.y, portal.x));
        self.found.remove(&room);
        self.failed.remove(&room);
        self.store(room, portals, None, true, events);
        true
    }

    /// Stores the portals of `room` fetched or scanned now (shown by a live room at
    /// `live_at`) and queues them; then infers the portals of the rooms they lead to.
    fn record(
        &mut self,
        room: RoomCoord,
        portals: Vec<Portal>,
        live_at: Option<u64>,
        events: &mut VecDeque<WorldEvent>,
    ) {
        if let Some(old) = self.knowledge.portals(room) {
            for portal in old.iter() {
                if let Some((to, tile)) = lands(portal)
                    && let Some(sources) = self
                        .incoming
                        .get_mut(&to)
                        .and_then(|incoming| incoming.get_mut(&tile))
                {
                    sources.retain(|from| from.room != room);
                }
            }
        }
        let mut targets = Vec::new();
        for portal in &portals {
            if let Some((to, tile)) = lands(portal) {
                self.incoming
                    .entry(to)
                    .or_default()
                    .entry(tile)
                    .or_default()
                    .push(portal.clone());
                if !targets.contains(&to) {
                    targets.push(to);
                }
            }
        }
        self.store(room, portals, live_at, false, events);
        for target in targets {
            self.infer(target, events);
        }
    }

    /// Stores the portals of `room` learnt now and queues them.
    fn store(
        &mut self,
        room: RoomCoord,
        portals: Vec<Portal>,
        live_at: Option<u64>,
        inferred: bool,
        events: &mut VecDeque<WorldEvent>,
    ) {
        let scanned_at = epoch_millis();
        let portals = Arc::<[Portal]>::from(portals);
        let stale_at = stale_at(room, &portals, scanned_at, Instant::now(), scanned_at);
        let some = !portals.is_empty() || self.tiles.contains_key(&room);
        let learnt = self
            .known
            .insert(
                room,
                Known {
                    // A room whose map view shows portals though none came (a portal its
                    // objects do not list) counts as having some, so its live minimap does not
                    // ask again.
                    some,
                    inferred,
                    scanned_at,
                    live_at,
                    stale_at,
                },
            )
            .is_none();
        self.check_at(stale_at);
        if learnt {
            self.learn(room);
        }
        self.knowledge.set_portals(room, Arc::clone(&portals));
        events.push_back(WorldEvent::Portals(room, portals));
    }

    /// Rebuilds the scan queue at `at` at the latest.
    fn check_at(&mut self, at: Instant) {
        self.next_check = Some(self.next_check.map_or(at, |next| next.min(at)));
    }

    /// When a room-objects request may go out: after a refusal and, when the endpoint's
    /// latest limit has no request left, after its reset.
    fn ready_at(&self, client: &Client, now: Instant) -> Instant {
        let mut ready = self.not_before.unwrap_or(now);
        if let Some(limit) = WorldOperation::RoomObjects.rate_limit(client)
            && limit.remaining == 0
        {
            ready = ready.max(reset_at(limit.reset, now));
        }
        ready
    }
}

/// The earliest of several instants.
#[derive(Default)]
struct Due(Option<Instant>);

impl Due {
    fn at(&mut self, at: Instant) {
        self.0 = Some(self.0.map_or(at, |due| due.min(at)));
    }
}

/// The room and tile a portal lands on in the same shard; `None` for inter-shard portals,
/// which name only the room, and pending ones.
fn lands(portal: &Portal) -> Option<(RoomCoord, Tile)> {
    let destination = portal.destination.as_ref()?;
    if destination.shard.is_some() || destination.room == portal.room {
        return None;
    }
    Some((destination.room, (destination.x?, destination.y?)))
}

/// Whether a map view lists portal positions.
fn shows_portals(view: &RoomMap) -> bool {
    view.p
        .as_ref()
        .is_some_and(|positions| !positions.is_empty())
}

/// When the portals of `room` scanned at `scanned_at` go stale, as an [`Instant`] (`now` is
/// `now_ms` milliseconds since the Unix epoch): after [`PORTAL_TTL`] ([`PORTAL_DECAYING_TTL`]
/// with a decaying portal, [`PORTAL_QUIET_TTL`] for no portals in a room where they rarely
/// stand), or when the first becomes unstable ([`PORTAL_DECAYING_TTL`] after the scan when it
/// already was).
fn stale_at(
    room: RoomCoord,
    portals: &[Portal],
    scanned_at: u64,
    now: Instant,
    now_ms: u64,
) -> Instant {
    let millis = |duration: Duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    let decaying = scanned_at.saturating_add(millis(PORTAL_DECAYING_TTL));
    let mut at = if portals.iter().any(|portal| portal.decay_time.is_some()) {
        decaying
    } else if portals.is_empty() && !may_have_portals(room) {
        scanned_at.saturating_add(millis(PORTAL_QUIET_TTL))
    } else {
        scanned_at.saturating_add(millis(PORTAL_TTL))
    };
    for expires in portals.iter().filter_map(|portal| portal.expires_at) {
        at = at.min(if expires > scanned_at {
            expires
        } else {
            decaying
        });
    }
    now + Duration::from_millis(at.saturating_sub(now_ms))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use overseer_net::rt::Instant;

    use super::{PORTAL_DECAYING_TTL, PORTAL_QUIET_TTL, PORTAL_TTL, stale_at};
    use crate::map::{Portal, PortalDestination, RoomCoord};

    const SCANNED: u64 = 1_800_000_000_000;

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    fn portal(expires_at: Option<u64>, decay_time: Option<u64>) -> Portal {
        let room = room("W5N5");
        Portal {
            room,
            x: 1,
            y: 2,
            destination: Some(PortalDestination {
                room,
                shard: None,
                x: Some(3),
                y: Some(4),
            }),
            expires_at,
            decay_time,
        }
    }

    #[test]
    fn portals_go_stale_after_a_day_when_unstable_or_within_the_hour_when_decaying() {
        let now = Instant::now();
        let hour = 3_600_000;
        let centre = room("W5N5");
        let stale = |portals: &[Portal]| stale_at(centre, portals, SCANNED, now, SCANNED) - now;
        assert_eq!(stale(&[]), PORTAL_TTL);
        assert_eq!(stale(&[portal(None, None)]), PORTAL_TTL);
        assert_eq!(
            stale(&[Portal::pending(centre, 1, 2)]),
            PORTAL_TTL,
            "pending destinations"
        );
        assert_eq!(
            stale(&[portal(None, None), portal(Some(SCANNED + 5 * hour), None)]),
            Duration::from_hours(5)
        );
        assert_eq!(
            stale(&[portal(Some(SCANNED + 48 * hour), None)]),
            PORTAL_TTL
        );
        assert_eq!(stale(&[portal(None, Some(1_000))]), PORTAL_DECAYING_TTL);
        // Unstable already when scanned: it decays now.
        assert_eq!(
            stale(&[portal(Some(SCANNED - 1), None)]),
            PORTAL_DECAYING_TTL
        );
        // Seeded from a scan two hours ago.
        let seeded = stale_at(centre, &[], SCANNED, now, SCANNED + 2 * hour) - now;
        assert_eq!(seeded, Duration::from_hours(22));
        let expired = stale_at(centre, &[], SCANNED, now, SCANNED + 25 * hour);
        assert_eq!(expired, now);
    }

    #[test]
    fn rooms_where_portals_rarely_stand_stay_without_them_for_a_week() {
        let now = Instant::now();
        let stale = |name: &str, portals: &[Portal]| {
            stale_at(room(name), portals, SCANNED, now, SCANNED) - now
        };
        // Neither a sector centre nor a highway: a week without portals.
        assert_eq!(stale("W3N7", &[]), PORTAL_QUIET_TTL);
        // Sector centres and highways: a day.
        for name in ["W5N5", "W10N3", "E4S0", "W0N0"] {
            assert_eq!(stale(name, &[]), PORTAL_TTL, "{name}");
        }
        // A rare room with portals: a day, like any.
        assert_eq!(stale("W3N7", &[portal(None, None)]), PORTAL_TTL);
    }
}
