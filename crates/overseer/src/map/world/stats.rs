//! Room overviews of the world map: batched `owner0` map statistics with a long lifetime,
//! paced by the server's rate limit.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use overseer_net::core::api::types::GameMapStatsResponse;
use overseer_net::core::api::{ApiError, RateLimit, endpoints};
use overseer_net::rt::{BoxFuture, Instant, SystemTime, UNIX_EPOCH};
use overseer_net::{Client, Error as NetError};

use super::WorldEvent;
use crate::map::{Map, MapStat, RoomCoord, RoomRect, RoomStat};
use crate::{Closed, Error};

/// How long a room's overview is used before it is requested again.
pub const STATS_TTL: Duration = Duration::from_mins(10);

/// Most sectors (10 × 10 rooms, at most 2 500 rooms) per map-stats request. The official
/// world map asks for every room of the visible sectors at once; a whole shard at the
/// overview zoom (about 15 000 rooms on screeps.com) is too much for one request, so it is
/// split into requests of the nearest whole sectors and fills progressively.
pub const STATS_BATCH_SECTORS: usize = 25;

/// Shortest pause between two requests, which also gathers rooms entering view meanwhile.
const GAP: Duration = Duration::from_secs(1);

/// Wait after a request failed for another reason than the rate limit.
const RETRY: Duration = Duration::from_secs(30);

/// Wait after HTTP 429 without the endpoint's limit (the server-wide limit counts per minute).
const GLOBAL_RETRY: Duration = Duration::from_secs(60);

/// A room's overview and when it was fetched.
struct Known {
    stat: Arc<RoomStat>,
    fetched: Instant,
}

/// The request in flight: its rooms, nearest first, and the request.
struct Fetch {
    rooms: Vec<RoomCoord>,
    request: BoxFuture<'static, Result<GameMapStatsResponse, Error>>,
}

/// The overviews known, wanted and being fetched, and the request budget.
#[derive(Default)]
pub(super) struct Stats {
    known: HashMap<RoomCoord, Known>,
    /// The rooms wanted, inside the world.
    pub(super) rect: Option<RoomRect>,
    fetch: Option<Fetch>,
    /// No request before this (pacing, rate limit or failure).
    not_before: Option<Instant>,
    /// The latest rate limit seen and its reset as an [`Instant`], converted once from the
    /// server's clock.
    seen: Option<(RateLimit, Instant)>,
}

impl Stats {
    /// The overview of `room`, when fetched.
    pub(super) fn get(&self, room: RoomCoord) -> Option<Arc<RoomStat>> {
        self.known.get(&room).map(|known| Arc::clone(&known.stat))
    }

    /// Requests the overviews of the nearest stale rooms when the budget allows; returns when
    /// to plan again (the budget allows the next request or an overview expires).
    pub(super) fn plan(
        &mut self,
        map: &Map,
        client: &Client,
        shard: Option<&str>,
        now: Instant,
    ) -> Option<Instant> {
        let rect = self.rect?;
        if self.fetch.is_some() {
            return None;
        }
        let mut stale = Vec::new();
        let mut expires: Option<Instant> = None;
        for room in rect.iter() {
            match self.known.get(&room) {
                Some(known) if known.fetched + STATS_TTL > now => {
                    let at = known.fetched + STATS_TTL;
                    expires = Some(expires.map_or(at, |earliest| earliest.min(at)));
                }
                _ => stale.push(room),
            }
        }
        if stale.is_empty() {
            return expires;
        }
        let ready = self.ready_at(client, now);
        if ready > now {
            return Some(ready);
        }
        let batch = nearest_sectors(stale, rect.center());
        let names = batch.iter().map(ToString::to_string).collect::<Vec<_>>();
        let (map, shard) = (map.clone(), shard.map(str::to_owned));
        let request =
            Box::pin(async move { map.stats(names, MapStat::Owner, shard.as_deref()).await });
        self.fetch = Some(Fetch {
            rooms: batch,
            request,
        });
        None
    }

    /// Completes the request in flight: queues a [`WorldEvent::Stats`] with the new and changed
    /// overviews and paces the next request. Returns whether anything changed.
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        client: &Client,
        events: &mut VecDeque<WorldEvent>,
    ) -> Result<bool, Closed> {
        let Some(fetch) = &mut self.fetch else {
            return Ok(false);
        };
        let Poll::Ready(result) = fetch.request.as_mut().poll(cx) else {
            return Ok(false);
        };
        let rooms = std::mem::take(&mut fetch.rooms);
        self.fetch = None;
        let now = Instant::now();
        match result {
            Ok(response) => {
                self.record(&rooms, &response, now, events);
                self.not_before = Some(self.pace(client, now));
            }
            Err(Error::Closed) => return Err(Closed),
            Err(Error::Net(NetError::Api(ApiError::RateLimited(limit)))) => {
                let wait = match limit {
                    Some(limit) => self.reset_at(limit, now).max(now + GAP),
                    None => now + GLOBAL_RETRY,
                };
                self.not_before = Some(wait);
            }
            Err(_) => self.not_before = Some(now + RETRY),
        }
        Ok(true)
    }

    /// Stores the overviews of `rooms` (absent ones are rooms the server does not know) and
    /// queues those that are new or changed.
    fn record(
        &mut self,
        rooms: &[RoomCoord],
        response: &GameMapStatsResponse,
        now: Instant,
        events: &mut VecDeque<WorldEvent>,
    ) {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            });
        let mut changed = Vec::new();
        for &room in rooms {
            let stat = response
                .stats
                .get(&room.to_string())
                .map_or_else(RoomStat::unknown, |stat| {
                    RoomStat::new(stat, &response.users, millis)
                });
            let stat = match self.known.get(&room) {
                Some(known) if *known.stat == stat => Arc::clone(&known.stat),
                _ => {
                    let stat = Arc::new(stat);
                    changed.push((room, Arc::clone(&stat)));
                    stat
                }
            };
            self.known.insert(room, Known { stat, fetched: now });
        }
        if !changed.is_empty() {
            events.push_back(WorldEvent::Stats(changed));
        }
    }

    /// When the next request may go out after one succeeded: after [`GAP`] while at least half
    /// of the window's requests are left, then spread evenly over the rest of the window, and
    /// after the reset once none is left.
    fn pace(&mut self, client: &Client, now: Instant) -> Instant {
        let gap = now + GAP;
        let Some(limit) = client.rate_limit::<endpoints::GameMapStats>() else {
            return gap;
        };
        let reset = self.reset_at(limit, now);
        if limit.remaining == 0 {
            reset.max(gap)
        } else if u64::from(limit.remaining) * 2 < u64::from(limit.limit) {
            (now + reset.saturating_duration_since(now) / limit.remaining).max(gap)
        } else {
            gap
        }
    }

    /// When a request may go out: after the pacing and, when the latest limit the client saw
    /// (from any request) has no request left, after its reset.
    fn ready_at(&mut self, client: &Client, now: Instant) -> Instant {
        let mut ready = self.not_before.unwrap_or(now);
        if let Some(limit) = client.rate_limit::<endpoints::GameMapStats>()
            && limit.remaining == 0
        {
            ready = ready.max(self.reset_at(limit, now));
        }
        ready
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

/// The rooms of the [`STATS_BATCH_SECTORS`] sectors nearest `center` among `rooms`, sector by
/// sector, each sector's rooms nearest first.
fn nearest_sectors(rooms: Vec<RoomCoord>, center: RoomCoord) -> Vec<RoomCoord> {
    let mut sectors = HashMap::<(i32, i32), Vec<RoomCoord>>::new();
    for room in rooms {
        // Sectors are named by tens (`W0`..`W9`, `E10`..`E19`): x -10..=-1 is one sector.
        let sector = (room.x.div_euclid(10), room.y.div_euclid(10));
        sectors.entry(sector).or_default().push(room);
    }
    // Distances between centres, doubled to stay whole: a room's centre is at 2x + 1, a
    // sector's at 20x + 10.
    let (cx, cy) = (2 * i64::from(center.x) + 1, 2 * i64::from(center.y) + 1);
    let distance = |x: i64, y: i64| ((x - cx).pow(2) + (y - cy).pow(2), y, x);
    let mut sectors = sectors.into_iter().collect::<Vec<_>>();
    sectors.sort_by_key(|&((x, y), _)| distance(20 * i64::from(x) + 10, 20 * i64::from(y) + 10));
    sectors
        .into_iter()
        .take(STATS_BATCH_SECTORS)
        .flat_map(|(_, mut rooms)| {
            rooms
                .sort_by_key(|room| distance(2 * i64::from(room.x) + 1, 2 * i64::from(room.y) + 1));
            rooms
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{STATS_BATCH_SECTORS, nearest_sectors};
    use crate::map::{RoomCoord, RoomRect};

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    #[test]
    fn batches_take_the_nearest_whole_sectors() {
        // 80 × 80 rooms: 64 sectors around W0N0|E0S0.
        let rect = RoomRect::new(room("W39N39"), room("E39S39"));
        let batch = nearest_sectors(rect.iter().collect(), rect.center());
        assert_eq!(batch.len(), STATS_BATCH_SECTORS * 100);
        // The four sectors around the centre come first, the nearest room first.
        assert_eq!(batch[0], room("W0N0"));
        let first = batch[..400]
            .iter()
            .all(|room| (-10..10).contains(&room.x) && (-10..10).contains(&room.y));
        assert!(first);
        // Whole sectors: 5 × 5 around the centre (ties broken north-west first).
        let sectors = batch
            .iter()
            .map(|room| (room.x.div_euclid(10), room.y.div_euclid(10)))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(sectors.len(), STATS_BATCH_SECTORS);
        assert!(
            sectors
                .iter()
                .all(|&(x, y)| (-3..=2).contains(&x) && (-3..=2).contains(&y))
        );
        // Partial sectors keep only their stale rooms.
        let few = vec![room("E15S15"), room("W0N0"), room("E1S1")];
        assert_eq!(
            nearest_sectors(few, room("W0N0")),
            [room("W0N0"), room("E1S1"), room("E15S15")]
        );
    }
}
