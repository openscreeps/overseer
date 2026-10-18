//! The tick speed of a shard ([`Map::tick_speed`](crate::Map::tick_speed)): the session's
//! state per followed shard ([`Ticks`]), its requests and the handle ([`TickWatch`]).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::future::{AbortHandle, abortable};
use overseer_net::api::WorldApi as _;
use overseer_net::model::world::tick::{self, MAX_GAP, Observations};
use overseer_net::rt::{self, Instant};
use overseer_net::{Api, Priority};
use tokio::sync::watch;

use crate::{Closed, ConnectionStatus, Overseer, refresh};

/// How often the server's tick duration is asked for.
const SERVER_POLL: Duration = Duration::from_secs(30);
/// How long the server's tick duration stays fresh after its reply: three polls.
const SERVER_FRESH: Duration = Duration::from_secs(90);
/// How often the game time is asked for while it is wanted ([`Estimate::wants_game_time`]).
const TIME_POLL: Duration = Duration::from_secs(15);
/// After this time without a live update no live update arrives.
const LIVE_QUIET: Duration = TIME_POLL;
/// The observations each kind keeps: a short window that follows changes of the speed.
const WINDOW: usize = 16;

/// Where a tick duration comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickSource {
    /// The server reported it.
    Server,
    /// The session measured it from the game time.
    Measured,
}

/// The duration of a tick and its source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickSpeed {
    /// Milliseconds per tick.
    pub ms: f64,
    /// Where it comes from.
    pub source: TickSource,
}

/// The tick speed of one shard, updated while the handle lives: the server's fresh duration,
/// else the measured one, `None` while neither is known.
///
/// Created by [`Map::tick_speed`](crate::Map::tick_speed). Two sources:
///
/// - The server: [`Map::tick_duration`](crate::Map::tick_duration) every 30 seconds. Its
///   duration stays fresh for 90 seconds after the reply (three polls: a failing poll or two
///   keep it).
/// - The game time: the game tick of each live room update the session receives for the shard
///   (any room watch: the map, a room window, …), and `GET /api/game/time` every 15 seconds
///   while the server gives no fresh duration and no live update arrived for 15 seconds. The
///   intervals of the last 16 observations of each kind give the measured duration
///   ([`tick::estimate`]). Live updates arrive every tick, so two of them more than
///   [`tick::MAX_GAP`] ticks apart give no interval; the polls span many ticks of a fast
///   server and give an interval at every gap. The two kinds are never paired: a live update
///   arrives when a tick starts, a polled tick at any time in the tick.
///
/// Every handle of a shard (clones and handles of other callers in the same session) shares
/// one state and one series of requests: the first handle of a shard starts the requests, the
/// last one dropped stops them and forgets what was measured. Each request waits for the
/// background request budget first ([`Priority::Background`]).
#[derive(Clone, Debug)]
pub struct TickWatch {
    receiver: watch::Receiver<Option<TickSpeed>>,
    follower: Arc<Follower>,
}

impl TickWatch {
    /// The followed shard; `None` on single-world servers.
    #[must_use]
    pub fn shard(&self) -> Option<&str> {
        self.follower.shard.as_deref()
    }

    /// The latest tick speed; `None` while it is not known.
    #[must_use]
    pub fn latest(&self) -> Option<TickSpeed> {
        *self.receiver.borrow()
    }

    /// Waits for a tick speed different from the latest one this handle returned or saw with
    /// [`Self::latest`] and returns it. Cancel-safe.
    ///
    /// # Errors
    ///
    /// Returns [`Closed`] once the session is shut down.
    pub async fn changed(&mut self) -> Result<Option<TickSpeed>, Closed> {
        self.receiver.changed().await.map_err(|_| Closed)?;
        Ok(*self.receiver.borrow_and_update())
    }
}

/// One [`TickWatch`] and its clones: dropping it releases its shard.
#[derive(Debug)]
struct Follower {
    ticks: Arc<Ticks>,
    shard: Option<String>,
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.ticks.unfollow(self.shard.as_deref());
    }
}

/// Starts following `shard` (already resolved) in the session of `overseer`: the first
/// follower of a shard starts its requests.
pub(crate) fn follow(overseer: &Overseer, shard: Option<String>) -> TickWatch {
    let ticks = Arc::clone(overseer.ticks());
    let (receiver, first) = ticks.follow(shard.as_deref());
    if first {
        let (future, handle) = abortable(poll(
            Arc::clone(&ticks),
            overseer.api().clone(),
            overseer.status(),
            shard.clone(),
        ));
        ticks.set_task(shard.as_deref(), handle);
        rt::spawn(async move {
            // `Err` only says it was aborted.
            let _ = future.await;
        });
    }
    TickWatch {
        receiver,
        follower: Arc::new(Follower { ticks, shard }),
    }
}

/// The requests of `shard` until the session closes: then every watch ends.
async fn poll(
    ticks: Arc<Ticks>,
    api: Api,
    mut status: watch::Receiver<ConnectionStatus>,
    shard: Option<String>,
) {
    let shard = shard.as_deref();
    tokio::select! {
        () = poll_server(&ticks, &api, shard) => {}
        () = poll_game_time(&ticks, &api, shard) => {}
        () = refresh::closed(&mut status) => {}
    }
    ticks.close();
}

/// Asks the server for the tick duration of `shard` every [`SERVER_POLL`]. A failed request
/// keeps the last duration until it is stale.
async fn poll_server(ticks: &Ticks, api: &Api, shard: Option<&str>) {
    loop {
        api.client().wait_for_budget(Priority::Background, 0).await;
        if let Ok(ms) = api.tick_duration(shard).await {
            ticks.update(shard, |estimate, now| estimate.report(ms, now));
        }
        rt::sleep(SERVER_POLL).await;
    }
}

/// Asks for the game time of `shard` every [`TIME_POLL`] while
/// [`Estimate::wants_game_time`]; the first time after one period, when the other sources had
/// their chance. Also wakes when the server's duration goes stale, to publish the change.
async fn poll_game_time(ticks: &Ticks, api: &Api, shard: Option<&str>) {
    let mut next_poll = Instant::now() + TIME_POLL;
    loop {
        let stale_at = ticks.read(shard, Estimate::stale_at).flatten();
        rt::sleep_until(stale_at.map_or(next_poll, |stale| stale.min(next_poll))).await;
        ticks.update(shard, |_, _| {});
        let now = Instant::now();
        if now < next_poll {
            continue;
        }
        next_poll = now + TIME_POLL;
        if !ticks
            .read(shard, Estimate::wants_game_time)
            .unwrap_or(false)
        {
            continue;
        }
        api.client().wait_for_budget(Priority::Background, 0).await;
        if let Ok(tick) = api.game_time(shard).await {
            ticks.update(shard, |estimate, now| estimate.observe_polled(tick, now));
        }
    }
}

/// The tick speeds of the session's followed shards.
#[derive(Debug, Default)]
pub(crate) struct Ticks {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    /// Set once the session closed: nothing is followed any more.
    closed: bool,
    /// The followed shards: a few at most.
    shards: Vec<Followed>,
}

impl State {
    /// The followed `shard`.
    fn get(&mut self, shard: Option<&str>) -> Option<&mut Followed> {
        self.shards
            .iter_mut()
            .find(|followed| followed.shard.as_deref() == shard)
    }
}

/// One followed shard.
#[derive(Debug)]
struct Followed {
    /// The shard; `None` on single-world servers.
    shard: Option<String>,
    estimate: Estimate,
    sender: watch::Sender<Option<TickSpeed>>,
    /// The live [`Follower`]s.
    followers: usize,
    /// Stops the shard's requests.
    task: Option<AbortHandle>,
}

impl Followed {
    /// Publishes the tick speed at `now` if it changed.
    fn publish(&self, now: Instant) {
        let speed = self.estimate.speed(now);
        self.sender.send_if_modified(|current| {
            let changed = *current != speed;
            *current = speed;
            changed
        });
    }
}

impl Ticks {
    /// Adds a follower of `shard`: the receiver of its tick speed, and whether it is the first
    /// (which starts the requests). After the session closed, a receiver that is closed already.
    fn follow(&self, shard: Option<&str>) -> (watch::Receiver<Option<TickSpeed>>, bool) {
        let mut state = lock(&self.state);
        if state.closed {
            return (watch::channel(None).1, false);
        }
        if let Some(followed) = state.get(shard) {
            followed.followers += 1;
            return (followed.sender.subscribe(), false);
        }
        let followed = Followed {
            shard: shard.map(str::to_owned),
            estimate: Estimate::new(Instant::now()),
            sender: watch::Sender::new(None),
            followers: 1,
            task: None,
        };
        let receiver = followed.sender.subscribe();
        state.shards.push(followed);
        (receiver, true)
    }

    /// Keeps the handle that stops the requests of `shard`.
    fn set_task(&self, shard: Option<&str>, task: AbortHandle) {
        match lock(&self.state).get(shard) {
            Some(followed) => followed.task = Some(task),
            None => task.abort(),
        }
    }

    /// Removes a follower of `shard`; the last one stops its requests and forgets it.
    fn unfollow(&self, shard: Option<&str>) {
        let mut state = lock(&self.state);
        let Some(index) = state
            .shards
            .iter()
            .position(|followed| followed.shard.as_deref() == shard)
        else {
            return;
        };
        let followed = &mut state.shards[index];
        followed.followers -= 1;
        if followed.followers == 0
            && let Some(task) = state.shards.swap_remove(index).task
        {
            task.abort();
        }
    }

    /// The session closed: drops every shard, which ends their watches.
    pub(crate) fn close(&self) {
        let mut state = lock(&self.state);
        state.closed = true;
        for followed in std::mem::take(&mut state.shards) {
            if let Some(task) = followed.task {
                task.abort();
            }
        }
    }

    /// A live room update of `shard` at game tick `tick` arrived now.
    pub(crate) fn observe_live(&self, shard: Option<&str>, tick: u64) {
        self.update(shard, |estimate, now| estimate.observe_live(tick, now));
    }

    /// Applies `change` to the estimate of `shard` (if followed) at the current time and
    /// publishes the result.
    fn update(&self, shard: Option<&str>, change: impl FnOnce(&mut Estimate, Instant)) {
        if let Some(followed) = lock(&self.state).get(shard) {
            let now = Instant::now();
            change(&mut followed.estimate, now);
            followed.publish(now);
        }
    }

    /// Reads the estimate of `shard` at the current time; `None` when not followed.
    fn read<T>(
        &self,
        shard: Option<&str>,
        read: impl FnOnce(&Estimate, Instant) -> T,
    ) -> Option<T> {
        lock(&self.state)
            .get(shard)
            .map(|followed| read(&followed.estimate, Instant::now()))
    }
}

/// What is known of the tick duration of one shard.
#[derive(Clone, Debug, PartialEq)]
struct Estimate {
    /// The zero of the observations' times.
    origin: Instant,
    /// Ticks of live room updates.
    live: Observations,
    /// Ticks of `GET /api/game/time`.
    polled: Observations,
    /// When the latest live update arrived.
    live_at: Option<Instant>,
    /// The server's latest known duration (milliseconds per tick) and when it arrived.
    server: Option<(f64, Instant)>,
}

impl Estimate {
    /// Nothing known yet; observation times count from `origin`.
    fn new(origin: Instant) -> Self {
        Self {
            origin,
            live: Observations::new(WINDOW),
            polled: Observations::new(WINDOW),
            live_at: None,
            server: None,
        }
    }

    /// `at` in milliseconds since the origin.
    fn millis(&self, at: Instant) -> f64 {
        at.saturating_duration_since(self.origin).as_secs_f64() * 1000.0
    }

    /// A live room update at tick `tick` arrived at `at`.
    fn observe_live(&mut self, tick: u64, at: Instant) {
        self.live.push(tick, self.millis(at));
        self.live_at = Some(at);
    }

    /// `GET /api/game/time` said tick `tick` at `at`.
    fn observe_polled(&mut self, tick: u64, at: Instant) {
        self.polled.push(tick, self.millis(at));
    }

    /// The server reported the duration `ms` at `at` (`None`: it does not know it).
    fn report(&mut self, ms: Option<f64>, at: Instant) {
        self.server = ms.map(|ms| (ms, at));
    }

    /// The duration measured from the game time: the estimate of the live and polled
    /// intervals.
    fn measured(&self) -> Option<f64> {
        tick::estimate(
            self.live
                .intervals(Some(MAX_GAP))
                .chain(self.polled.intervals(None)),
        )
    }

    /// The server's duration while fresh at `now`.
    fn fresh_server(&self, now: Instant) -> Option<f64> {
        self.server
            .filter(|&(_, at)| now.saturating_duration_since(at) < SERVER_FRESH)
            .map(|(ms, _)| ms)
    }

    /// When the server's duration, fresh at `now`, goes stale.
    fn stale_at(&self, now: Instant) -> Option<Instant> {
        self.fresh_server(now)?;
        self.server.map(|(_, at)| at + SERVER_FRESH)
    }

    /// The tick speed at `now`: the server's while fresh, else the measured one.
    fn speed(&self, now: Instant) -> Option<TickSpeed> {
        self.fresh_server(now)
            .map(|ms| TickSpeed {
                ms,
                source: TickSource::Server,
            })
            .or_else(|| {
                self.measured().map(|ms| TickSpeed {
                    ms,
                    source: TickSource::Measured,
                })
            })
    }

    /// Whether to ask for the game time at `now`: the server gives no fresh duration and no
    /// live update arrived for [`LIVE_QUIET`].
    fn wants_game_time(&self, now: Instant) -> bool {
        self.fresh_server(now).is_none()
            && self
                .live_at
                .is_none_or(|at| now.saturating_duration_since(at) >= LIVE_QUIET)
    }
}

/// Locks a mutex whose critical sections cannot panic halfway through an update.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use overseer_net::rt::Instant;

    use super::{Estimate, LIVE_QUIET, SERVER_FRESH, TickSource, TickSpeed, Ticks};

    /// `ms` milliseconds after `origin`.
    fn after(origin: Instant, ms: u64) -> Instant {
        origin + Duration::from_millis(ms)
    }

    /// The fresh duration of the server.
    fn fresh_ms() -> u64 {
        u64::try_from(SERVER_FRESH.as_millis()).unwrap()
    }

    #[test]
    fn live_and_polled_ticks_are_not_paired() {
        let origin = Instant::now();
        let mut estimate = Estimate::new(origin);
        estimate.observe_polled(100, origin);
        estimate.observe_live(101, after(origin, 500));
        estimate.observe_polled(105, after(origin, 20_000));
        estimate.observe_live(102, after(origin, 3500));
        // Live: 3 s per tick; polled: 20 s for 5 ticks.
        assert_eq!(estimate.measured(), Some(3500.0));
    }

    #[test]
    fn a_fresh_server_value_wins_over_the_measured_one() {
        let origin = Instant::now();
        let mut estimate = Estimate::new(origin);
        assert_eq!(estimate.speed(origin), None);
        for (tick, at) in [(1, 0), (2, 3000), (3, 6000)] {
            estimate.observe_live(tick, after(origin, at));
        }
        let measured = Some(TickSpeed {
            ms: 3000.0,
            source: TickSource::Measured,
        });
        assert_eq!(estimate.speed(after(origin, 6000)), measured);
        estimate.report(Some(3150.5), after(origin, 7000));
        assert_eq!(
            estimate.speed(after(origin, 8000)),
            Some(TickSpeed {
                ms: 3150.5,
                source: TickSource::Server,
            })
        );
        assert_eq!(
            estimate.stale_at(after(origin, 8000)),
            Some(after(origin, 7000 + fresh_ms()))
        );
        // A stale server value gives way to the measured one.
        let stale = after(origin, 7000 + fresh_ms());
        assert_eq!(estimate.speed(stale), measured);
        assert_eq!(estimate.stale_at(stale), None);
        // So does a server that does not know it.
        estimate.report(None, after(origin, 9000));
        assert_eq!(estimate.speed(after(origin, 9000)), measured);
    }

    #[test]
    fn the_game_time_is_asked_for_only_without_other_sources() {
        let origin = Instant::now();
        let quiet = u64::try_from(LIVE_QUIET.as_millis()).unwrap();
        let mut estimate = Estimate::new(origin);
        assert!(estimate.wants_game_time(origin));
        estimate.observe_live(1, after(origin, 1000));
        assert!(!estimate.wants_game_time(after(origin, 1000 + quiet - 1)));
        assert!(estimate.wants_game_time(after(origin, 1000 + quiet)));
        estimate.report(Some(3000.0), after(origin, 20_000));
        assert!(!estimate.wants_game_time(after(origin, 20_000)));
        assert!(estimate.wants_game_time(after(origin, 20_000 + fresh_ms())));
        estimate.report(None, after(origin, 30_000));
        assert!(estimate.wants_game_time(after(origin, 30_000)));
    }

    #[test]
    fn shards_are_followed_apart_and_forgotten_by_their_last_follower() {
        let ticks = Ticks::default();
        let (first, starts) = ticks.follow(Some("shard1"));
        assert!(starts);
        let (second, starts) = ticks.follow(Some("shard1"));
        assert!(!starts, "one series of requests per shard");
        let (other, starts) = ticks.follow(Some("shard2"));
        assert!(starts);
        // Live updates of shard1, one second apart: 1 s per tick on shard1 only.
        for tick in 10..13 {
            ticks.update(Some("shard1"), |estimate, _| {
                let at = estimate.origin + Duration::from_secs(tick - 10);
                estimate.observe_live(tick, at);
            });
        }
        let measured = Some(TickSpeed {
            ms: 1000.0,
            source: TickSource::Measured,
        });
        assert_eq!(*first.borrow(), measured);
        assert_eq!(*second.borrow(), measured);
        assert_eq!(*other.borrow(), None);
        // Ticks of a shard nobody follows are not kept.
        ticks.observe_live(Some("shard3"), 1);
        assert!(ticks.read(Some("shard3"), |_, _| ()).is_none());
        // The last follower forgets the shard: a new one starts over.
        ticks.unfollow(Some("shard1"));
        assert!(ticks.read(Some("shard1"), |_, _| ()).is_some());
        ticks.unfollow(Some("shard1"));
        assert!(first.has_changed().is_err(), "the watch ended");
        let (again, starts) = ticks.follow(Some("shard1"));
        assert!(starts);
        assert_eq!(*again.borrow(), None);
        // Closing the session ends every watch, and nothing is followed after it.
        ticks.close();
        assert!(other.has_changed().is_err());
        let (late, starts) = ticks.follow(Some("shard2"));
        assert!(!starts);
        assert!(late.has_changed().is_err());
    }
}
