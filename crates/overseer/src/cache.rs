//! When a cached read of the session is old enough to read again.

use std::time::Duration;

use overseer_net::rt::Instant;

/// How long a cached read stays in use: [`Self::fresh`] after a read that gave something,
/// [`Self::fresh_empty`] after a read that gave nothing or failed (a first creep, or a busy
/// server, then shows up sooner).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Freshness {
    /// How long a read that gave something stays in use.
    pub fresh: Duration,
    /// How long a read that gave nothing, or failed, stays in use.
    pub fresh_empty: Duration,
}

impl Freshness {
    /// Reads that stay in use for `fresh`, and for `fresh_empty` when they gave nothing.
    #[must_use]
    pub const fn new(fresh: Duration, fresh_empty: Duration) -> Self {
        Self { fresh, fresh_empty }
    }
}

/// The reads of one cached part: when the last one ended, whether it gave nothing, and how
/// many run now.
#[derive(Debug, Default)]
pub(crate) struct Load {
    /// When the last read ended (also after a failure), and whether it gave nothing.
    last: Option<(Instant, bool)>,
    /// The reads that run now.
    running: usize,
}

impl Load {
    /// Whether the part must be read again: no read runs, and the last one is older than
    /// `freshness` allows (or there was none).
    pub(crate) fn stale(&self, freshness: &Freshness) -> bool {
        self.running == 0
            && self.last.is_none_or(|(at, empty)| {
                let fresh = if empty {
                    freshness.fresh_empty
                } else {
                    freshness.fresh
                };
                at.elapsed() >= fresh
            })
    }

    /// Whether a read runs.
    pub(crate) const fn running(&self) -> bool {
        self.running > 0
    }

    /// Marks a read as started.
    pub(crate) const fn start(&mut self) {
        self.running += 1;
    }

    /// Marks a read as ended; `empty` when it gave nothing or failed.
    pub(crate) fn finish(&mut self, empty: bool) {
        self.running = self.running.saturating_sub(1);
        self.last = Some((Instant::now(), empty));
    }

    /// Marks a read as ended without a result (it was dropped before it ended).
    pub(crate) const fn cancel(&mut self) {
        self.running = self.running.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{Freshness, Load};

    const FRESHNESS: Freshness = Freshness::new(Duration::from_mins(5), Duration::ZERO);

    #[test]
    fn a_part_is_read_again_once_old() {
        let mut load = Load::default();
        assert!(load.stale(&FRESHNESS));
        load.start();
        // A read runs: no second one.
        assert!(load.running());
        assert!(!load.stale(&FRESHNESS));
        load.finish(false);
        assert!(!load.running());
        assert!(!load.stale(&FRESHNESS));
        // An empty result is old at once here.
        load.start();
        load.finish(true);
        assert!(load.stale(&FRESHNESS));
        // A dropped read keeps the last result.
        load.start();
        load.cancel();
        assert!(load.stale(&FRESHNESS));
    }
}
