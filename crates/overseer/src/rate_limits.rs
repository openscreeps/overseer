//! The session's exemption from rate limits: until when the session's API token is exempt
//! (screeps.com's `noRatelimitUntil`), and when to check it again.
//!
//! screeps.com limits each API token per endpoint and lets the user lift the limits for about
//! two hours ([`Account::rate_limit_lift`](crate::Account::rate_limit_lift)); so does
//! screepsmod-auth on community servers ([`Subsystem::RateLimitLift`]). The session keeps the
//! exemption as last checked ([`RateLimits::exemption`]) and gives it to the client at once
//! ([`net::Client::set_rate_limit_exempt_until`](crate::net::Client::set_rate_limit_exempt_until)),
//! whose request budget then lets every request go. Every [`WorldMap`](crate::WorldMap) of the
//! session follows it too. Each check is one `GET /api/user/auth-token`
//! ([`Account::rate_limit_exemption`](crate::Account::rate_limit_exemption)).
//!
//! The user may lift the limits on the website while the app runs. While an app follows the
//! exemption ([`RateLimits::follow`]), the session checks it on a schedule.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use overseer_net::Client;
use overseer_net::model::platform::Subsystem;
use overseer_net::rt;
use tokio::sync::{Notify, watch};

use crate::account::epoch_millis;
use crate::refresh::closed;
use crate::{Error, Overseer};

/// How often the exemption is checked again while it lasts (it may be extended elsewhere), in
/// milliseconds.
pub const EXEMPT_RECHECK_MS: u64 = 15 * 60_000;

/// Shortest time between two checks caused by HTTP 429 replies, in milliseconds.
pub const RATE_LIMITED_DEBOUNCE_MS: u64 = 30_000;

/// Shortest time from the latest check to one caused by the user's attention coming back
/// ([`RateLimits::attention`]), in milliseconds.
pub const ATTENTION_THROTTLE_MS: u64 = 15_000;

/// How often the exemption is checked while there is none and loading waits on the limits
/// ([`Following::set_held_back`]), in milliseconds.
pub const HELD_BACK_RECHECK_MS: u64 = 60_000;

/// How often a follower looks at the schedule.
const POLL: Duration = Duration::from_secs(1);

/// The session's exemption from rate limits, as last checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Exemption {
    /// Until when the session's token is exempt from rate limits, in milliseconds since the
    /// Unix epoch; `None` when it is not, or before the first check. The time can be past:
    /// see [`Self::until_after`].
    pub until: Option<u64>,
    /// A check has completed and none is in flight: [`Self::until`] says what the server
    /// said.
    pub settled: bool,
}

impl Exemption {
    /// [`Self::until`] while it is after `now` (milliseconds since the Unix epoch).
    #[must_use]
    pub fn until_after(&self, now: u64) -> Option<u64> {
        self.until.filter(|until| *until > now)
    }
}

/// When to check whether the token is exempt from rate limits, for one session; one check at
/// a time. A check is due:
///
/// - at once: when following starts and when asked ([`RateLimits::request_check`]);
/// - after new HTTP 429 replies, at most [`RATE_LIMITED_DEBOUNCE_MS`] after the latest check;
/// - after the user's attention came back ([`RateLimits::attention`]), at most
///   [`ATTENTION_THROTTLE_MS`] after the latest check;
/// - while exempt, every [`EXEMPT_RECHECK_MS`] and when the exemption ends;
/// - while not exempt and loading waits on the limits, every [`HELD_BACK_RECHECK_MS`]. An idle
///   app sends nothing.
///
/// Times are milliseconds since the Unix epoch.
#[derive(Debug, Default)]
pub(crate) struct CheckSchedule {
    /// A check is in flight.
    in_flight: bool,
    /// Asked for since the latest check: the shortest time from the latest check that the
    /// asks allow.
    wanted: Option<u64>,
    /// When the latest check started.
    last_check: Option<u64>,
    /// While exempt: when to check again.
    next_check: Option<u64>,
    /// The client's count of HTTP 429 replies already seen.
    rate_limited: u64,
}

impl CheckSchedule {
    /// A schedule for a client that counted `rate_limited` HTTP 429 replies so far: a check
    /// is due at once.
    pub(crate) fn new(rate_limited: u64) -> Self {
        Self {
            wanted: Some(0),
            rate_limited,
            ..Self::default()
        }
    }

    /// Asks for a check at least `throttle` after the latest one.
    fn want(&mut self, throttle: u64) {
        self.wanted = Some(self.wanted.map_or(throttle, |wanted| wanted.min(throttle)));
    }

    /// Checks at once.
    pub(crate) fn request(&mut self) {
        self.want(0);
    }

    /// The user's attention came back.
    pub(crate) fn attention(&mut self) {
        self.want(ATTENTION_THROTTLE_MS);
    }

    /// Whether a check has completed and none is in flight.
    pub(crate) const fn settled(&self) -> bool {
        self.last_check.is_some() && !self.in_flight
    }

    /// Whether a check is due at `now`, the client having counted `rate_limited` HTTP 429
    /// replies so far, the token `exempt` (as last checked) or not, and loading `held_back`
    /// by the limits or not. A due check counts as started.
    pub(crate) fn due(
        &mut self,
        now: u64,
        rate_limited: u64,
        exempt: bool,
        held_back: bool,
    ) -> bool {
        if rate_limited > self.rate_limited {
            self.rate_limited = rate_limited;
            self.want(RATE_LIMITED_DEBOUNCE_MS);
        }
        if self.in_flight {
            return false;
        }
        let since = self
            .last_check
            .map_or(u64::MAX, |last| now.saturating_sub(last));
        let due = self.wanted.is_some_and(|wanted| since >= wanted)
            || self.next_check.is_some_and(|at| now >= at)
            || (!exempt && held_back && since >= HELD_BACK_RECHECK_MS);
        if due {
            *self = Self {
                in_flight: true,
                last_check: Some(now),
                rate_limited: self.rate_limited,
                ..Self::default()
            };
        }
        due
    }

    /// The check finished at `now`: exempt until `until`, or not exempt or failed (`None`).
    pub(crate) fn finished(&mut self, now: u64, until: Option<u64>) {
        self.in_flight = false;
        self.next_check = until.map(|until| until.min(now + EXEMPT_RECHECK_MS));
    }
}

/// The exemption state that the session keeps for every [`RateLimits`] handle.
pub(crate) struct Shared {
    exemption: watch::Sender<Exemption>,
    schedule: Mutex<CheckSchedule>,
    /// Wakes the follower after a request or the user's attention.
    wake: Notify,
    /// The follower while one runs.
    follower: Mutex<Weak<Follower>>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            exemption: watch::Sender::new(Exemption::default()),
            schedule: Mutex::default(),
            wake: Notify::new(),
            follower: Mutex::default(),
        }
    }
}

impl Shared {
    /// Records that the session's token is exempt until `until` (or not) and tells `client`.
    pub(crate) fn store(&self, client: &Client, until: Option<u64>) {
        client.set_rate_limit_exempt_until(until);
        self.exemption.send_if_modified(|exemption| {
            let changed = exemption.until != until;
            exemption.until = until;
            changed
        });
    }

    /// Publishes whether the schedule is settled.
    fn publish_settled(&self) {
        let settled = lock(&self.schedule).settled();
        self.exemption.send_if_modified(|exemption| {
            let changed = exemption.settled != settled;
            exemption.settled = settled;
            changed
        });
    }
}

/// The session's exemption from rate limits (see the [module](self) docs).
///
/// Created by [`Overseer::rate_limits`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct RateLimits {
    overseer: Overseer,
}

impl RateLimits {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// The exemption as last checked, and every change of it. Every receiver shares one
    /// state; reading it sends no request.
    #[must_use]
    pub fn exemption(&self) -> watch::Receiver<Exemption> {
        self.state().exemption.subscribe()
    }

    /// Records that the session's token is exempt from rate limits until `until`
    /// (milliseconds since the Unix epoch), or not (`None`), as learned without a check of
    /// the session: tells the client at once
    /// ([`net::Client::set_rate_limit_exempt_until`](crate::net::Client::set_rate_limit_exempt_until))
    /// and publishes it ([`Self::exemption`]). The next check replaces it.
    pub fn set_exempt_until(&self, until: Option<u64>) {
        self.state().store(self.overseer.client(), until);
    }

    /// Checks the exemption on the session's behalf for as long as a returned [`Following`]
    /// (or a clone) lives, one check at a time. Every caller shares one follower. A check is
    /// due:
    ///
    /// - at once: when following starts and when asked ([`Self::request_check`]);
    /// - after new HTTP 429 replies, at most [`RATE_LIMITED_DEBOUNCE_MS`] after the latest
    ///   check;
    /// - after the user's attention came back ([`Self::attention`]), at most
    ///   [`ATTENTION_THROTTLE_MS`] after the latest check;
    /// - while exempt, every [`EXEMPT_RECHECK_MS`] and when the exemption ends;
    /// - while not exempt and loading waits on the limits ([`Following::set_held_back`]),
    ///   every [`HELD_BACK_RECHECK_MS`]. Otherwise nothing is sent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`], and [`Error::Net`] with
    /// [`Unsupported`](overseer_net::Error::Unsupported) on a server that does not lift rate
    /// limits.
    pub fn follow(&self) -> Result<Following, Error> {
        let api = self.overseer.user_api()?;
        if !api.supports(Subsystem::RateLimitLift) {
            return Err(overseer_net::Error::Unsupported(Subsystem::RateLimitLift).into());
        }
        let shared = self.state();
        let mut slot = lock(&shared.follower);
        if let Some(follower) = slot.upgrade() {
            return Ok(Following { follower });
        }
        *lock(&shared.schedule) = CheckSchedule::new(self.overseer.client().rate_limited_count());
        let (held_back, receiver) = watch::channel(false);
        let follower = Arc::new(Follower { held_back });
        *slot = Arc::downgrade(&follower);
        drop(slot);
        // Detached: the task ends with the last `Following` or the session.
        let _task = rt::spawn(follow(self.overseer.clone(), receiver));
        Ok(Following { follower })
    }

    /// Asks the follower to check at once: for example after the user lifted the limits.
    pub fn request_check(&self) {
        lock(&self.state().schedule).request();
        self.state().wake.notify_one();
    }

    /// The user's attention came back (window focus, page visible again; the user may come
    /// back from lifting the limits on the website): the follower checks, at most
    /// [`ATTENTION_THROTTLE_MS`] after the latest check.
    pub fn attention(&self) {
        lock(&self.state().schedule).attention();
        self.state().wake.notify_one();
    }

    fn state(&self) -> &Shared {
        self.overseer.rate_limit_state()
    }
}

/// What the followers of the exemption share.
#[derive(Debug)]
struct Follower {
    held_back: watch::Sender<bool>,
}

/// Keeps the session checking its exemption ([`RateLimits::follow`]); dropping the last clone
/// stops the checks.
#[derive(Clone, Debug)]
pub struct Following {
    follower: Arc<Follower>,
}

impl Following {
    /// Tells whether loading waits on the rate limits (for example
    /// [`WorldProgress::waits_on_budget`](crate::map::WorldProgress::waits_on_budget)):
    /// then, while not exempt, the session checks every [`HELD_BACK_RECHECK_MS`].
    pub fn set_held_back(&self, held_back: bool) {
        self.follower.held_back.send_if_modified(|current| {
            let changed = *current != held_back;
            *current = held_back;
            changed
        });
    }
}

/// Checks the exemption of `overseer`'s session as its schedule says, until `held_back`'s
/// sender (the last [`Following`]) is gone or the session closes.
async fn follow(overseer: Overseer, mut held_back: watch::Receiver<bool>) {
    let shared = overseer.rate_limit_state();
    let account = overseer.account();
    let mut status = overseer.status();
    loop {
        let now = epoch_millis();
        let exempt = shared.exemption.borrow().until_after(now).is_some();
        let due = lock(&shared.schedule).due(
            now,
            overseer.client().rate_limited_count(),
            exempt,
            *held_back.borrow(),
        );
        if due {
            shared.publish_settled();
            let result = tokio::select! {
                biased;
                () = closed(&mut status) => return,
                result = account.rate_limit_exemption() => result,
            };
            // A failure keeps what is known.
            let until = result.unwrap_or_else(|_| shared.exemption.borrow().until);
            lock(&shared.schedule).finished(epoch_millis(), until);
            shared.publish_settled();
            continue;
        }
        tokio::select! {
            biased;
            () = closed(&mut status) => return,
            changed = held_back.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            () = shared.wake.notified() => {}
            () = rt::sleep(POLL) => {}
        }
    }
}

/// Locks a mutex whose critical sections cannot panic halfway through an update.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
