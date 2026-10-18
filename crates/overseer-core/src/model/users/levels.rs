//! Global Control Level and power level from an account's lifetime points, with the game
//! constants and formulas of the engine (`Game.gcl` and `Game.gpl` in
//! `engine/src/game/game.js`):
//!
//! - GCL: `level = floor((gcl / GCL_MULTIPLY) ^ (1 / GCL_POW)) + 1`, level `L` starts at
//!   `(L - 1) ^ GCL_POW * GCL_MULTIPLY` points.
//! - Power: `level = floor((power / POWER_LEVEL_MULTIPLY) ^ (1 / POWER_LEVEL_POW))`, level `L`
//!   starts at `L ^ POWER_LEVEL_POW * POWER_LEVEL_MULTIPLY` points.
//!
//! The floating-point root can land just below an exact level boundary (for example
//! `4096 ^ (1 / 2.4)`), so the estimate is corrected against the boundaries themselves.

/// `GCL_POW` of the game constants.
pub const GCL_POW: f64 = 2.4;
/// `GCL_MULTIPLY` of the game constants.
pub const GCL_MULTIPLY: f64 = 1_000_000.0;
/// `POWER_LEVEL_POW` of the game constants.
pub const POWER_LEVEL_POW: f64 = 2.0;
/// `POWER_LEVEL_MULTIPLY` of the game constants.
pub const POWER_LEVEL_MULTIPLY: f64 = 1000.0;

/// A level and the progress towards the next one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LevelProgress {
    /// The level.
    pub level: u32,
    /// Points gained since the level was reached.
    pub progress: f64,
    /// Points between this level and the next.
    pub total: f64,
}

impl LevelProgress {
    /// The Global Control Level for `points` lifetime control points (`gcl` of a profile).
    /// Negative or non-finite points count as 0: level 1.
    #[must_use]
    pub fn gcl(points: f64) -> Self {
        // Level `L` (from 1) starts at `(L - 1) ^ GCL_POW * GCL_MULTIPLY`.
        level_of(
            points,
            1,
            |points| (points / GCL_MULTIPLY).powf(GCL_POW.recip()).floor() + 1.0,
            |level| f64::from(level - 1).powf(GCL_POW) * GCL_MULTIPLY,
        )
    }

    /// The power level for `points` lifetime processed power (`power` of a profile).
    /// Negative or non-finite points count as 0: level 0.
    #[must_use]
    pub fn power(points: f64) -> Self {
        // Level `L` (from 0) starts at `L ^ POWER_LEVEL_POW * POWER_LEVEL_MULTIPLY`.
        level_of(
            points,
            0,
            |points| {
                (points / POWER_LEVEL_MULTIPLY)
                    .powf(POWER_LEVEL_POW.recip())
                    .floor()
            },
            |level| f64::from(level).powf(POWER_LEVEL_POW) * POWER_LEVEL_MULTIPLY,
        )
    }

    /// Progress towards the next level, `0.0..=1.0`.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        if self.total > 0.0 {
            (self.progress / self.total).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// The level whose range holds `points` (negative or non-finite count as 0): the engine's
/// `estimate` corrected so that `start(level) <= points < start(level + 1)`, never below
/// `lowest`.
fn level_of(
    points: f64,
    lowest: u32,
    estimate: impl Fn(f64) -> f64,
    start: impl Fn(u32) -> f64,
) -> LevelProgress {
    let points = if points.is_finite() {
        points.max(0.0)
    } else {
        0.0
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the estimate is a small non-negative whole number for finite points"
    )]
    let mut level = (estimate(points) as u32).max(lowest);
    while level > lowest && start(level) > points {
        level -= 1;
    }
    while start(level + 1) <= points {
        level += 1;
    }
    let base = start(level);
    LevelProgress {
        level,
        progress: points - base,
        total: start(level + 1) - base,
    }
}

#[cfg(test)]
mod tests {
    use super::LevelProgress;

    fn gcl(points: f64) -> LevelProgress {
        LevelProgress::gcl(points)
    }

    fn power(points: f64) -> LevelProgress {
        LevelProgress::power(points)
    }

    #[test]
    fn gcl_starts_at_level_one() {
        let level = gcl(0.0);
        assert_eq!(level.level, 1);
        assert!((level.total - 1_000_000.0).abs() < 1e-6);
        assert_eq!(gcl(-5.0).level, 1);
        assert_eq!(gcl(f64::NAN).level, 1);
    }

    #[test]
    fn gcl_changes_exactly_at_the_boundaries() {
        assert_eq!(gcl(999_999.0).level, 1);
        assert_eq!(gcl(1_000_000.0).level, 2);
        // Level 33 starts at 32 ^ 2.4 * 10^6 = 2 ^ 12 * 10^6 points exactly.
        assert_eq!(gcl(4_095_999_999.0).level, 32);
        let level = gcl(4_096_000_000.0);
        assert_eq!(level.level, 33);
        assert!(level.progress.abs() < 1e-3);
        // Level 3 starts at 2 ^ 2.4 * 10^6 = 5_278_031.6… points.
        assert_eq!(gcl(5_278_031.0).level, 2);
        assert_eq!(gcl(5_278_032.0).level, 3);
    }

    #[test]
    fn gcl_progress_counts_from_the_level_start() {
        let level = gcl(1_500_000.0);
        assert_eq!(level.level, 2);
        assert!((level.progress - 500_000.0).abs() < 1e-6);
        assert!((level.total - (2f64.powf(2.4) * 1e6 - 1e6)).abs() < 1e-3);
        assert!(level.fraction() > 0.11 && level.fraction() < 0.12);
    }

    #[test]
    fn power_levels_start_at_zero() {
        assert_eq!(power(0.0).level, 0);
        assert_eq!(power(999.0).level, 0);
        let level = power(1000.0);
        assert_eq!(level.level, 1);
        assert!(level.progress.abs() < 1e-9);
        assert!((level.total - 3000.0).abs() < 1e-9);
        assert_eq!(power(3999.0).level, 1);
        assert_eq!(power(4000.0).level, 2);
        // Level 100 starts at 100^2 * 1000 points.
        assert_eq!(power(9_999_999.0).level, 99);
        assert_eq!(power(10_000_000.0).level, 100);
    }
}
