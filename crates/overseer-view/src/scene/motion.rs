//! Timing of tick-to-tick motion: the server's tick duration, measured from snapshot
//! arrival times, the eased glide of an object over most of a tick, and its turn towards a
//! new heading.

use std::collections::VecDeque;

use glam::Vec2;
use overseer::map::RoomCoord;
use overseer::model::world::tick::{self, MAX_GAP, Observation};

use crate::{MAX_TICK_MS, MIN_TICK_MS, MOVE_DURATION_MS};

/// Tick duration assumed before the snapshots of the rooms were measured, in milliseconds.
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
/// Rooms whose latest snapshot is kept: the rooms of the detail slots, a handful at most.
const ROOMS: usize = 8;

/// The tick duration of the server, estimated from snapshot arrival times: the estimate
/// ([`tick::estimate`]) of the latest [`SAMPLES`] intervals between two snapshots of a room
/// (divided by the ticks between them, at most [`MAX_GAP`] ticks apart), within
/// [`MIN_TICK_MS`]..=[`MAX_TICK_MS`]; [`DEFAULT_TICK_MS`] before there is an estimate.
#[derive(Clone, Debug, Default)]
pub struct TickClock {
    /// The latest measured tick durations (ms), oldest first.
    samples: VecDeque<f64>,
    /// The estimate of `samples`, updated with them.
    estimate: Option<f64>,
    /// Latest snapshot per room: its game time and arrival (ms).
    last: Vec<(RoomCoord, Observation)>,
}

impl TickClock {
    /// Records that a snapshot of `room` at game tick `time` arrived at `now` (ms). The same
    /// tick again and an earlier one say nothing.
    pub fn observe(&mut self, room: RoomCoord, time: Option<u64>, now: f64) {
        let Some(time) = time else {
            return;
        };
        let seen = Observation {
            tick: time,
            at: now,
        };
        let previous = self.last.iter_mut().find(|(known, _)| *known == room);
        let Some((_, last)) = previous else {
            if self.last.len() >= ROOMS {
                self.last.remove(0);
            }
            self.last.push((room, seen));
            return;
        };
        if time <= last.tick {
            return;
        }
        let sample = last.ms_per_tick(seen, Some(MAX_GAP));
        *last = seen;
        if let Some(sample) = sample {
            if self.samples.len() == SAMPLES {
                self.samples.pop_front();
            }
            self.samples.push_back(sample);
            self.estimate = tick::estimate(self.samples.iter().copied());
        }
    }

    /// Forgets a room's latest snapshot (its next one starts a new interval).
    pub fn forget(&mut self, room: RoomCoord) {
        self.last.retain(|(known, _)| *known != room);
    }

    /// The estimated tick duration (ms).
    pub fn tick_ms(&self) -> f64 {
        self.estimate
            .map_or(DEFAULT_TICK_MS, |ms| ms.clamp(MIN_TICK_MS, MAX_TICK_MS))
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
