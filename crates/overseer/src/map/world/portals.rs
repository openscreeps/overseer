//! Portals of the world map: the candidate rooms scanned through short-lived `roomMap2`
//! subscriptions, then one room-objects request per room with portals for their destinations;
//! viewed and visible rooms first, the rest of the world in the background; live rooms show
//! their portals directly.

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

use super::focus::{Focus, Tier};
use super::stats::epoch_millis;
use super::terrain::reset_at;
use super::{MINIMAP_CAP, Server, WorldEvent};
use crate::map::portal::{portal_rooms, portals_of};
use crate::map::{Map, Portal, RoomCoord, RoomRect, WorldKnowledge, may_have_portals};
use crate::{Closed, Error, Overseer, RoomSnapshot, Watch};

/// How long the portals of a room (or their absence) are used before the room is scanned
/// again, unless one of them becomes unstable earlier ([`Portal::expires_at`]).
pub const PORTAL_TTL: Duration = Duration::from_hours(24);

/// How long the portals of a room with a decaying portal ([`Portal::decay_time`]) are used:
/// its end is counted in game ticks, so the room is looked at again within the hour.
pub const PORTAL_DECAYING_TTL: Duration = Duration::from_hours(1);

/// Shortest pause between two room-objects requests for portal destinations on servers with
/// rate limits: at most 30 per minute, a quarter of screeps.com's server-wide 120 per minute.
pub const PORTAL_GAP: Duration = Duration::from_secs(2);

/// How long a room-objects request for portal destinations may take; then it counts as failed
/// (the room is asked again later, see [`WorldMap`](super::WorldMap)).
pub const PORTAL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Room-objects requests for portal destinations in flight at once on servers that show no
/// rate limit (without pause); one on the others.
pub const PORTAL_IN_FLIGHT: usize = 4;

/// Most scan subscriptions of background rooms at once, so that the rooms coming into view
/// always find free subscriptions.
pub const PORTAL_BACKGROUND_SCANS: usize = 64;

/// How long a scan subscription waits for the room's first `roomMap2` update before it is
/// released; the room is scanned again 30 s later. Servers answer a subscription as soon as
/// they process it: screeps.com within a fraction of a second, `OpenScreeps` through its state
/// queue, behind the tick being processed (0.5 to 0.9 s on an idle disposable world, seconds
/// when busy).
pub const PORTAL_SCAN_TIMEOUT: Duration = Duration::from_secs(30);

/// Wait before a room whose scan timed out is scanned again.
const SCAN_RETRY: Duration = Duration::from_secs(30);

/// Wait after a room's room-objects request failed (for another reason than the rate limit)
/// before that room is asked again; it doubles with each failure in a row, up to
/// [`RETRY_MAX`]. Other rooms are asked meanwhile.
const RETRY: Duration = Duration::from_secs(30);

/// Longest wait before a room whose room-objects requests keep failing is asked again.
const RETRY_MAX: Duration = Duration::from_hours(1);

/// Room-objects requests of one room that failed in a row.
struct Failure {
    attempts: u32,
    /// Not asked again before this.
    until: Instant,
}

/// Wait after HTTP 429 without the endpoint's limit (the server-wide limit counts per minute).
const GLOBAL_RETRY: Duration = Duration::from_mins(1);

/// When a room's portals were learnt and when they go stale (the portals themselves are in
/// the [`WorldKnowledge`]).
struct Known {
    /// Whether the room has portals.
    some: bool,
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
}

/// A room-objects request in flight.
struct Fetch {
    room: RoomCoord,
    /// When it went out, in milliseconds since the Unix epoch.
    sent_at: u64,
    /// `None` when it took longer than [`PORTAL_FETCH_TIMEOUT`].
    request: BoxFuture<'static, Option<Result<Vec<Portal>, Error>>>,
}

/// The portals known, the scans in flight and the rooms waiting for their destinations.
pub(super) struct Portals {
    known: HashMap<RoomCoord, Known>,
    /// Where the portals are kept and read.
    knowledge: WorldKnowledge,
    /// Every room of the world: its candidates are scanned in the background while
    /// [`Self::rect`] is set.
    world: RoomRect,
    /// The rooms whose portals are wanted, inside the world; `None` without a signed-in user
    /// (`roomMap2` needs one).
    pub(super) rect: Option<RoomRect>,
    /// The live minimaps, which take their share of [`MINIMAP_CAP`].
    minimaps: usize,
    scans: Vec<Scan>,
    /// Rooms whose map view shows portals, waiting for their destinations.
    found: HashSet<RoomCoord>,
    /// Rooms whose scan timed out, not scanned again before then.
    retry: HashMap<RoomCoord, Instant>,
    fetches: Vec<Fetch>,
    /// Rooms whose room-objects request failed (for another reason than the rate limit): how
    /// often in a row, and when to ask again.
    failed: HashMap<RoomCoord, Failure>,
    /// How the server limits requests, as of the latest plan.
    server: Server,
    /// No room-objects request before this (pacing, rate limit or failure).
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
            minimaps: 0,
            scans: Vec::new(),
            found: HashSet::new(),
            retry: HashMap::new(),
            fetches: Vec::new(),
            failed: HashMap::new(),
            server: Server::Limited,
            not_before: None,
        }
    }

    /// Records how many minimaps are live: scans use the rest of [`MINIMAP_CAP`].
    pub(super) const fn set_minimaps(&mut self, count: usize) {
        self.minimaps = count;
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
                scanned_at,
                live_at: None,
                stale_at,
            },
        );
        self.knowledge.set_portals(room, portals);
    }

    /// The candidate rooms of the wanted rooms whose portals are known (possibly stale), and
    /// all of them.
    pub(super) fn progress(&self) -> (usize, usize) {
        self.rect.map_or((0, 0), |rect| {
            portal_rooms(rect).fold((0, 0), |(known, wanted), room| {
                (
                    known + usize::from(self.known.contains_key(&room)),
                    wanted + 1,
                )
            })
        })
    }

    /// The live room `snapshot` of `room` arrived: its portals (with their destinations) are
    /// known at once, whether or not it is a candidate, and reported
    /// ([`WorldEvent::Portals`]) when they differ from the known ones. Scans and requests of
    /// the room in flight are dropped.
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
        if same {
            if let Some(known) = self.known.get_mut(&room) {
                known.live_at = Some(millis);
                known.stale_at =
                    known
                        .stale_at
                        .max(stale_at(&portals, millis, Instant::now(), millis));
            }
        } else {
            self.record(room, portals, Some(millis), events);
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
            None if !shows && !may_have_portals(room) && !self.known.contains_key(&room) => {
                return false;
            }
            _ => {}
        }
        self.scans.retain(|scan| scan.room != room);
        self.scanned(room, shows, events);
        true
    }

    /// Releases the scans that timed out, subscribes the candidates of the world neither known
    /// (fresh), scanned nor waiting for their destinations, by tier (viewed, then those of the
    /// wanted rooms or visible, then the background, at most [`PORTAL_BACKGROUND_SCANS`] at
    /// once) and nearest the centre first, as far as [`MINIMAP_CAP`] allows; then requests the
    /// destinations of the rooms with portals in the same order when the pacing and the rate
    /// limit allow (one at a time, [`PORTAL_GAP`] apart, on a `server` with rate limits; up to
    /// [`PORTAL_IN_FLIGHT`] without pause on the others). Returns when to plan again (a scan
    /// times out, a retry or a request is due, or portals go stale).
    pub(super) fn plan(
        &mut self,
        map: &Map,
        overseer: &Overseer,
        shard: Option<&str>,
        (now, focus, server): (Instant, &Focus, Server),
    ) -> Option<Instant> {
        self.server = server;
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
            (tier(room), rank, distance, y, x)
        };
        let mut due = Due::default();
        let mut index = 0;
        while index < self.scans.len() {
            let scan = &self.scans[index];
            if scan.deadline <= now {
                self.retry.insert(scan.room, now + SCAN_RETRY);
                self.scans.swap_remove(index);
            } else {
                due.at(scan.deadline);
                index += 1;
            }
        }
        self.retry.retain(|_, at| *at > now);
        let scanning = self
            .scans
            .iter()
            .map(|scan| scan.room)
            .chain(self.fetches.iter().map(|fetch| fetch.room))
            .collect::<HashSet<_>>();
        let mut wanted = Vec::new();
        for room in portal_rooms(self.world) {
            if let Some(known) = self.known.get(&room)
                && known.stale_at > now
            {
                due.at(known.stale_at);
            } else if let Some(&at) = self.retry.get(&room) {
                due.at(at);
            } else if !scanning.contains(&room) && !self.found.contains(&room) {
                wanted.push(room);
            }
        }
        wanted.sort_unstable_by_key(|&room| key(room));
        let mut free = MINIMAP_CAP.saturating_sub(self.minimaps + self.scans.len());
        let mut background = self.scans.iter().filter(|scan| scan.background).count();
        let millis = epoch_millis();
        for room in wanted {
            let is_background = tier(room) == Tier::Background;
            if free == 0 || (is_background && background >= PORTAL_BACKGROUND_SCANS) {
                break;
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
            });
            due.at(now + PORTAL_SCAN_TIMEOUT);
            free -= 1;
            background += usize::from(is_background);
        }
        let in_flight = match server {
            Server::Unlimited => PORTAL_IN_FLIGHT,
            Server::Limited => 2,
        };
        while self.fetches.len() < in_flight {
            let mut waiting = None;
            let Some(room) = self
                .found
                .iter()
                .copied()
                .filter(|&room| !self.fetches.iter().any(|fetch| fetch.room == room))
                .filter(|room| match self.failed.get(room) {
                    Some(failure) if failure.until > now => {
                        waiting = Some(
                            waiting.map_or(failure.until, |at: Instant| at.min(failure.until)),
                        );
                        false
                    }
                    _ => true,
                })
                .min_by_key(|&room| key(room))
            else {
                if let Some(at) = waiting {
                    due.at(at);
                }
                break;
            };
            let ready = self.ready_at(overseer.client(), now);
            if ready > now {
                due.at(ready);
                break;
            }
            // On servers with rate limits, starts are spaced (whatever the requests' latency).
            self.not_before = (server == Server::Limited).then(|| now + PORTAL_GAP);
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
                        self.scanned(scan.room, shows_portals(&view), events);
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
                    Some(reset_at(limit.reset, now).max(now + PORTAL_GAP))
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

    /// A room's map view arrived: its destinations are wanted when it `shows` portals, else
    /// it has none.
    fn scanned(&mut self, room: RoomCoord, shows: bool, events: &mut VecDeque<WorldEvent>) {
        if shows {
            self.found.insert(room);
        } else {
            self.record(room, Vec::new(), None, events);
        }
    }

    /// Stores the portals of `room` scanned now (shown by a live room at `live_at`) and
    /// queues them.
    fn record(
        &mut self,
        room: RoomCoord,
        portals: Vec<Portal>,
        live_at: Option<u64>,
        events: &mut VecDeque<WorldEvent>,
    ) {
        let scanned_at = epoch_millis();
        let portals = Arc::<[Portal]>::from(portals);
        let stale_at = stale_at(&portals, scanned_at, Instant::now(), scanned_at);
        self.known.insert(
            room,
            Known {
                some: !portals.is_empty(),
                scanned_at,
                live_at,
                stale_at,
            },
        );
        self.knowledge.set_portals(room, Arc::clone(&portals));
        events.push_back(WorldEvent::Portals(room, portals));
    }

    /// When a room-objects request may go out: after the pacing and, when the endpoint's
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
