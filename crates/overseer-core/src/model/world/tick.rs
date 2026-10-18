//! The duration of a game tick, measured from game ticks and the times they arrived.
//!
//! An [`Observation`] is a game tick seen at a time. [`Observation::ms_per_tick`] divides the
//! time between two observations by the ticks between them. [`estimate`] takes the middle
//! ([`median`]) of such intervals once there are [`MIN_INTERVALS`]: a late or bunched update
//! moves it little. [`Observations`] keeps the latest observations of one stream, one per tick,
//! and gives their intervals.
//!
//! Live updates (room snapshots) arrive every tick: two of them more than [`MAX_GAP`] ticks
//! apart give no interval (updates were lost, the connection was down, or the ticks of another
//! shard came in). Polls of the game time are slow and span many ticks of a fast server: their
//! pairs give an interval at every gap.

use std::collections::VecDeque;

/// The most ticks between two live updates that still give an interval.
pub const MAX_GAP: u64 = 5;

/// The intervals an estimate needs: one alone can be a late update.
pub const MIN_INTERVALS: usize = 2;

/// A game tick seen at a time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    /// The game tick.
    pub tick: u64,
    /// When it arrived, in milliseconds on a clock of the caller's choice.
    pub at: f64,
}

impl Observation {
    /// The milliseconds per tick from this observation to the `later` one: the time between
    /// the two divided by the ticks between them. `None` when the tick does not change or goes
    /// back, when no time passed, and, with a `max_gap`, when they are more than `max_gap`
    /// ticks apart.
    #[must_use]
    pub fn ms_per_tick(self, later: Self, max_gap: Option<u64>) -> Option<f64> {
        let ticks = later.tick.checked_sub(self.tick)?;
        let elapsed = later.at - self.at;
        if ticks == 0 || max_gap.is_some_and(|max| ticks > max) || elapsed <= 0.0 {
            return None;
        }
        Some(elapsed / f64::from(u32::try_from(ticks).ok()?))
    }
}

/// The middle of `values` (the mean of the two middle ones for an even count); `None` without
/// values.
#[must_use]
pub fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        count if count % 2 == 1 => Some(values[middle]),
        _ => Some(f64::midpoint(values[middle - 1], values[middle])),
    }
}

/// The tick duration of `intervals` (milliseconds per tick): their [`median`], once there are
/// [`MIN_INTERVALS`].
#[must_use]
pub fn estimate(intervals: impl IntoIterator<Item = f64>) -> Option<f64> {
    let intervals: Vec<f64> = intervals.into_iter().collect();
    if intervals.len() < MIN_INTERVALS {
        return None;
    }
    median(intervals)
}

/// The latest observations of one stream of game ticks, oldest first, one per tick.
#[derive(Clone, Debug, PartialEq)]
pub struct Observations {
    /// The observations, oldest first.
    items: VecDeque<Observation>,
    /// The most observations kept.
    window: usize,
}

impl Observations {
    /// No observations; keeps the latest `window` ones (at least two).
    #[must_use]
    pub fn new(window: usize) -> Self {
        let window = window.max(2);
        Self {
            items: VecDeque::with_capacity(window),
            window,
        }
    }

    /// Adds tick `tick` seen at `at`. The same tick again keeps its first arrival (the start
    /// of the tick); a tick before the latest one starts over (the server restarted).
    pub fn push(&mut self, tick: u64, at: f64) {
        match self.items.back() {
            Some(latest) if latest.tick == tick => return,
            Some(latest) if latest.tick > tick => self.items.clear(),
            _ => {}
        }
        if self.items.len() == self.window {
            self.items.pop_front();
        }
        self.items.push_back(Observation { tick, at });
    }

    /// The milliseconds per tick of each pair of consecutive observations
    /// ([`Observation::ms_per_tick`] with `max_gap`), oldest first.
    pub fn intervals(&self, max_gap: Option<u64>) -> impl Iterator<Item = f64> + '_ {
        self.items
            .iter()
            .zip(self.items.iter().skip(1))
            .filter_map(move |(first, second)| first.ms_per_tick(*second, max_gap))
    }

    /// The latest observation.
    #[must_use]
    pub fn latest(&self) -> Option<Observation> {
        self.items.back().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_GAP, Observation, Observations, estimate, median};

    /// The observations of `(tick, at)` pairs, in a window that keeps them all.
    fn seen(pairs: &[(u64, f64)]) -> Observations {
        let mut observations = Observations::new(pairs.len());
        for &(tick, at) in pairs {
            observations.push(tick, at);
        }
        observations
    }

    /// The intervals of `pairs` with `max_gap`.
    fn intervals(pairs: &[(u64, f64)], max_gap: Option<u64>) -> Vec<f64> {
        seen(pairs).intervals(max_gap).collect()
    }

    /// The estimate of `pairs` with live updates' gap limit.
    fn live_estimate(pairs: &[(u64, f64)]) -> Option<f64> {
        estimate(intervals(pairs, Some(MAX_GAP)))
    }

    /// The tick of `tick` seen at `at`.
    const fn at(tick: u64, at: f64) -> Observation {
        Observation { tick, at }
    }

    #[test]
    fn steady_ticks_give_their_interval() {
        let steady = [(100, 0.0), (101, 3000.0), (102, 6000.0), (103, 9000.0)];
        assert_eq!(live_estimate(&steady), Some(3000.0));
        assert_eq!(intervals(&steady, None).len(), 3);
    }

    #[test]
    fn skipped_ticks_divide_the_interval() {
        // Two ticks apart: half the interval each.
        assert_eq!(
            at(10, 0.0).ms_per_tick(at(12, 1000.0), Some(MAX_GAP)),
            Some(500.0)
        );
    }

    #[test]
    fn a_gap_counts_only_without_a_limit() {
        // Ticks 103 to 108 were not seen: 7 ticks in 21 s.
        let gap = [(100, 0.0), (101, 3000.0), (102, 6000.0), (109, 27_000.0)];
        assert_eq!(intervals(&gap, Some(MAX_GAP)), [3000.0, 3000.0]);
        assert_eq!(intervals(&gap, None), [3000.0, 3000.0, 3000.0]);
        // A lost connection: 28 ticks later.
        assert_eq!(
            at(12, 1000.0).ms_per_tick(at(40, 30_000.0), Some(MAX_GAP)),
            None
        );
        // Polls 15 s apart on a 1 s server: each pair spans 15 ticks.
        let polled = [(1000, 0.0), (1015, 15_000.0), (1030, 30_000.0)];
        assert_eq!(estimate(intervals(&polled, None)), Some(1000.0));
        assert_eq!(live_estimate(&polled), None);
    }

    #[test]
    fn no_tick_change_or_no_time_gives_no_interval() {
        let first = at(12, 1000.0);
        assert_eq!(first.ms_per_tick(at(12, 1100.0), None), None);
        // A tick that goes back (another shard, a restarted server) gives none either.
        assert_eq!(first.ms_per_tick(at(11, 1200.0), None), None);
        assert_eq!(first.ms_per_tick(at(13, 1000.0), None), None);
        assert_eq!(first.ms_per_tick(at(13, 900.0), None), None);
        assert_eq!(median(Vec::new()), None);
    }

    #[test]
    fn one_interval_is_too_few() {
        assert_eq!(live_estimate(&[(10, 0.0), (12, 1000.0)]), None);
        assert_eq!(
            live_estimate(&[(10, 0.0), (12, 1000.0), (13, 1500.0)]),
            Some(500.0)
        );
        assert_eq!(estimate([]), None);
    }

    #[test]
    fn jitter_moves_the_median_little() {
        // 3 s ticks with late and bunched updates.
        let jitter = [
            (100, 0.0),
            (101, 3100.0),
            (102, 5900.0),
            (103, 9000.0),
            (104, 14_000.0),
            (105, 14_050.0),
            (106, 18_000.0),
        ];
        // Intervals 3100, 2800, 3100, 5000, 50, 3950: the middle two are 3100 and 3100.
        assert_eq!(live_estimate(&jitter), Some(3100.0));
        // About three seconds a tick, with jitter and one late snapshot.
        let late = [
            (100, 0.0),
            (101, 3100.0),
            (102, 6000.0),
            (103, 9050.0),
            (104, 15_000.0),
            (105, 18_000.0),
        ];
        let tick = live_estimate(&late).unwrap();
        assert!((2900.0..=3100.0).contains(&tick), "{tick}");
        assert_eq!(median(vec![1.0, 9.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![1.0, 2.0, 4.0, 9.0]), Some(3.0));
    }

    #[test]
    fn a_tick_seen_again_keeps_its_first_arrival() {
        // Two rooms report each tick.
        let mut observations = seen(&[
            (10, 0.0),
            (10, 40.0),
            (11, 2000.0),
            (11, 2050.0),
            (12, 4000.0),
        ]);
        assert_eq!(
            estimate(observations.intervals(Some(MAX_GAP))),
            Some(2000.0)
        );
        assert_eq!(observations.latest(), Some(at(12, 4000.0)));
        // A restart starts over: one interval is too few.
        observations.push(5, 5000.0);
        observations.push(6, 7000.0);
        assert_eq!(estimate(observations.intervals(Some(MAX_GAP))), None);
    }

    #[test]
    fn the_window_keeps_the_latest_observations() {
        let window = 16;
        let mut observations = Observations::new(window);
        let ticks = u64::try_from(window).unwrap() + 4;
        // 1 s per tick at first, then 2 s per tick for a whole window.
        for tick in 0..4 {
            observations.push(tick, f64::from(u32::try_from(tick).unwrap()) * 1000.0);
        }
        for tick in 4..ticks {
            let later = f64::from(u32::try_from(tick - 4).unwrap());
            observations.push(tick, 4000.0 + later * 2000.0);
        }
        assert_eq!(
            estimate(observations.intervals(Some(MAX_GAP))),
            Some(2000.0)
        );
    }
}
