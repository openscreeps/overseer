//! The duration of a game tick on the shard on the map, from two sources.
//!
//! - The server: [`overseer::Map::tick_duration`] reports the duration it measures
//!   ([`TickSpeed::report`]). It is fresh for [`SERVER_FRESH_MS`] after the reply.
//! - The game time: each game tick seen with its arrival time ([`TickSpeed::observe_live`] for
//!   live rooms, [`TickSpeed::observe_polled`] for `GET /api/game/time`). [`ms_per_tick`]
//!   divides the time between two observations by the ticks between them, and [`median`] takes
//!   the middle of these intervals: a late or bunched update moves it little.
//!
//! [`TickSpeed::duration`] prefers the server's fresh value, else the measured one, and says
//! which ([`Source`]). Live updates arrive every tick, so two of them more than
//! [`LIVE_MAX_GAP`] ticks apart do not count (updates were lost, or the ticks of another shard
//! came in). The poll of the game time is slow ([`TIME_POLL_MS`]) and spans many ticks of a fast
//! server, so its pairs count at every gap. The two kinds of observation are never paired:
//! a live update arrives when a tick starts, a polled tick at any time in the tick.

use std::collections::VecDeque;

/// How often the server's tick duration is asked for, in milliseconds.
pub const SERVER_POLL_MS: u64 = 30_000;
/// How long the server's tick duration stays fresh after its reply, in milliseconds: three
/// polls (a failing poll or two keep it).
pub const SERVER_FRESH_MS: u64 = 3 * SERVER_POLL_MS;
/// How often the game time is asked for while no live update arrives and the server gives no
/// fresh duration, in milliseconds.
pub const TIME_POLL_MS: u64 = 15_000;
/// After this time without a live update no live update arrives, in milliseconds.
pub const LIVE_QUIET_MS: u64 = TIME_POLL_MS;
/// The most ticks between two live updates that still give an interval.
pub const LIVE_MAX_GAP: u64 = 5;
/// The observations each kind keeps: a short window that follows changes of the speed.
pub const WINDOW: usize = 16;
/// The intervals a measured duration needs: one alone can be a late update.
pub const MIN_INTERVALS: usize = 2;
/// The most characters of [`TickDuration::compact`] (`250 ms/tick`).
pub const COMPACT_CHARS: usize = 11;

/// A game tick seen at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Observation {
    /// The game tick.
    pub tick: u64,
    /// When it arrived, in milliseconds since the Unix epoch.
    pub at: u64,
}

/// The milliseconds per tick of each pair of consecutive `observations` (oldest first): the
/// time between the two divided by the ticks between them. Pairs with no tick change (or a
/// tick that goes back) and, with a `max_gap`, pairs more than `max_gap` ticks apart give no
/// interval.
pub fn ms_per_tick<'a>(
    observations: impl IntoIterator<Item = &'a Observation> + Clone,
    max_gap: Option<u64>,
) -> impl Iterator<Item = f64> {
    let later = observations.clone().into_iter().skip(1);
    observations
        .into_iter()
        .zip(later)
        .filter_map(move |(first, second)| {
            let ticks = second.tick.checked_sub(first.tick)?;
            if ticks == 0 || max_gap.is_some_and(|max| ticks > max) {
                return None;
            }
            let ticks = f64::from(u32::try_from(ticks).ok()?);
            let elapsed = f64::from(u32::try_from(second.at.saturating_sub(first.at)).ok()?);
            Some(elapsed / ticks)
        })
}

/// The middle of `values` (the mean of the two middle ones for an even count); `None` without
/// values.
pub fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        count if count % 2 == 1 => Some(values[middle]),
        _ => Some(f64::midpoint(values[middle - 1], values[middle])),
    }
}

/// The last [`WINDOW`] observations of one kind, oldest first, one per tick.
#[derive(Clone, Debug, Default, PartialEq)]
struct Observations(VecDeque<Observation>);

impl Observations {
    /// Adds tick `tick` seen at `at`. The same tick again keeps its first arrival (the start
    /// of the tick); a tick before the latest one starts over (the server restarted).
    fn push(&mut self, tick: u64, at: u64) {
        match self.0.back() {
            Some(latest) if latest.tick == tick => return,
            Some(latest) if latest.tick > tick => self.0.clear(),
            _ => {}
        }
        if self.0.len() == WINDOW {
            self.0.pop_front();
        }
        self.0.push_back(Observation { tick, at });
    }
}

/// Where a tick duration comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The server reported it.
    Server,
    /// The app measured it from the game time.
    Measured,
}

/// The duration of a tick and its source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickDuration {
    /// Milliseconds per tick.
    pub ms: f64,
    /// Where it comes from.
    pub source: Source,
}

impl TickDuration {
    /// Short, for the status bar, with its unit: `250 ms/tick` below a second, `3.1 s/tick`
    /// below 10 seconds, `12 s/tick` from there on; at most [`COMPACT_CHARS`] characters for
    /// ticks below 1000 s.
    pub fn compact(&self) -> String {
        compact(self.ms)
    }

    /// In a sentence: `4.00 s per tick (0.25 ticks per second), reported by the server` or
    /// `… measured from game time`.
    pub fn sentence(&self) -> String {
        let source = match self.source {
            Source::Server => "reported by the server",
            Source::Measured => "measured from game time",
        };
        format!(
            "{:.2} s per tick ({:.2} ticks per second), {source}",
            self.ms / 1000.0,
            1000.0 / self.ms
        )
    }
}

/// A tick duration of `ms` milliseconds, short, with its unit ([`TickDuration::compact`]).
pub fn compact(ms: f64) -> String {
    let seconds = ms / 1000.0;
    if ms < 999.5 {
        format!("{ms:.0} ms/tick")
    } else if seconds < 9.95 {
        format!("{seconds:.1} s/tick")
    } else {
        format!("{seconds:.0} s/tick")
    }
}

/// The server's tick duration and when it arrived.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reported {
    /// Milliseconds per tick.
    ms: f64,
    /// When the reply arrived, in milliseconds since the Unix epoch.
    at: u64,
}

/// What the app knows of the tick duration of one shard.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickSpeed {
    /// Ticks of live room updates.
    live: Observations,
    /// Ticks of `GET /api/game/time`.
    polled: Observations,
    /// When the latest live update arrived.
    live_at: Option<u64>,
    /// The server's latest known duration.
    server: Option<Reported>,
}

impl TickSpeed {
    /// A live room update at tick `tick` arrived at `at`.
    pub fn observe_live(&mut self, tick: u64, at: u64) {
        self.live.push(tick, at);
        self.live_at = Some(at);
    }

    /// `GET /api/game/time` said tick `tick` at `at`.
    pub fn observe_polled(&mut self, tick: u64, at: u64) {
        self.polled.push(tick, at);
    }

    /// The server reported the duration `ms` at `at` (`None`: it does not know it).
    pub fn report(&mut self, ms: Option<f64>, at: u64) {
        self.server = ms.map(|ms| Reported { ms, at });
    }

    /// The duration measured from the game time: the median of the live and polled intervals,
    /// once there are [`MIN_INTERVALS`].
    pub fn measured(&self) -> Option<f64> {
        let intervals: Vec<f64> = ms_per_tick(&self.live.0, Some(LIVE_MAX_GAP))
            .chain(ms_per_tick(&self.polled.0, None))
            .collect();
        if intervals.len() < MIN_INTERVALS {
            return None;
        }
        median(intervals)
    }

    /// The server's duration while fresh at `now`.
    fn fresh_server(&self, now: u64) -> Option<f64> {
        self.server
            .filter(|server| now.saturating_sub(server.at) < SERVER_FRESH_MS)
            .map(|server| server.ms)
    }

    /// The tick duration at `now`: the server's while fresh, else the measured one.
    pub fn duration(&self, now: u64) -> Option<TickDuration> {
        self.fresh_server(now)
            .map(|ms| TickDuration {
                ms,
                source: Source::Server,
            })
            .or_else(|| {
                self.measured().map(|ms| TickDuration {
                    ms,
                    source: Source::Measured,
                })
            })
    }

    /// Whether to ask for the game time at `now`: the server gives no fresh duration and no
    /// live update arrived for [`LIVE_QUIET_MS`].
    pub fn wants_game_time(&self, now: u64) -> bool {
        self.fresh_server(now).is_none()
            && self
                .live_at
                .is_none_or(|at| now.saturating_sub(at) >= LIVE_QUIET_MS)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        COMPACT_CHARS, LIVE_MAX_GAP, LIVE_QUIET_MS, Observation, SERVER_FRESH_MS, Source,
        TickDuration, TickSpeed, WINDOW, compact, median, ms_per_tick,
    };

    /// Observations of `(tick, at)`.
    fn seen(pairs: &[(u64, u64)]) -> Vec<Observation> {
        pairs
            .iter()
            .map(|&(tick, at)| Observation { tick, at })
            .collect()
    }

    /// The median interval of `pairs` with live updates' gap limit.
    fn live_median(pairs: &[(u64, u64)]) -> Option<f64> {
        median(ms_per_tick(&seen(pairs), Some(LIVE_MAX_GAP)).collect())
    }

    #[test]
    fn steady_ticks_give_their_interval() {
        let steady = [(100, 0), (101, 3000), (102, 6000), (103, 9000)];
        assert_eq!(live_median(&steady), Some(3000.0));
        assert_eq!(ms_per_tick(&seen(&steady), None).count(), 3);
    }

    #[test]
    fn a_gap_counts_only_without_a_limit() {
        // Ticks 103 to 109 were not seen: 7 ticks in 21 s.
        let gap = [(100, 0), (101, 3000), (102, 6000), (109, 27_000)];
        assert_eq!(
            ms_per_tick(&seen(&gap), Some(LIVE_MAX_GAP)).collect::<Vec<_>>(),
            [3000.0, 3000.0]
        );
        // Polls 15 s apart on a 1 s server: each pair spans 15 ticks.
        let polled = [(1000, 0), (1015, 15_000), (1030, 30_000)];
        assert_eq!(
            median(ms_per_tick(&seen(&polled), None).collect()),
            Some(1000.0)
        );
        assert_eq!(live_median(&polled), None);
    }

    #[test]
    fn no_tick_change_gives_no_interval() {
        let paused = [(100, 0), (100, 3000), (100, 6000)];
        assert_eq!(live_median(&paused), None);
        // A tick that goes back (another shard, a restarted server) gives none either.
        assert_eq!(live_median(&[(100, 0), (90, 3000)]), None);
        assert_eq!(median(Vec::new()), None);
    }

    #[test]
    fn jitter_moves_the_median_little() {
        // 3 s ticks with late and bunched updates.
        let jitter = [
            (100, 0),
            (101, 3100),
            (102, 5900),
            (103, 9000),
            (104, 14_000),
            (105, 14_050),
            (106, 18_000),
        ];
        // Intervals 3100, 2800, 3100, 5000, 50, 3950: the middle two are 3100 and 3100.
        assert_eq!(live_median(&jitter), Some(3100.0));
        assert_eq!(median(vec![1.0, 9.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![1.0, 2.0, 4.0, 9.0]), Some(3.0));
    }

    #[test]
    fn a_tick_seen_again_keeps_its_first_arrival() {
        let mut speed = TickSpeed::default();
        // Two rooms report each tick.
        for (tick, at) in [(10, 0), (10, 40), (11, 2000), (11, 2050), (12, 4000)] {
            speed.observe_live(tick, at);
        }
        assert_eq!(speed.measured(), Some(2000.0));
        // A restart starts over: one interval is too few.
        speed.observe_live(5, 5000);
        speed.observe_live(6, 7000);
        assert_eq!(speed.measured(), None);
    }

    #[test]
    fn the_window_keeps_the_latest_observations() {
        let mut speed = TickSpeed::default();
        let ticks = u64::try_from(WINDOW).unwrap() + 4;
        // 1 s per tick at first, then 2 s per tick for a whole window.
        for tick in 0..4 {
            speed.observe_live(tick, tick * 1000);
        }
        for tick in 4..ticks {
            speed.observe_live(tick, 4000 + (tick - 4) * 2000);
        }
        assert_eq!(speed.measured(), Some(2000.0));
    }

    #[test]
    fn live_and_polled_ticks_are_not_paired() {
        let mut speed = TickSpeed::default();
        speed.observe_polled(100, 0);
        speed.observe_live(101, 500);
        speed.observe_polled(105, 20_000);
        speed.observe_live(102, 3500);
        // Live: 3 s per tick; polled: 20 s for 5 ticks.
        assert_eq!(speed.measured(), Some(3500.0));
    }

    #[test]
    fn a_fresh_server_value_wins_over_the_measured_one() {
        let mut speed = TickSpeed::default();
        assert_eq!(speed.duration(0), None);
        for (tick, at) in [(1, 0), (2, 3000), (3, 6000)] {
            speed.observe_live(tick, at);
        }
        let measured = Some(TickDuration {
            ms: 3000.0,
            source: Source::Measured,
        });
        assert_eq!(speed.duration(6000), measured);
        speed.report(Some(3150.5), 7000);
        assert_eq!(
            speed.duration(8000),
            Some(TickDuration {
                ms: 3150.5,
                source: Source::Server,
            })
        );
        // A stale server value gives way to the measured one.
        assert_eq!(speed.duration(7000 + SERVER_FRESH_MS), measured);
        // So does a server that does not know it.
        speed.report(None, 9000);
        assert_eq!(speed.duration(9000), measured);
    }

    #[test]
    fn the_game_time_is_asked_for_only_without_other_sources() {
        let mut speed = TickSpeed::default();
        assert!(speed.wants_game_time(0));
        speed.observe_live(1, 1000);
        assert!(!speed.wants_game_time(1000 + LIVE_QUIET_MS - 1));
        assert!(speed.wants_game_time(1000 + LIVE_QUIET_MS));
        speed.report(Some(3000.0), 20_000);
        assert!(!speed.wants_game_time(20_000));
        assert!(speed.wants_game_time(20_000 + SERVER_FRESH_MS));
        speed.report(None, 30_000);
        assert!(speed.wants_game_time(30_000));
    }

    #[test]
    fn durations_read_with_their_unit() {
        assert_eq!(compact(250.0), "250 ms/tick");
        assert_eq!(compact(999.0), "999 ms/tick");
        assert_eq!(compact(1000.0), "1.0 s/tick");
        assert_eq!(compact(3104.0), "3.1 s/tick");
        assert_eq!(compact(9960.0), "10 s/tick");
        assert_eq!(compact(123_400.0), "123 s/tick");
        for ms in [250.0, 999.0, 3104.0, 123_400.0] {
            assert!(compact(ms).chars().count() <= COMPACT_CHARS, "{ms}");
        }
        let server = TickDuration {
            ms: 4000.0,
            source: Source::Server,
        };
        assert_eq!(server.compact(), "4.0 s/tick");
        assert_eq!(
            server.sentence(),
            "4.00 s per tick (0.25 ticks per second), reported by the server"
        );
        let measured = TickDuration {
            ms: 250.0,
            source: Source::Measured,
        };
        assert_eq!(
            measured.sentence(),
            "0.25 s per tick (4.00 ticks per second), measured from game time"
        );
    }
}
