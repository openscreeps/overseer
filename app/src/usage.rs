//! How the CPU and Memory usage of the signed-in account's code reads: how near the limits it
//! comes ([`Level`]), bars and sparklines, and its texts.
//!
//! The session keeps the usage ([`overseer::Account::usage`]): the recent runs, their average
//! and peak, the CPU limit of a run and whether the usage is idle. Memory has a fixed capacity
//! ([`MEMORY_CAPACITY`]).

use overseer::account::usage::{IDLE_AFTER_MS, RUNS_KEPT, Run, RunCpu};
use overseer::model::memory::MEMORY_CAPACITY;

/// From this share of a limit on, a value is near the limit ([`Level::Warn`]).
const WARN_SHARE: f64 = 0.8;

/// How near a limit a value is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Below 80 % of the limit, or no limit known.
    Ok,
    /// From 80 % of the limit on.
    Warn,
    /// At the limit or above it, or the CPU limit was hit.
    Error,
}

impl Level {
    /// The level of `value` against `limit` (no level above [`Level::Ok`] without a positive
    /// limit).
    pub fn of(value: f64, limit: Option<f64>) -> Self {
        match share(value, limit) {
            Some(share) if share >= 1.0 => Self::Error,
            Some(share) if share >= WARN_SHARE => Self::Warn,
            _ => Self::Ok,
        }
    }

    /// The level of a run's `cpu` against the CPU `limit`.
    pub fn of_cpu(cpu: RunCpu, limit: Option<f64>) -> Self {
        match cpu {
            RunCpu::Used(ms) => Self::of(ms, limit),
            RunCpu::LimitHit => Self::Error,
        }
    }
}

/// `value` as a share of `limit` (1.0 at the limit), without a positive limit `None`.
pub fn share(value: f64, limit: Option<f64>) -> Option<f64> {
    limit
        .filter(|limit| *limit > 0.0)
        .map(|limit| value / limit)
}

/// `value` as a whole percentage of `limit`.
pub fn percent(value: f64, limit: Option<f64>) -> Option<u32> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a clamped, rounded, non-negative percentage fits in u32"
    )]
    let percent = share(value, limit).map(|share| (share * 100.0).round().clamp(0.0, 1e6) as u32);
    percent
}

/// The width of a bar of `value` against `limit`, in percent: 0 to 100.
pub fn bar_width(value: f64, limit: Option<f64>) -> f64 {
    share(value, limit).map_or(0.0, |share| (share * 100.0).clamp(0.0, 100.0))
}

/// `bytes` in kibibytes (`KB`, as the official client says).
#[expect(
    clippy::cast_precision_loss,
    reason = "Memory sizes are far below 2^52"
)]
pub fn kilobytes(bytes: u64) -> f64 {
    bytes as f64 / 1024.0
}

/// The value at the top edge of a sparkline of values up to `peak` against `limit`, a quarter
/// above the larger of both, so that the limit line shows below the edge. While the peak stays
/// below half the limit, the scale follows the peak alone (the limit line is then off the
/// scale): small values still show their changes.
pub fn sparkline_top(limit: Option<f64>, peak: Option<f64>) -> f64 {
    let peak = peak.filter(|peak| *peak > 0.0);
    let top = match (limit.filter(|limit| *limit > 0.0), peak) {
        (Some(limit), Some(peak)) if peak < limit / 2.0 => peak,
        (Some(limit), peak) => limit.max(peak.unwrap_or(0.0)),
        (None, Some(peak)) => peak,
        (None, None) => return 1.0,
    };
    top * 1.25
}

/// The x of value `index` of `count` values in a sparkline `width` wide: the latest value at
/// the right edge, one step of `width / (RUNS_KEPT - 1)` for each value before it.
#[expect(
    clippy::cast_precision_loss,
    reason = "RUNS_KEPT and the indices are small"
)]
pub fn sparkline_x(index: usize, count: usize, width: f64) -> f64 {
    width - count.saturating_sub(index + 1) as f64 * width / (RUNS_KEPT - 1) as f64
}

/// The y of `value` in a sparkline `height` high with `top` at its top edge; a value above
/// `top` stays at the top edge.
pub fn sparkline_y(value: f64, top: f64, height: f64) -> f64 {
    height - height * share(value, Some(top)).unwrap_or(0.0).clamp(0.0, 1.0)
}

/// The points of a sparkline of `values` (oldest first, at most [`RUNS_KEPT`]) in a box
/// `width` × `height` with `top` at its top edge ([`sparkline_x`], [`sparkline_y`]), as the
/// `points` of an SVG `polyline`.
pub fn sparkline(values: &[f64], top: f64, width: f64, height: f64) -> String {
    let count = values.len();
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let x = sparkline_x(index, count, width);
            let y = sparkline_y(*value, top, height);
            format!("{x:.1},{y:.1}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// [`MEMORY_CAPACITY`] as a float, in bytes.
#[expect(clippy::cast_precision_loss, reason = "2 MiB is exact")]
const MEMORY_CAPACITY_F64: f64 = MEMORY_CAPACITY as f64;

/// `bytes` as a share of the Memory limit.
#[expect(
    clippy::cast_precision_loss,
    reason = "Memory sizes are far below 2^52"
)]
pub fn memory_level(bytes: u64) -> Level {
    Level::of(bytes as f64, Some(MEMORY_CAPACITY_F64))
}

/// The width of the Memory bar of `bytes`, in percent.
#[expect(
    clippy::cast_precision_loss,
    reason = "Memory sizes are far below 2^52"
)]
pub fn memory_bar(bytes: u64) -> f64 {
    bar_width(bytes as f64, Some(MEMORY_CAPACITY_F64))
}

/// An amount of CPU: a whole number without decimals (`20`), else with one (`12.5`).
pub fn cpu_amount(cpu: f64) -> String {
    if cpu.fract().abs() < f64::EPSILON {
        format!("{cpu:.0}")
    } else {
        format!("{cpu:.1}")
    }
}

/// The CPU of a run, wide: `4.2 / 20` against `limit`, `4.2 ms` without one, or `limit hit`.
pub fn cpu_long(cpu: RunCpu, limit: Option<f64>) -> String {
    match (cpu, limit) {
        (RunCpu::Used(ms), Some(limit)) => format!("{ms:.1} / {}", cpu_amount(limit)),
        (RunCpu::Used(ms), None) => format!("{ms:.1} ms"),
        (RunCpu::LimitHit, _) => "limit hit".to_owned(),
    }
}

/// The CPU of a run, short: `21%` of `limit`, `4 ms` without one, or `limit`.
pub fn cpu_short(cpu: RunCpu, limit: Option<f64>) -> String {
    match cpu {
        RunCpu::Used(ms) => percent(ms, limit)
            .map_or_else(|| format!("{ms:.0} ms"), |percent| format!("{percent}%")),
        RunCpu::LimitHit => "limit".to_owned(),
    }
}

/// A Memory size of `bytes`: `38 B` below 1 KB, else `38.1 KB`.
pub fn memory_amount(bytes: f64) -> String {
    if bytes < 1024.0 {
        format!("{bytes:.0} B")
    } else {
        format!("{:.1} KB", bytes / 1024.0)
    }
}

/// A Memory size, wide: `38.1 KB / 2048 KB`.
#[expect(
    clippy::cast_precision_loss,
    reason = "Memory sizes are far below 2^52"
)]
pub fn memory_long(bytes: u64) -> String {
    format!(
        "{} / {} KB",
        memory_amount(bytes as f64),
        cpu_amount(kilobytes(MEMORY_CAPACITY))
    )
}

/// A Memory size, short: `2%` of the limit.
#[expect(
    clippy::cast_precision_loss,
    reason = "Memory sizes are far below 2^52"
)]
pub fn memory_short(bytes: u64) -> String {
    format!(
        "{}%",
        percent(bytes as f64, Some(MEMORY_CAPACITY_F64)).unwrap_or(0)
    )
}

/// What the usage says while idle ([`IDLE_AFTER_MS`]): `No run in the last 10 s. `
pub fn idle_text() -> String {
    format!("No run in the last {} s. ", IDLE_AFTER_MS / 1000)
}

/// A run in one line: `4.21 ms, Memory 38.1 KB`, or `limit exceeded, Memory 38.1 KB`.
pub fn run_text(run: &Run) -> String {
    let cpu = match run.cpu {
        RunCpu::Used(ms) => format!("{ms:.2} ms"),
        RunCpu::LimitHit => "limit exceeded".to_owned(),
    };
    match run.memory {
        Some(bytes) => format!("{cpu}, Memory {:.1} KB", kilobytes(bytes)),
        None => cpu,
    }
}

/// The CPU of a run against `limit` in a sentence: `4.21 ms of 20 CPU (21 %)`.
pub fn cpu_sentence(cpu: RunCpu, limit: Option<f64>) -> String {
    match cpu {
        RunCpu::Used(ms) => match (limit, percent(ms, limit)) {
            (Some(limit), Some(percent)) => {
                format!("{ms:.2} ms of {} CPU ({percent} %)", cpu_amount(limit))
            }
            _ => format!("{ms:.2} ms"),
        },
        RunCpu::LimitHit => "the CPU limit was hit".to_owned(),
    }
}

/// A Memory size against the limit in a sentence: `38.1 KB of 2048 KB (2 %)`.
#[expect(
    clippy::cast_precision_loss,
    reason = "Memory sizes are far below 2^52"
)]
pub fn memory_sentence(bytes: u64) -> String {
    format!(
        "{} of {} KB ({})",
        memory_amount(bytes as f64),
        cpu_amount(kilobytes(MEMORY_CAPACITY)),
        memory_short(bytes).replace('%', " %")
    )
}

/// What the status bar's usage button says on hover about the latest `run`, against the CPU
/// `limit`, idle or not.
pub fn summary(run: &Run, limit: Option<f64>, idle: bool) -> String {
    let memory = run
        .memory
        .map(|bytes| format!(", Memory {}", memory_sentence(bytes)))
        .unwrap_or_default();
    let idle = if idle { idle_text() } else { String::new() };
    format!(
        "{idle}Latest run: CPU {}{memory}. Show the usage.",
        cpu_sentence(run.cpu, limit)
    )
}

/// How long ago something happened, `elapsed` milliseconds before now: `3 s ago`,
/// `2 min ago`, `1 h ago`.
pub fn ago(elapsed: u64) -> String {
    let seconds = elapsed / 1000;
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3600 => format!("{} min ago", seconds / 60),
        _ => format!("{} h ago", seconds / 3600),
    }
}

#[cfg(test)]
mod tests {
    use overseer::account::usage::{Run, RunCpu};
    use overseer::model::memory::MEMORY_CAPACITY;

    use super::{
        Level, RUNS_KEPT, ago, bar_width, cpu_amount, cpu_long, cpu_short, kilobytes,
        memory_amount, memory_bar, memory_level, memory_long, memory_short, percent, run_text,
        sparkline, sparkline_top, sparkline_x, summary,
    };

    #[test]
    fn texts_say_the_usage_against_the_limits() {
        assert_eq!(cpu_amount(20.0), "20");
        assert_eq!(cpu_amount(12.5), "12.5");
        assert_eq!(cpu_long(RunCpu::Used(4.21), Some(20.0)), "4.2 / 20");
        assert_eq!(cpu_long(RunCpu::Used(4.21), None), "4.2 ms");
        assert_eq!(cpu_long(RunCpu::LimitHit, Some(20.0)), "limit hit");
        assert_eq!(cpu_short(RunCpu::Used(4.21), Some(20.0)), "21%");
        assert_eq!(cpu_short(RunCpu::Used(4.21), None), "4 ms");
        assert_eq!(cpu_short(RunCpu::LimitHit, None), "limit");
        assert_eq!(memory_long(39_014), "38.1 KB / 2048 KB");
        assert_eq!(memory_long(38), "38 B / 2048 KB");
        assert_eq!(memory_amount(1023.0), "1023 B");
        assert_eq!(memory_amount(1024.0), "1.0 KB");
        assert_eq!(memory_short(39_014), "2%");
        assert_eq!(memory_level(MEMORY_CAPACITY), Level::Error);
        assert_eq!(memory_level(39_014), Level::Ok);
        assert!((memory_bar(MEMORY_CAPACITY / 4) - 25.0).abs() < 1e-9);
        let latest = run(4.21, 39_014, 1);
        assert_eq!(run_text(&latest), "4.21 ms, Memory 38.1 KB");
        assert_eq!(
            run_text(&Run {
                cpu: RunCpu::LimitHit,
                memory: None,
                at: 1
            }),
            "limit exceeded"
        );
        assert_eq!(
            summary(&latest, Some(20.0), false),
            "Latest run: CPU 4.21 ms of 20 CPU (21 %), Memory 38.1 KB of 2048 KB (2 %). Show the usage."
        );
        assert_eq!(
            summary(&latest, None, true),
            "No run in the last 10 s. Latest run: CPU 4.21 ms, Memory 38.1 KB of 2048 KB (2 %). Show the usage."
        );
    }

    fn run(ms: f64, memory: u64, at: u64) -> Run {
        Run {
            cpu: RunCpu::Used(ms),
            memory: Some(memory),
            at,
        }
    }

    #[test]
    fn levels_turn_at_80_and_100_percent_and_on_a_limit_hit() {
        assert_eq!(Level::of(15.9, Some(20.0)), Level::Ok);
        assert_eq!(Level::of(16.0, Some(20.0)), Level::Warn);
        assert_eq!(Level::of(19.9, Some(20.0)), Level::Warn);
        assert_eq!(Level::of(20.0, Some(20.0)), Level::Error);
        assert_eq!(Level::of(50.0, None), Level::Ok);
        assert_eq!(Level::of(50.0, Some(0.0)), Level::Ok);
        assert_eq!(Level::of_cpu(RunCpu::LimitHit, None), Level::Error);
        assert_eq!(Level::of_cpu(RunCpu::Used(1.0), Some(20.0)), Level::Ok);
        #[expect(clippy::cast_precision_loss, reason = "2 MiB is exact")]
        let memory_limit = Some(MEMORY_CAPACITY as f64);
        assert_eq!(Level::of(1_800_000.0, memory_limit), Level::Warn);
    }

    #[test]
    fn percentages_and_bars_follow_the_limit() {
        assert_eq!(percent(4.2, Some(20.0)), Some(21));
        assert_eq!(percent(30.0, Some(20.0)), Some(150));
        assert_eq!(percent(1.0, None), None);
        assert!((bar_width(5.0, Some(20.0)) - 25.0).abs() < 1e-9);
        assert!((bar_width(30.0, Some(20.0)) - 100.0).abs() < 1e-9);
        assert!(bar_width(5.0, None).abs() < 1e-9);
        assert!((kilobytes(2048) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn a_sparkline_ends_at_the_right_edge_and_stays_inside_its_box() {
        assert_eq!(sparkline(&[], 10.0, 119.0, 20.0), "");
        // A limit hit counts as infinite: at the top edge.
        assert_eq!(
            sparkline(&[0.0, 5.0, f64::INFINITY], 10.0, 119.0, 20.0),
            "117.0,20.0 118.0,10.0 119.0,0.0"
        );
        assert!((sparkline_x(0, RUNS_KEPT, 119.0)).abs() < 1e-9);
        assert!((sparkline_x(4, 5, 119.0) - 119.0).abs() < 1e-9);
        // Near the limit: the limit line shows; far below it: the peak sets the scale.
        assert!((sparkline_top(Some(20.0), Some(12.0)) - 25.0).abs() < 1e-9);
        assert!((sparkline_top(Some(20.0), Some(24.0)) - 30.0).abs() < 1e-9);
        assert!((sparkline_top(Some(20.0), Some(4.0)) - 5.0).abs() < 1e-9);
        assert!((sparkline_top(Some(20.0), None) - 25.0).abs() < 1e-9);
        assert!((sparkline_top(None, Some(10.0)) - 12.5).abs() < 1e-9);
        assert!((sparkline_top(None, None) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn times_ago_round_down_to_their_unit() {
        assert_eq!(ago(999), "0 s ago");
        assert_eq!(ago(59_999), "59 s ago");
        assert_eq!(ago(60_000), "1 min ago");
        assert_eq!(ago(7_200_000), "2 h ago");
    }
}
