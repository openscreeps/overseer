//! The request budget: every request the client sends, counted over a sliding window, so that
//! background loading leaves room for the user's own requests.

use std::collections::VecDeque;
use std::time::Duration;

use crate::rt::Instant;

/// The window requests are counted over: screeps.com limits each token to 120 requests per
/// minute across every endpoint.
pub const REQUEST_WINDOW: Duration = Duration::from_mins(1);

/// Most requests per [`REQUEST_WINDOW`] (counting every request of the client, the user's
/// included) up to which a request of [`Priority::Viewed`] may go out on servers with rate
/// limits; [`Priority::Visible`] stops 5 earlier, [`Priority::Background`] 10 earlier. The
/// 10 left of screeps.com's 120 stay for the user's own requests, which never wait.
pub const REQUEST_BUDGET: usize = 110;

/// Longest wait for background requests after HTTP 429 of the server-wide limit (it counts
/// per minute). A refusal that reports the limit's reset waits until then; one without
/// `X-RateLimit-*` headers waits this long.
pub const GLOBAL_RATE_LIMIT_WAIT: Duration = Duration::from_mins(1);

/// How urgent a background request is (see [`Client::budget_ready_at`]).
///
/// [`Client::budget_ready_at`]: crate::Client::budget_ready_at
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// For a room the user views (the focused room, drawers, pop-out windows).
    Viewed,
    /// For what is on screen, or just around it.
    Visible,
    /// For the rest of the world.
    Background,
}

impl Priority {
    /// Most requests per window up to which a request of this priority may go out.
    #[must_use]
    pub const fn budget(self) -> usize {
        match self {
            Self::Viewed => REQUEST_BUDGET,
            Self::Visible => REQUEST_BUDGET - 5,
            Self::Background => REQUEST_BUDGET - 10,
        }
    }
}

/// When the client sent its requests of the latest window, and its rate-limit state.
#[derive(Debug, Default)]
pub(crate) struct Budget {
    /// Send times of the requests of the latest [`REQUEST_WINDOW`], oldest first.
    sent: VecDeque<Instant>,
    /// Until when the latest HTTP 429 of the server-wide limit holds every priority.
    held_until: Option<Instant>,
    /// Until when the token is exempt from rate limits (milliseconds since the Unix epoch).
    exempt_until: Option<u64>,
}

impl Budget {
    /// Counts a request sent at `now`.
    pub(crate) fn record(&mut self, now: Instant) {
        self.prune(now);
        self.sent.push_back(now);
    }

    /// Records HTTP 429 of the server-wide limit: every priority waits until `until`.
    pub(crate) fn refused(&mut self, until: Instant) {
        self.held_until = Some(self.held_until.map_or(until, |held| held.max(until)));
    }

    /// Requests sent in the window ending at `now`.
    pub(crate) fn count(&mut self, now: Instant) -> usize {
        self.prune(now);
        self.sent.len()
    }

    pub(crate) const fn set_exempt_until(&mut self, until: Option<u64>) {
        self.exempt_until = until;
    }

    /// Whether the token is exempt from rate limits at `now_ms` (milliseconds since the Unix
    /// epoch).
    pub(crate) fn is_exempt(&self, now_ms: u64) -> bool {
        self.exempt_until.is_some_and(|until| until > now_ms)
    }

    /// When a request of `priority` may go out after `pending` granted ones not sent yet:
    /// `None` at once (`now`).
    pub(crate) fn ready_at(
        &mut self,
        priority: Priority,
        pending: usize,
        now: Instant,
    ) -> Option<Instant> {
        self.prune(now);
        let refused = self.held_until.filter(|until| *until > now);
        let budget = priority.budget();
        let used = self.sent.len() + pending;
        let window = (used >= budget).then(|| {
            // The oldest requests that must leave the window first.
            self.sent
                .get(used - budget)
                .map_or(now + REQUEST_WINDOW, |at| *at + REQUEST_WINDOW)
        });
        match (refused, window) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        }
    }

    fn prune(&mut self, now: Instant) {
        while self
            .sent
            .front()
            .is_some_and(|at| *at + REQUEST_WINDOW <= now)
        {
            self.sent.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Budget, Priority, REQUEST_BUDGET, REQUEST_WINDOW};
    use crate::rt::Instant;

    #[test]
    fn priorities_stop_at_their_share_of_the_window() {
        let start = Instant::now();
        let mut budget = Budget::default();
        for index in 0..REQUEST_BUDGET - 10 {
            budget.record(start + Duration::from_millis(index as u64));
        }
        let now = start + Duration::from_secs(1);
        assert_eq!(
            budget.ready_at(Priority::Background, 0, now),
            Some(start + REQUEST_WINDOW)
        );
        assert_eq!(budget.ready_at(Priority::Visible, 0, now), None);
        // 5 granted, not sent yet: visible waits too.
        assert_eq!(
            budget.ready_at(Priority::Visible, 5, now),
            Some(start + REQUEST_WINDOW)
        );
        assert_eq!(budget.ready_at(Priority::Viewed, 5, now), None);
        // The window slides.
        let later = start + REQUEST_WINDOW + Duration::from_millis(1);
        assert_eq!(budget.count(later), REQUEST_BUDGET - 12);
        assert_eq!(budget.ready_at(Priority::Background, 0, later), None);
        // HTTP 429 of the server-wide limit: everyone waits until it resets.
        budget.refused(later + REQUEST_WINDOW);
        assert_eq!(
            budget.ready_at(Priority::Viewed, 0, later),
            Some(later + REQUEST_WINDOW)
        );
        assert!(!budget.is_exempt(0));
        budget.set_exempt_until(Some(10));
        assert!(budget.is_exempt(9) && !budget.is_exempt(10));
    }
}
