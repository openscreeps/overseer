//! Timing of tick-to-tick motion: the server's tick duration, measured from snapshot
//! arrival times, the eased glide of an object over most of a tick, and its turn towards a
//! new heading.

use glam::Vec2;
use overseer::map::RoomCoord;

use crate::{MAX_TICK_MS, MIN_TICK_MS, MOVE_DURATION_MS};

/// Tick duration assumed before two snapshots of a room were measured, in milliseconds.
pub const DEFAULT_TICK_MS: f64 = 1000.0;
/// Share of a tick an object glides for: it arrives just before the next tick.
pub const GLIDE_SHARE: f64 = 0.82;
/// How long an object turns to a new heading, in milliseconds.
pub const TURN_MS: f64 = 200.0;
/// The heading of objects that have not moved or aimed yet: south, towards the viewer of
/// the top-down view.
pub const DEFAULT_HEADING: f32 = 0.5;
/// Intervals kept for the estimate.
const SAMPLES: usize = 9;
/// Snapshots further apart (in ticks) say nothing about the tick rate (a lost connection).
const MAX_TICK_GAP: u64 = 5;

/// The tick duration of the server, estimated from snapshot arrival times: the median of
/// the latest [`SAMPLES`] intervals between two snapshots of a room (divided by the ticks
/// between them), within [`MIN_TICK_MS`]..=[`MAX_TICK_MS`].
#[derive(Clone, Debug, Default)]
pub struct TickClock {
    /// Ring of measured tick durations (ms).
    samples: [f64; SAMPLES],
    len: usize,
    next: usize,
    /// Latest snapshot per room: room, game time, arrival (ms).
    last: Vec<(RoomCoord, u64, f64)>,
}

impl TickClock {
    /// Records that a snapshot of `room` at game tick `time` arrived at `now` (ms).
    pub fn observe(&mut self, room: RoomCoord, time: Option<u64>, now: f64) {
        let Some(time) = time else {
            return;
        };
        let previous = self.last.iter_mut().find(|(seen, _, _)| *seen == room);
        let Some((_, last_time, last_arrival)) = previous else {
            // Rooms of the detail slots: a handful at most.
            if self.last.len() >= 8 {
                self.last.remove(0);
            }
            self.last.push((room, time, now));
            return;
        };
        if time <= *last_time {
            return;
        }
        let ticks = time - *last_time;
        let interval = now - *last_arrival;
        (*last_time, *last_arrival) = (time, now);
        if ticks > MAX_TICK_GAP || interval <= 0.0 {
            return;
        }
        #[expect(clippy::cast_precision_loss, reason = "a few ticks")]
        let sample = interval / ticks as f64;
        self.samples[self.next] = sample;
        self.next = (self.next + 1) % SAMPLES;
        self.len = (self.len + 1).min(SAMPLES);
    }

    /// Forgets a room's latest snapshot (its next one starts a new interval).
    pub fn forget(&mut self, room: RoomCoord) {
        self.last.retain(|(seen, _, _)| *seen != room);
    }

    /// The estimated tick duration (ms).
    pub fn tick_ms(&self) -> f64 {
        if self.len == 0 {
            return DEFAULT_TICK_MS;
        }
        let mut sorted = self.samples;
        let sorted = &mut sorted[..self.len];
        sorted.sort_unstable_by(f64::total_cmp);
        let middle = self.len / 2;
        let median = if self.len % 2 == 1 {
            sorted[middle]
        } else {
            f64::midpoint(sorted[middle - 1], sorted[middle])
        };
        median.clamp(MIN_TICK_MS, MAX_TICK_MS)
    }

    /// How long an object glides to its new tile: [`GLIDE_SHARE`] of a tick, at least
    /// [`MOVE_DURATION_MS`].
    pub fn glide_ms(&self) -> f64 {
        (self.tick_ms() * GLIDE_SHARE).max(MOVE_DURATION_MS)
    }
}

/// Smoothstep easing of a progress in `0..=1`: starts and ends at rest.
pub fn ease(progress: f32) -> f32 {
    let t = progress.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Elapsed fraction of a span of `duration` ms started at `start`, from 0 to 1.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction in 0..=1 fits f32"
)]
pub fn progress(now: f64, start: f64, duration: f64) -> f32 {
    if duration <= 0.0 {
        return 1.0;
    }
    ((now - start) / duration).clamp(0.0, 1.0) as f32
}

/// The heading from `from` to `to` (tile coordinates): a turn fraction `0..1` clockwise
/// from north; `None` when they are the same point.
pub fn heading(from: Vec2, to: Vec2) -> Option<f32> {
    let delta = to - from;
    if delta.length_squared() < 1e-6 {
        return None;
    }
    Some((delta.x.atan2(-delta.y) / std::f32::consts::TAU).rem_euclid(1.0))
}

/// The heading `t` (`0..=1`) of the way from heading `from` to heading `to`, turning the
/// short way round.
pub fn turn(from: f32, to: f32, t: f32) -> f32 {
    let delta = (to - from + 0.5).rem_euclid(1.0) - 0.5;
    (from + delta * t).rem_euclid(1.0)
}
