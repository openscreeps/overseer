//! Portals of the world map: the candidate rooms scanned through short-lived `roomMap2`
//! subscriptions, then one room-objects request per room with portals for their destinations.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::StreamExt as _;
use overseer_net::core::api::{ApiError, endpoints};
use overseer_net::core::socket::types::RoomMap;
use overseer_net::core::socket::{Channel, Payload};
use overseer_net::rt::{BoxFuture, Instant};
use overseer_net::{Client, Error as NetError};

use super::stats::epoch_millis;
use super::terrain::reset_at;
use super::{MINIMAP_CAP, WorldEvent};
use crate::map::portal::portal_rooms;
use crate::map::{Map, Portal, RoomCoord, RoomRect, may_have_portals};
use crate::{Closed, Error, Overseer, Watch};

/// How long the portals of a room (or their absence) are used before the room is scanned
/// again, unless one of them becomes unstable earlier ([`Portal::expires_at`]).
pub const PORTAL_TTL: Duration = Duration::from_hours(24);

/// How long the portals of a room with a decaying portal ([`Portal::decay_time`]) are used:
/// its end is counted in game ticks, so the room is looked at again within the hour.
pub const PORTAL_DECAYING_TTL: Duration = Duration::from_hours(1);

/// Shortest pause between two room-objects requests for portal destinations: at most 30 per
/// minute, a quarter of screeps.com's server-wide 120 per minute.
pub const PORTAL_GAP: Duration = Duration::from_secs(2);

/// How long a scan subscription waits for the room's first `roomMap2` update before it is
/// released; the room is scanned again 30 s later. Servers answer a subscription as soon as
/// they process it: screeps.com within a fraction of a second, `OpenScreeps` through its state
/// queue, behind the tick being processed (0.5 to 0.9 s on an idle disposable world, seconds
/// when busy).
pub const PORTAL_SCAN_TIMEOUT: Duration = Duration::from_secs(30);

/// Wait before a room whose scan timed out is scanned again.
const SCAN_RETRY: Duration = Duration::from_secs(30);

/// Wait after a failed room-objects request before the next one.
const RETRY: Duration = Duration::from_secs(30);

/// Wait after HTTP 429 without the endpoint's limit (the server-wide limit counts per minute).
const GLOBAL_RETRY: Duration = Duration::from_mins(1);

/// The portals of a room and when they go stale.
struct Known {
    portals: Arc<[Portal]>,
    /// When the room was scanned, in milliseconds since the Unix epoch.
    scanned_at: u64,
    /// Scanned again from then on.
    stale_at: Instant,
}

/// A scan subscription waiting for the room's first update.
struct Scan {
    room: RoomCoord,
    watch: Watch<RoomMap>,
    deadline: Instant,
}

/// The room-objects request in flight.
struct Fetch {
    room: RoomCoord,
    request: BoxFuture<'static, Result<Vec<Portal>, Error>>,
}

/// The portals known, the scans in flight and the rooms waiting for their destinations.
#[derive(Default)]
pub(super) struct Portals {
    known: HashMap<RoomCoord, Known>,
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
    fetch: Option<Fetch>,
    /// No room-objects request before this (pacing, rate limit or failure).
    not_before: Option<Instant>,
}

impl Portals {
    /// The portals of `room`, when scanned or seeded.
    pub(super) fn get(&self, room: RoomCoord) -> Option<Arc<[Portal]>> {
        self.known
            .get(&room)
            .map(|known| Arc::clone(&known.portals))
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
                portals,
                scanned_at,
                stale_at,
            },
        );
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

    /// A live minimap of `room` arrived: it counts as a scan of a wanted room not known yet
    /// (or whose portals appeared or vanished meanwhile), whether or not it is a candidate.
    /// Returns whether anything changed.
    pub(super) fn minimap(
        &mut self,
        room: RoomCoord,
        view: &RoomMap,
        events: &mut VecDeque<WorldEvent>,
    ) -> bool {
        if !self.rect.is_some_and(|rect| rect.contains(room))
            || self.found.contains(&room)
            || self.fetch.as_ref().is_some_and(|fetch| fetch.room == room)
        {
            return false;
        }
        let shows = shows_portals(view);
        let fresh = self
            .known
            .get(&room)
            .filter(|known| known.stale_at > Instant::now());
        match fresh {
            Some(known) if known.portals.is_empty() != shows => return false,
            None if !shows && !may_have_portals(room) && !self.known.contains_key(&room) => {
                return false;
            }
            _ => {}
        }
        self.scans.retain(|scan| scan.room != room);
        self.scanned(room, shows, events);
        true
    }

    /// Releases the scans that timed out, subscribes the wanted candidates neither known
    /// (fresh), scanned nor waiting for their destinations, nearest the centre of the wanted
    /// rooms first, as far as [`MINIMAP_CAP`] allows; then requests the destinations of the
    /// nearest room with portals when the pacing and the rate limit allow. Returns when to plan
    /// again (a scan times out, a retry or a request is due, or portals go stale).
    pub(super) fn plan(
        &mut self,
        map: &Map,
        overseer: &Overseer,
        shard: Option<&str>,
        now: Instant,
    ) -> Option<Instant> {
        let Some(rect) = self.rect else {
            self.scans.clear();
            return None;
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
            .chain(self.fetch.as_ref().map(|fetch| fetch.room))
            .collect::<HashSet<_>>();
        let mut wanted = Vec::new();
        for room in portal_rooms(rect) {
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
        let free = MINIMAP_CAP.saturating_sub(self.minimaps + self.scans.len());
        let center = rect.center();
        wanted.sort_by_key(|&room| distance(room, center));
        for room in wanted.into_iter().take(free) {
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
                watch,
                deadline: now + PORTAL_SCAN_TIMEOUT,
            });
            due.at(now + PORTAL_SCAN_TIMEOUT);
        }
        if self.fetch.is_none()
            && let Some(room) = self
                .found
                .iter()
                .copied()
                .filter(|&room| rect.contains(room))
                .min_by_key(|&room| distance(room, center))
        {
            let ready = self.ready_at(overseer.client(), now);
            if ready > now {
                due.at(ready);
            } else {
                self.not_before = None;
                let (map, shard, name) = (map.clone(), shard.map(str::to_owned), room.to_string());
                let request = Box::pin(async move { map.portals(&name, shard.as_deref()).await });
                self.fetch = Some(Fetch { room, request });
            }
        }
        due.0
    }

    /// Completes the scans that were answered (a room whose map view shows no portal is known
    /// to have none: [`WorldEvent::Portals`] with no portal) and the room-objects request
    /// ([`WorldEvent::Portals`] with its portals). Returns whether anything changed.
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
                    let room = self.scans.swap_remove(index).room;
                    self.scanned(room, shows_portals(&view), events);
                    changed = true;
                }
            }
        }
        let Some(fetch) = &mut self.fetch else {
            return Ok(changed);
        };
        let Poll::Ready(result) = fetch.request.as_mut().poll(cx) else {
            return Ok(changed);
        };
        let room = fetch.room;
        self.fetch = None;
        let now = Instant::now();
        let wait = match result {
            Ok(portals) => {
                self.found.remove(&room);
                self.record(room, portals, events);
                now + PORTAL_GAP
            }
            Err(Error::Closed) => return Err(Closed),
            Err(Error::Net(NetError::Api(ApiError::RateLimited(Some(limit))))) => {
                reset_at(limit.reset, now).max(now + PORTAL_GAP)
            }
            Err(Error::Net(NetError::Api(ApiError::RateLimited(None)))) => now + GLOBAL_RETRY,
            Err(_) => now + RETRY,
        };
        self.not_before = Some(wait);
        Ok(true)
    }

    /// A room's map view arrived: its destinations are wanted when it `shows` portals, else
    /// it has none.
    fn scanned(&mut self, room: RoomCoord, shows: bool, events: &mut VecDeque<WorldEvent>) {
        if shows {
            self.found.insert(room);
        } else {
            self.record(room, Vec::new(), events);
        }
    }

    /// Stores the portals of `room` scanned now and queues them.
    fn record(&mut self, room: RoomCoord, portals: Vec<Portal>, events: &mut VecDeque<WorldEvent>) {
        let scanned_at = epoch_millis();
        let portals = Arc::<[Portal]>::from(portals);
        let stale_at = stale_at(&portals, scanned_at, Instant::now(), scanned_at);
        self.known.insert(
            room,
            Known {
                portals: Arc::clone(&portals),
                scanned_at,
                stale_at,
            },
        );
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

/// The squared distance between two rooms, ties broken north-west first.
fn distance(room: RoomCoord, center: RoomCoord) -> (i64, i32, i32) {
    let (dx, dy) = (
        i64::from(room.x) - i64::from(center.x),
        i64::from(room.y) - i64::from(center.y),
    );
    (dx * dx + dy * dy, room.y, room.x)
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
