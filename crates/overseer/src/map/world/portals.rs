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
use overseer_net::core::api::{ApiError, endpoints};
use overseer_net::core::socket::types::RoomMap;
use overseer_net::core::socket::{Channel, Payload};
use overseer_net::rt::{self, BoxFuture, Instant};
use overseer_net::{Client, Error as NetError};

use super::focus::{Budget, Focus, Server, Tier};
use super::stats::epoch_millis;
use super::terrain::{Terrains, reset_at};
use super::{MINIMAP_CAP, WorldEvent};
use crate::map::portal::portals_of;
use crate::map::{
    Map, Portal, PortalDestination, RoomCoord, RoomRect, WorldKnowledge, may_have_portals,
};
use crate::{Closed, Error, Overseer, RoomSnapshot, Watch};

/// How long the portals of a room (or their absence) are used before the room is scanned
/// again, unless one of them becomes unstable earlier ([`Portal::expires_at`]).
pub const PORTAL_TTL: Duration = Duration::from_hours(24);

/// How long the portals of a room with a decaying portal ([`Portal::decay_time`]) are used:
/// its end is counted in game ticks, so the room is looked at again within the hour.
pub const PORTAL_DECAYING_TTL: Duration = Duration::from_hours(1);

/// How long a room-objects request for portal destinations may take; then it counts as failed
/// (the room is asked again later, see [`WorldMap`](super::WorldMap)).
pub const PORTAL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Room-objects requests for portal destinations in flight at once, on every server: enough
/// to reach the request budget at screeps.com's latencies (0.3 to 2 s each).
pub const PORTAL_IN_FLIGHT: usize = 6;

/// Most scan subscriptions of background rooms at once, so that the rooms coming into view
/// always find free subscriptions.
pub const PORTAL_BACKGROUND_SCANS: usize = 64;

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
    background: bool,
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
    /// cleared by every change of what is known or wanted.
    progress: Cell<Option<(usize, (usize, usize))>>,
    /// The live minimaps, which take their share of [`MINIMAP_CAP`].
    minimaps: usize,
    scans: Vec<Scan>,
    /// Rooms whose map view shows portals, waiting for their destinations.
    found: HashSet<RoomCoord>,
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
            queue: VecDeque::new(),
            reorder: true,
            next_check: None,
            minimaps: 0,
            scans: Vec::new(),
            found: HashSet::new(),
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

    /// Whether the known portals of `room` were inferred from the rooms they lead to.
    pub(super) fn is_inferred(&self, room: RoomCoord) -> bool {
        self.known.get(&room).is_some_and(|known| known.inferred)
    }

    /// Seeds the portals of `room` scanned at `scanned_at` (milliseconds since the Unix epoch)
    /// from the app's cache. Ignored when a newer scan is known.
    pub(super) fn insert(&mut self, room: RoomCoord, portals: Arc<[Portal]>, scanned_at: u64) {
        if self
            .known
            .get(&room)
            .is_some_and(|known| known.scanned_at >= scanned_at)
        {
            return;
        }
        let stale_at = stale_at(&portals, scanned_at, Instant::now(), epoch_millis());
        self.known.insert(
            room,
            Known {
                some: !portals.is_empty(),
                inferred: false,
                scanned_at,
                live_at: None,
                stale_at,
            },
        );
        self.knowledge.set_portals(room, portals);
        self.check_at(stale_at);
        self.progress.set(None);
    }

    /// The wanted rooms (but those `terrain` knows the server has no terrain for) whose
    /// portals are known (possibly stale), and all of them.
    pub(super) fn progress(&self, terrain: &Terrains) -> (usize, usize) {
        let Some(rect) = self.rect else {
            return (0, 0);
        };
        let absent = terrain.absent_count();
        if let Some((seen, progress)) = self.progress.get()
            && seen == absent
        {
            return progress;
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
                        .max(stale_at(&portals, millis, Instant::now(), millis));
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
    /// (viewed, then those of the wanted rooms or visible, then the background, at most
    /// [`PORTAL_BACKGROUND_SCANS`] at once), the rooms where portals usually stand
    /// ([`may_have_portals`]) first but for viewed ones, nearest the centre first, as far as
    /// [`MINIMAP_CAP`] allows; then requests the destinations of the rooms with portals (by
    /// tier, then those with the most portal tiles their paired rooms do not tell, nearest
    /// first), up to [`PORTAL_IN_FLIGHT`] at once, as far as the request `budget` and the
    /// endpoint's rate limit allow. Returns when to plan again (a scan times out, a retry or
    /// a request is due, or portals go stale).
    pub(super) fn plan(
        &mut self,
        map: &Map,
        overseer: &Overseer,
        shard: Option<&str>,
        (now, focus, _): (Instant, &Focus, Server),
        (budget, terrain): (&mut Budget<'_>, &Terrains),
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
        let mut background = self.scans.iter().filter(|scan| scan.background).count();
        let millis = epoch_millis();
        while free > 0
            && let Some(&room) = self.queue.front()
        {
            let is_background = tier(room) == Tier::Background;
            if is_background && background >= PORTAL_BACKGROUND_SCANS {
                break;
            }
            self.queue.pop_front();
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
                background: is_background,
                started_at: millis,
                watch,
                deadline: now + PORTAL_SCAN_TIMEOUT,
                settled: now + PORTAL_SCAN_SETTLE,
            });
            due.at(now + PORTAL_SCAN_TIMEOUT);
            free -= 1;
            background += usize::from(is_background);
        }
        // Rooms still being scanned where portals usually stand may be the partners of rooms
        // found: their fetches wait for those scans to settle, so that a fetch can tell its
        // partner instead of racing it.
        let settling = self
            .scans
            .iter()
            .filter(|scan| scan.settled > now && may_have_portals(scan.room))
            .map(|scan| (tier(scan.room), scan.settled))
            .collect::<Vec<_>>();
        let mut waiting = Due::default();
        let mut queue = self
            .found
            .iter()
            .copied()
            .filter(|&room| !self.fetches.iter().any(|fetch| fetch.room == room))
            .filter(|&room| {
                if let Some(failure) = self.failed.get(&room)
                    && failure.until > now
                {
                    waiting.at(failure.until);
                    return false;
                }
                let tier = tier(room);
                let mut held = false;
                for &(scanning, settled) in &settling {
                    if tier != Tier::Viewed && scanning <= tier {
                        waiting.at(settled);
                        held = true;
                    }
                }
                !held
            })
            .collect::<Vec<_>>();
        queue.sort_unstable_by_key(|&room| {
            let (tier, rank, _, distance, y, x) = key(room);
            (tier, rank, Reverse(self.untold(room)), distance, y, x)
        });
        let mut queue = queue.into_iter();
        while self.fetches.len() < PORTAL_IN_FLIGHT {
            let Some(room) = queue.next() else {
                if let Some(at) = waiting.0 {
                    due.at(at);
                }
                break;
            };
            // A room whose portal tiles have the shape of those of a room being fetched may
            // be its partner: it waits for that fetch, which may tell its portals (the
            // completion plans again).
            let shape = self.shape(room);
            if tier(room) != Tier::Viewed
                && self
                    .fetches
                    .iter()
                    .any(|fetch| self.shape(fetch.room) == shape)
            {
                continue;
            }
            let ready = self.ready_at(overseer.client(), now);
            if ready > now {
                due.at(ready);
                break;
            }
            if let Err(at) = budget.take(tier(room)) {
                due.at(at);
                break;
            }
            self.not_before = None;
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
        due.0
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

    /// A room's map view arrived: its destinations are wanted when it shows portals (unless
    /// its paired portals tell them), else it has none.
    fn scanned(&mut self, room: RoomCoord, view: &RoomMap, events: &mut VecDeque<WorldEvent>) {
        let tiles = view
            .p
            .iter()
            .flatten()
            .filter_map(|&[x, y]| Some((u8::try_from(x).ok()?, u8::try_from(y).ok()?)))
            .collect::<Vec<_>>();
        if tiles.is_empty() {
            self.tiles.remove(&room);
            self.record(room, Vec::new(), None, events);
        } else {
            self.tiles.insert(room, tiles);
            self.found.insert(room);
            self.infer(room, events);
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
                    destination: PortalDestination {
                        room: source.room,
                        shard: None,
                        x: Some(source.x),
                        y: Some(source.y),
                    },
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
        let stale_at = stale_at(&portals, scanned_at, Instant::now(), scanned_at);
        self.known.insert(
            room,
            Known {
                // A room whose map view shows portals though none came (a portal its objects
                // do not list) counts as having some, so its live minimap does not ask again.
                some: !portals.is_empty() || self.tiles.contains_key(&room),
                inferred,
                scanned_at,
                live_at,
                stale_at,
            },
        );
        self.check_at(stale_at);
        self.progress.set(None);
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
        if let Some(limit) = client.rate_limit::<endpoints::GameRoomObjects>()
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
/// which name only the room.
fn lands(portal: &Portal) -> Option<(RoomCoord, Tile)> {
    let destination = &portal.destination;
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

/// When portals scanned at `scanned_at` go stale, as an [`Instant`] (`now` is `now_ms`
/// milliseconds since the Unix epoch): after [`PORTAL_TTL`] ([`PORTAL_DECAYING_TTL`] with a
/// decaying portal), or when the first becomes unstable ([`PORTAL_DECAYING_TTL`] after the scan
/// when it already was).
fn stale_at(portals: &[Portal], scanned_at: u64, now: Instant, now_ms: u64) -> Instant {
    let millis = |duration: Duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    let decaying = scanned_at.saturating_add(millis(PORTAL_DECAYING_TTL));
    let mut at = if portals.iter().any(|portal| portal.decay_time.is_some()) {
        decaying
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

    use super::{PORTAL_DECAYING_TTL, PORTAL_TTL, stale_at};
    use crate::map::{Portal, PortalDestination, RoomCoord};

    const SCANNED: u64 = 1_800_000_000_000;

    fn portal(expires_at: Option<u64>, decay_time: Option<u64>) -> Portal {
        let room: RoomCoord = "W5N5".parse().unwrap();
        Portal {
            room,
            x: 1,
            y: 2,
            destination: PortalDestination {
                room,
                shard: None,
                x: Some(3),
                y: Some(4),
            },
            expires_at,
            decay_time,
        }
    }

    #[test]
    fn portals_go_stale_after_a_day_when_unstable_or_within_the_hour_when_decaying() {
        let now = Instant::now();
        let hour = 3_600_000;
        let stale = |portals: &[Portal]| stale_at(portals, SCANNED, now, SCANNED) - now;
        assert_eq!(stale(&[]), PORTAL_TTL);
        assert_eq!(stale(&[portal(None, None)]), PORTAL_TTL);
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
        let seeded = stale_at(&[], SCANNED, now, SCANNED + 2 * hour) - now;
        assert_eq!(seeded, Duration::from_hours(22));
        let expired = stale_at(&[], SCANNED, now, SCANNED + 25 * hour);
        assert_eq!(expired, now);
    }
}
