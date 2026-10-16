//! Room overviews of the world map: batched `owner0` map statistics with a long lifetime,
//! viewed rooms first, paced by the server's rate limit, and overviews derived from live rooms.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use overseer_net::core::api::types::GameMapStatsResponse;
use overseer_net::core::api::{ApiError, RateLimit, endpoints};
use overseer_net::rt::{BoxFuture, Instant, SystemTime, UNIX_EPOCH};
use overseer_net::{Client, Error as NetError};

use super::WorldEvent;
use super::focus::{Budget, Focus, Server, Tier};
use crate::map::{Map, MapStat, RoomCoord, RoomRect, RoomStat, WorldKnowledge};
use crate::{Closed, Error, RoomSnapshot};

/// How long the overview of a near room (see [`WorldMap`](super::WorldMap)) is used before it
/// is requested again.
pub const STATS_TTL: Duration = Duration::from_mins(10);

/// How long the overview of any other room of the world is used before it is requested again.
pub const STATS_FAR_TTL: Duration = Duration::from_mins(60);

/// Percentage of the endpoint's rate-limit window that requests for far rooms may use: the
/// rest stays for near rooms, so they always refresh.
pub const STATS_FAR_SHARE: u32 = 70;

/// Most sectors (10 × 10 rooms, at most 5 000 rooms) per map-stats request. The official
/// world map asks for every room of the visible sectors at once; a whole shard (33 124 rooms
/// on screeps.com `shard0`) is too much for one request, so it is split into requests of the
/// nearest whole sectors and fills progressively.
///
/// Measured on screeps.com `shard0` (October 2026): 100 rooms answer in 0.18 s, 2 500 in
/// 1.0 s (330 kB), 5 000 in 1.4 to 1.5 s (0.5 to 0.8 MB) with two in flight at once (the
/// server answers them in parallel). The hourly budget (60 requests per token), not time,
/// binds: 5 000 rooms per request load a whole shard in 7 requests instead of 14, in as many
/// rounds of [`STATS_IN_FLIGHT`] requests. A cold load of `shard0` then took 8 requests and
/// 6.9 s from the first request to the last overview (it took 14 to 16 requests and 33 to
/// 41 s one request of 2 500 rooms at a time).
pub const STATS_BATCH_SECTORS: usize = 50;

/// Map-stats requests in flight at once. On servers with rate limits (not exempt), one until
/// the endpoint's first response tells how many requests its window has left, and one while
/// the budget spreads requests over the rest of the window (see
/// [`WorldMap`](super::WorldMap)).
pub const STATS_IN_FLIGHT: usize = 3;

/// A room that becomes viewed is requested again unless its overview was requested less than
/// this before.
pub const STATS_VIEWED_FRESH: Duration = Duration::from_mins(1);

/// Shortest wait after HTTP 429 with the endpoint's limit.
const MIN_LIMIT_WAIT: Duration = Duration::from_secs(1);

/// Wait after a request failed for another reason than the rate limit.
const RETRY: Duration = Duration::from_secs(30);

/// Wait after HTTP 429 without the endpoint's limit (the server-wide limit counts per minute).
const GLOBAL_RETRY: Duration = Duration::from_secs(60);

/// When a room's overview was learnt and when it goes stale (the overview itself is in the
/// [`WorldKnowledge`]).
struct Known {
    /// The overview's [`RoomStat::fetched_at`].
    fetched_at: u64,
    /// When the latest request for it went out (a seed's [`RoomStat::fetched_at`]), in
    /// milliseconds since the Unix epoch.
    requested_at: u64,
    /// When a live room last showed it, in milliseconds since the Unix epoch: overviews
    /// requested before keep what the live room showed.
    live_at: Option<u64>,
    /// Stale from then on while the room is near.
    near_until: Instant,
    /// Stale from then on while the room is far.
    far_until: Instant,
}

impl Known {
    /// An overview fetched at `fetched_at`, requested at `requested_at`, `age` old at `now`.
    fn new(fetched_at: u64, requested_at: u64, now: Instant, age: Duration) -> Self {
        Self {
            fetched_at,
            requested_at,
            live_at: None,
            near_until: now + STATS_TTL.saturating_sub(age),
            far_until: now + STATS_FAR_TTL.saturating_sub(age),
        }
    }
}

/// A request in flight: its rooms, nearest first, when it went out (milliseconds since the
/// Unix epoch), and the request.
struct Fetch {
    rooms: Vec<RoomCoord>,
    sent_at: u64,
    request: BoxFuture<'static, Result<GameMapStatsResponse, Error>>,
}

/// The overviews known, wanted and being fetched, and the request budget.
pub(super) struct Stats {
    known: HashMap<RoomCoord, Known>,
    /// Where the overviews are kept and read.
    knowledge: WorldKnowledge,
    /// Every room of the world: all are wanted while [`Self::rect`] is set.
    world: RoomRect,
    /// The rooms in view, inside the world. `None`: no overview is wanted.
    pub(super) rect: Option<RoomRect>,
    /// The near sectors: those of the rooms wanted as minimaps and the sectors around them.
    near: HashSet<(i32, i32)>,
    /// The first minimap room: near rooms nearest it are asked for first.
    near_center: Option<RoomCoord>,
    /// The viewed rooms and since when they are viewed (milliseconds since the Unix epoch).
    viewed: HashMap<RoomCoord, u64>,
    /// Rooms of the world with an overview.
    known_in_world: usize,
    fetches: Vec<Fetch>,
    /// The rooms of the requests in flight.
    pending: HashSet<RoomCoord>,
    /// How the server limits requests, as of the latest plan.
    server: Server,
    /// No request before this (the gap after a request, a refusal's reset or a failure),
    /// exempt or not.
    not_before: Option<Instant>,
    /// After the latest success, when the budget allows the next request (spread over the
    /// rest of the window, or its reset once none is left); ignored while exempt.
    paced: Option<Instant>,
    /// The latest rate limit seen and its reset as an [`Instant`], converted once from the
    /// server's clock.
    seen: Option<(RateLimit, Instant)>,
    /// While the rate limit holds back wanted overviews: when it resets, in milliseconds since
    /// the Unix epoch.
    paused: Option<u64>,
    /// Until when the token is exempt from the rate limit: as given (milliseconds since the
    /// Unix epoch) and converted once to an [`Instant`]; the budget does not apply until then.
    exempt: Option<(u64, Instant)>,
}

impl Stats {
    /// No overview of `world` known or wanted yet; overviews go to `knowledge`.
    pub(super) fn new(world: RoomRect, knowledge: WorldKnowledge) -> Self {
        Self {
            known: HashMap::new(),
            knowledge,
            world,
            rect: None,
            near: HashSet::new(),
            near_center: None,
            viewed: HashMap::new(),
            known_in_world: 0,
            fetches: Vec::new(),
            pending: HashSet::new(),
            server: Server::Limited,
            not_before: None,
            paced: None,
            seen: None,
            paused: None,
            exempt: None,
        }
    }

    /// Makes the sectors of `minimaps` (nearest first) and the sectors around them the near
    /// rooms.
    pub(super) fn set_near(&mut self, minimaps: &[RoomCoord]) {
        self.near.clear();
        for room in minimaps {
            let (x, y) = sector(*room);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    self.near.insert((x + dx, y + dy));
                }
            }
        }
        self.near_center = minimaps.first().copied();
    }

    /// Makes `rooms` the viewed rooms: those that just became viewed are requested again
    /// first, unless requested less than [`STATS_VIEWED_FRESH`] before.
    pub(super) fn set_viewed(&mut self, rooms: &[RoomCoord]) {
        self.viewed.retain(|room, _| rooms.contains(room));
        let now = epoch_millis();
        for &room in rooms {
            self.viewed.entry(room).or_insert(now);
        }
    }

    /// Seeds the overview of `room` from the app's cache; its [`RoomStat::fetched_at`] tells
    /// how stale it is. Ignored when a newer one is known.
    pub(super) fn insert(&mut self, room: RoomCoord, stat: Arc<RoomStat>) {
        if self
            .known
            .get(&room)
            .is_some_and(|known| known.fetched_at >= stat.fetched_at)
        {
            return;
        }
        let age = Duration::from_millis(epoch_millis().saturating_sub(stat.fetched_at));
        let known = Known::new(stat.fetched_at, stat.fetched_at, Instant::now(), age);
        self.knowledge.set_stats(&[(room, stat)]);
        if self.known.insert(room, known).is_none() && self.world.contains(room) {
            self.known_in_world += 1;
        }
    }

    /// The live room `snapshot` of `room` arrived: its overview (see [`RoomStat::live`]) is
    /// fresh, and replaces the known one when it tells something else (queueing a
    /// [`WorldEvent::Stats`] of the room). Overviews requested before it keep what it showed.
    pub(super) fn live(
        &mut self,
        room: RoomCoord,
        snapshot: &RoomSnapshot,
        events: &mut VecDeque<WorldEvent>,
    ) {
        let (now, millis) = (Instant::now(), epoch_millis());
        let previous = self.knowledge.stat(room);
        let stat = RoomStat::live(snapshot, previous.as_deref(), millis);
        let changed = previous.is_none_or(|previous| !previous.same(&stat));
        if !self.known.contains_key(&room) && self.world.contains(room) {
            self.known_in_world += 1;
        }
        let known = self
            .known
            .entry(room)
            .or_insert_with(|| Known::new(millis, 0, now, Duration::ZERO));
        known.live_at = Some(millis);
        known.near_until = now + STATS_TTL;
        known.far_until = now + STATS_FAR_TTL;
        if changed {
            known.fetched_at = millis;
            let fetched = vec![(room, Arc::new(stat))];
            self.knowledge.set_stats(&fetched);
            events.push_back(WorldEvent::Stats(fetched));
        }
    }

    /// The rooms of the world with an overview, and those wanted (every room of the world
    /// while overviews are wanted).
    pub(super) fn progress(&self) -> (usize, usize) {
        let wanted = if self.rect.is_some() {
            self.world.len()
        } else {
            0
        };
        (self.known_in_world, wanted)
    }

    /// While the rate limit holds back wanted overviews, when it resets (milliseconds since
    /// the Unix epoch).
    pub(super) const fn paused(&self) -> Option<u64> {
        self.paused
    }

    /// After the user lifted the rate limit (screeps.com's reset page): while paused, forgets
    /// the endpoint's limit and the wait for its reset, so the next request goes out and its
    /// response's headers record the real limit. Returns whether it was paused; otherwise
    /// nothing changes.
    pub(super) fn recheck(&mut self, client: &Client) -> bool {
        if self.paused.is_none() {
            return false;
        }
        client.forget_rate_limit::<endpoints::GameMapStats>();
        self.not_before = None;
        self.paced = None;
        self.seen = None;
        true
    }

    /// Makes the token exempt from the rate limit until `until` (milliseconds since the Unix
    /// epoch; `None` or a past time: not exempt). Returns whether anything changed.
    pub(super) fn set_exempt_until(&mut self, until: Option<u64>, now: Instant) -> bool {
        let millis = epoch_millis();
        let exempt = until
            .filter(|until| *until > millis)
            .map(|until| (until, now + Duration::from_millis(until - millis)));
        if exempt.map(|(until, _)| until) == self.exempt.map(|(until, _)| until) {
            return false;
        }
        self.exempt = exempt;
        true
    }

    /// When the exemption from the rate limit ends, while it lasts at `now`.
    pub(super) fn exempt_end(&self, now: Instant) -> Option<Instant> {
        self.exempt.map(|(_, end)| end).filter(|end| *end > now)
    }

    /// How many requests may be in flight at `now`: [`STATS_IN_FLIGHT`], but on a server with
    /// rate limits (not exempt) one while the endpoint's limit is not known (its first
    /// response tells how many requests are left before more go out) or while the budget
    /// spreads requests over the rest of the window ([`Self::pace`]).
    fn in_flight(&self, client: &Client, now: Instant) -> usize {
        if self.server == Server::Unlimited || self.exempt_end(now).is_some() {
            return STATS_IN_FLIGHT;
        }
        match self.window(client) {
            Some((limit, left)) if !spreads(limit, left) => STATS_IN_FLIGHT,
            _ => 1,
        }
    }

    /// The endpoint's latest limit and the requests left in its window, those in flight
    /// counted as spent (a response's count may already include requests whose responses
    /// are still on their way: it errs on the side of the budget).
    fn window(&self, client: &Client) -> Option<(RateLimit, u32)> {
        let limit = client.rate_limit::<endpoints::GameMapStats>()?;
        let in_flight = u32::try_from(self.fetches.len()).unwrap_or(u32::MAX);
        Some((limit, limit.remaining.saturating_sub(in_flight)))
    }

    /// Requests the overviews of the viewed rooms due a refresh and the stale near rooms,
    /// else of the stale far rooms (visible sectors first), when the endpoint's budget and the
    /// client's request budget allow (the endpoint's budget not while exempt, until the
    /// exemption ends), up to [`Self::in_flight`] requests at once; returns when to plan again (a budget allows the next request, an overview
    /// goes stale or the exemption ends). Queues a [`WorldEvent::StatsPaused`] when the rate
    /// limit starts or stops holding back wanted overviews (or its reset moves).
    pub(super) fn plan(
        &mut self,
        map: &Map,
        client: &Client,
        shard: Option<&str>,
        (now, focus, server): (Instant, &Focus, Server),
        (budget, events): (&mut Budget<'_>, &mut VecDeque<WorldEvent>),
    ) -> Option<Instant> {
        self.server = server;
        let Some(rect) = self.rect else {
            self.set_paused(None, events);
            return None;
        };
        let mut due = None;
        while self.fetches.len() < self.in_flight(client, now) {
            match self.start(map, client, shard, (now, rect, focus), (budget, events)) {
                Ok(()) => {}
                Err(at) => {
                    due = at;
                    break;
                }
            }
        }
        due
    }

    /// Starts the next request, or returns when to plan again.
    fn start(
        &mut self,
        map: &Map,
        client: &Client,
        shard: Option<&str>,
        (now, rect, focus): (Instant, RoomRect, &Focus),
        (budget, events): (&mut Budget<'_>, &mut VecDeque<WorldEvent>),
    ) -> Result<(), Option<Instant>> {
        let millis = epoch_millis();
        let fresh = u64::try_from(STATS_VIEWED_FRESH.as_millis()).unwrap_or(u64::MAX);
        let mut viewed = self
            .viewed
            .iter()
            .filter(|&(&room, &since)| {
                self.world.contains(room)
                    && !self.pending.contains(&room)
                    && self
                        .known
                        .get(&room)
                        .is_none_or(|known| known.requested_at.saturating_add(fresh) < since)
            })
            .map(|(&room, _)| room)
            .collect::<Vec<_>>();
        viewed.sort_unstable_by_key(|&room| focus.key(room));
        let (mut near, mut far) = (Vec::new(), Vec::new());
        let mut expires: Option<Instant> = None;
        for room in self.world.iter() {
            if self.pending.contains(&room) || viewed.contains(&room) {
                continue;
            }
            let is_near = self.near.contains(&sector(room));
            let until = self.known.get(&room).map(|known| {
                if is_near {
                    known.near_until
                } else {
                    known.far_until
                }
            });
            match until {
                Some(until) if until > now => {
                    expires = Some(expires.map_or(until, |earliest| earliest.min(until)));
                }
                _ if is_near => near.push(room),
                _ => far.push(room),
            }
        }
        let urgent = !viewed.is_empty() || !near.is_empty();
        let budgeted = self.server == Server::Limited;
        let exempt_end = self.exempt_end(now).filter(|_| budgeted);
        let wake = |at: Instant| Err(Some(exempt_end.map_or(at, |end| end.min(at))));
        let paused = if exempt_end.is_some() || !budgeted {
            None
        } else {
            self.held_back(client, now, urgent, !far.is_empty())
        };
        self.set_paused(paused, events);
        let ready = self.ready_at(client, now, exempt_end.is_some() || !budgeted);
        let batch = if urgent {
            if ready > now {
                return wake(ready);
            }
            let tier = if viewed.is_empty() {
                Tier::Visible
            } else {
                Tier::Viewed
            };
            budget.take(tier).map_err(Some)?;
            let sectors = STATS_BATCH_SECTORS - usize::from(!viewed.is_empty());
            let center = self.near_center.unwrap_or_else(|| focus.centre());
            let near = nearest_sectors(near, center, sectors, None);
            let used = near
                .iter()
                .map(|&room| sector(room))
                .collect::<HashSet<_>>()
                .len();
            viewed.extend(near);
            // The rest of the request takes the stale far rooms in view, as far as their share
            // of the budget allows: a view wider than the near rooms fills in the same round.
            let far_allowed =
                exempt_end.is_some() || !budgeted || self.far_ready_at(client, now) <= now;
            if far_allowed && used < sectors {
                far.retain(|&room| rect.contains(room));
                viewed.extend(nearest_sectors(far, focus.centre(), sectors - used, None));
            }
            viewed
        } else {
            if far.is_empty() {
                return Err(expires);
            }
            let ready = if exempt_end.is_some() || !budgeted {
                ready
            } else {
                ready.max(self.far_ready_at(client, now))
            };
            if ready > now {
                return wake(expires.map_or(ready, |earliest| earliest.min(ready)));
            }
            let visible = focus.visible().or(Some(rect));
            let batch = nearest_sectors(far, focus.centre(), STATS_BATCH_SECTORS, visible);
            let tier = batch
                .first()
                .map_or(Tier::Background, |&room| focus.tier(room));
            budget.take(tier).map_err(Some)?;
            batch
        };
        let names = batch.iter().map(ToString::to_string).collect::<Vec<_>>();
        let (map, shard) = (map.clone(), shard.map(str::to_owned));
        let request =
            Box::pin(async move { map.stats(names, MapStat::Owner, shard.as_deref()).await });
        self.pending.extend(batch.iter().copied());
        self.fetches.push(Fetch {
            rooms: batch,
            sent_at: millis,
            request,
        });
        Ok(())
    }

    /// Completes the requests that finished: queues a [`WorldEvent::Stats`] with every
    /// overview each brought and paces the next request. Returns whether anything changed.
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        client: &Client,
        events: &mut VecDeque<WorldEvent>,
    ) -> Result<bool, Closed> {
        let mut changed = false;
        let mut index = 0;
        while index < self.fetches.len() {
            let Poll::Ready(result) = self.fetches[index].request.as_mut().poll(cx) else {
                index += 1;
                continue;
            };
            let Fetch { rooms, sent_at, .. } = self.fetches.swap_remove(index);
            for room in &rooms {
                self.pending.remove(room);
            }
            changed = true;
            let now = Instant::now();
            match result {
                Ok(response) => {
                    self.record(&rooms, &response, sent_at, now, events);
                    if self.server == Server::Limited {
                        self.paced = Some(self.pace(client, now));
                    }
                }
                Err(Error::Closed) => return Err(Closed),
                // The other requests in flight carry on; the latest wait holds the next ones.
                Err(Error::Net(NetError::Api(ApiError::RateLimited(limit)))) => {
                    let wait = match limit {
                        Some(limit) => self.reset_at(limit, now).max(now + MIN_LIMIT_WAIT),
                        None => now + GLOBAL_RETRY,
                    };
                    self.wait_until(wait);
                }
                Err(_) => self.wait_until(now + RETRY),
            }
        }
        Ok(changed)
    }

    /// Stores the overviews of `rooms` requested at `sent_at` (absent ones are rooms the
    /// server does not know) and queues them all: each has a new [`RoomStat::fetched_at`]. A
    /// room a live room showed since keeps what it showed; only its status is taken.
    fn record(
        &mut self,
        rooms: &[RoomCoord],
        response: &GameMapStatsResponse,
        sent_at: u64,
        now: Instant,
        events: &mut VecDeque<WorldEvent>,
    ) {
        let millis = epoch_millis();
        let mut fetched = Vec::with_capacity(rooms.len());
        for &room in rooms {
            let stat = response.stats.get(&room.to_string()).map_or_else(
                || RoomStat::unknown(millis),
                |stat| RoomStat::new(stat, &response.users, millis),
            );
            let live_at = self.known.get(&room).and_then(|known| known.live_at);
            let stat = match live_at
                .filter(|&live_at| live_at >= sent_at)
                .and_then(|_| self.knowledge.stat(room))
            {
                Some(live) => RoomStat {
                    status: stat.status,
                    status_until: stat.status_until,
                    fetched_at: millis,
                    ..(*live).clone()
                },
                None => stat,
            };
            fetched.push((room, Arc::new(stat)));
            let mut known = Known::new(millis, sent_at, now, Duration::ZERO);
            known.live_at = live_at;
            if self.known.insert(room, known).is_none() && self.world.contains(room) {
                self.known_in_world += 1;
            }
        }
        if !fetched.is_empty() {
            self.knowledge.set_stats(&fetched);
            events.push_back(WorldEvent::Stats(fetched));
        }
    }

    /// No request before `at` (nor before any later wait already set).
    fn wait_until(&mut self, at: Instant) {
        self.not_before = Some(self.not_before.map_or(at, |before| before.max(at)));
        self.paced = None;
    }

    /// When the next request may go out after one succeeded, as far as the endpoint's budget
    /// goes (requests in flight counted as spent): at once while at least half of the
    /// window's requests are left, then spread evenly over the rest of the window, and after
    /// the reset once none is left.
    fn pace(&mut self, client: &Client, now: Instant) -> Instant {
        let Some((limit, left)) = self.window(client) else {
            return now;
        };
        let reset = self.reset_at(limit, now);
        if left == 0 {
            reset
        } else if spreads(limit, left) {
            now + reset.saturating_duration_since(now) / left
        } else {
            now
        }
    }

    /// When the rate limit holds back wanted overviews, its reset in milliseconds since the
    /// Unix epoch: no request is left while `urgent` rooms (viewed or near) are wanted, or the
    /// window is down to its near share while `far` rooms are.
    fn held_back(&mut self, client: &Client, now: Instant, urgent: bool, far: bool) -> Option<u64> {
        let (limit, left) = self.window(client)?;
        let blocks = (urgent && left == 0) || (far && left <= near_share(limit.limit));
        (blocks && self.reset_at(limit, now) > now)
            .then(|| limit.reset.saturating_add(1).saturating_mul(1000))
    }

    /// Makes `paused` the paused state, queueing a [`WorldEvent::StatsPaused`] when it
    /// changes.
    fn set_paused(&mut self, paused: Option<u64>, events: &mut VecDeque<WorldEvent>) {
        if self.paused != paused {
            self.paused = paused;
            events.push_back(WorldEvent::StatsPaused(paused));
        }
    }

    /// When a request may go out: after the gap, a refusal's reset or a failure's wait and,
    /// unless `unbudgeted` (exempt, or a server without rate limits), after the pacing and,
    /// when the latest limit the client saw (from any request) has no request left (those in
    /// flight counted as spent), after its reset.
    fn ready_at(&mut self, client: &Client, now: Instant, unbudgeted: bool) -> Instant {
        let mut ready = self.not_before.unwrap_or(now);
        if unbudgeted {
            return ready;
        }
        if let Some(paced) = self.paced {
            ready = ready.max(paced);
        }
        if let Some((limit, 0)) = self.window(client) {
            ready = ready.max(self.reset_at(limit, now));
        }
        ready
    }

    /// When a request for far rooms may go out as far as the budget goes: once the window has
    /// no more than its near share ([`STATS_FAR_SHARE`]) left (those in flight counted as
    /// spent), after its reset.
    fn far_ready_at(&mut self, client: &Client, now: Instant) -> Instant {
        match self.window(client) {
            Some((limit, left)) if left <= near_share(limit.limit) => self.reset_at(limit, now),
            _ => now,
        }
    }

    /// The reset of `limit` (plus a second for clock skew) as an [`Instant`]; converted from
    /// the server's clock once per limit seen, so it does not move afterwards.
    fn reset_at(&mut self, limit: RateLimit, now: Instant) -> Instant {
        if let Some((seen, at)) = self.seen
            && seen == limit
        {
            return at;
        }
        let reset = UNIX_EPOCH + Duration::from_secs(limit.reset.saturating_add(1));
        let at = now
            + reset
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO);
        self.seen = Some((limit, at));
        at
    }
}

/// Whether the budget spreads requests over the rest of the window: fewer than half of the
/// window's requests are `left`.
fn spreads(limit: RateLimit, left: u32) -> bool {
    u64::from(left) * 2 < u64::from(limit.limit)
}

/// Requests of a window of `limit` kept for near rooms: all but [`STATS_FAR_SHARE`] percent.
fn near_share(limit: u32) -> u32 {
    let far = u64::from(limit) * u64::from(STATS_FAR_SHARE) / 100;
    limit.saturating_sub(u32::try_from(far).unwrap_or(limit))
}

/// The sector of `room`: sectors are named by tens (`W0`..`W9`, `E10`..`E19`), so x -10..=-1
/// is one sector.
const fn sector(room: RoomCoord) -> (i32, i32) {
    (room.x.div_euclid(10), room.y.div_euclid(10))
}

/// Now, in milliseconds since the Unix epoch.
pub(super) fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The rooms of the `count` sectors among `rooms` that touch `visible` (all, when `None`)
/// nearest `center`, then of the others nearest it: sector by sector, each sector's rooms
/// nearest first.
fn nearest_sectors(
    rooms: Vec<RoomCoord>,
    center: RoomCoord,
    count: usize,
    visible: Option<RoomRect>,
) -> Vec<RoomCoord> {
    let mut sectors = HashMap::<(i32, i32), Vec<RoomCoord>>::new();
    for room in rooms {
        sectors.entry(sector(room)).or_default().push(room);
    }
    // Distances between centres, doubled to stay whole: a room's centre is at 2x + 1, a
    // sector's at 20x + 10.
    let (cx, cy) = (2 * i64::from(center.x) + 1, 2 * i64::from(center.y) + 1);
    let distance = |x: i64, y: i64| ((x - cx).pow(2) + (y - cy).pow(2), y, x);
    let outside = |(x, y): (i32, i32)| {
        let rect = RoomRect::new(
            RoomCoord {
                x: x * 10,
                y: y * 10,
            },
            RoomCoord {
                x: x * 10 + 9,
                y: y * 10 + 9,
            },
        );
        visible.is_some_and(|visible| visible.intersection(&rect).is_none())
    };
    let mut sectors = sectors.into_iter().collect::<Vec<_>>();
    sectors.sort_by_key(|&((x, y), _)| {
        (
            outside((x, y)),
            distance(20 * i64::from(x) + 10, 20 * i64::from(y) + 10),
        )
    });
    sectors
        .into_iter()
        .take(count)
        .flat_map(|(_, mut rooms)| {
            rooms
                .sort_by_key(|room| distance(2 * i64::from(room.x) + 1, 2 * i64::from(room.y) + 1));
            rooms
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{STATS_BATCH_SECTORS, near_share, nearest_sectors};
    use crate::map::{RoomCoord, RoomRect};

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    #[test]
    fn batches_take_the_nearest_whole_sectors() {
        // 160 × 160 rooms: 256 sectors around W0N0|E0S0.
        let rect = RoomRect::new(room("W79N79"), room("E79S79"));
        let batch = nearest_sectors(
            rect.iter().collect(),
            rect.center(),
            STATS_BATCH_SECTORS,
            None,
        );
        assert_eq!(batch.len(), STATS_BATCH_SECTORS * 100);
        // The four sectors around the centre come first, the nearest room first.
        assert_eq!(batch[0], room("W0N0"));
        let first = batch[..400]
            .iter()
            .all(|room| (-10..10).contains(&room.x) && (-10..10).contains(&room.y));
        assert!(first);
        // Whole sectors around the centre (ties broken north-west first).
        let sectors = batch
            .iter()
            .map(|room| (room.x.div_euclid(10), room.y.div_euclid(10)))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(sectors.len(), STATS_BATCH_SECTORS);
        assert!(
            sectors
                .iter()
                .all(|&(x, y)| (-5..=4).contains(&x) && (-5..=4).contains(&y))
        );
        // Partial sectors keep only their stale rooms.
        let few = vec![room("E15S15"), room("W0N0"), room("E1S1")];
        assert_eq!(
            nearest_sectors(few, room("W0N0"), STATS_BATCH_SECTORS, None),
            [room("W0N0"), room("E1S1"), room("E15S15")]
        );
    }

    #[test]
    fn visible_sectors_go_first() {
        // A wide view east of the centre: its far sector goes before nearer hidden ones.
        let rect = RoomRect::new(room("W39N39"), room("E39S39"));
        let visible = RoomRect::new(room("E30N5"), room("E39S5"));
        let batch = nearest_sectors(rect.iter().collect(), room("W0N0"), 2, Some(visible));
        let sectors = batch
            .iter()
            .map(|room| (room.x.div_euclid(10), room.y.div_euclid(10)))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(sectors, [(3, -1), (3, 0)].into_iter().collect());
    }

    #[test]
    fn far_rooms_leave_thirty_percent_of_the_window_to_near_rooms() {
        assert_eq!(near_share(60), 18);
        assert_eq!(near_share(100), 30);
        assert_eq!(near_share(1), 1);
        assert_eq!(near_share(0), 0);
    }
}
