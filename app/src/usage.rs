//! CPU and Memory usage of the signed-in account's code: the recent runs, their average and
//! peak, and how near the limits they come.
//!
//! The server sends one event for each run of the code (`user:<id>/cpu`,
//! [`overseer::Account::cpu`]): the CPU time used, or the CPU limit hit, and the size of the
//! serialised Memory. The event names no shard, so the app cannot tell the usage of each shard.
//! [`Runs`] keeps the last [`WINDOW`] runs. The usage is idle when no run arrived for
//! [`IDLE_AFTER_MS`], as in the official client.
//!
//! The CPU limit of a run is the usable CPU of the shard on the map where the server allocates
//! CPU to each shard ([`cpu_limit`], [`ShardCpu`]), else the CPU of the account. Memory has a
//! fixed limit ([`MEMORY_LIMIT`]).

use std::collections::{BTreeMap, VecDeque};

use overseer::model::socket::{UserCpu, UserCpuCpu};
use overseer::model::world::ShardInfo;

/// The runs [`Runs`] keeps.
pub const WINDOW: usize = 120;
/// After this time without a run the usage is idle, in milliseconds.
pub const IDLE_AFTER_MS: u64 = 10_000;
/// The Memory limit of each shard: 2 MiB, in bytes.
pub const MEMORY_LIMIT: u64 = 2 * 1024 * 1024;
/// From this share of a limit on, a value is near the limit ([`Level::Warn`]).
const WARN_SHARE: f64 = 0.8;

/// The CPU of a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunCpu {
    /// The CPU time used, in milliseconds.
    Used(f64),
    /// The run hit the CPU limit (and was stopped).
    LimitHit,
}

/// A run of the code.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    /// The CPU of the run.
    pub cpu: RunCpu,
    /// The size of the serialised Memory, in bytes, when reported.
    pub memory: Option<u64>,
    /// When the event arrived, in milliseconds since the Unix epoch.
    pub at: u64,
}

impl Run {
    /// The run of `event`, arrived at `at`. The `first` event after subscribing reports no run
    /// when its CPU is 0: the server sends it to every new subscriber.
    pub fn of(event: &UserCpu, at: u64, first: bool) -> Option<Self> {
        let cpu = match event.cpu {
            UserCpuCpu::Number(ms) if first && ms <= 0.0 => return None,
            UserCpuCpu::Number(ms) => RunCpu::Used(ms),
            UserCpuCpu::Error => RunCpu::LimitHit,
        };
        Some(Self {
            cpu,
            memory: event.memory,
            at,
        })
    }
}

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

/// The average and the peak of values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    /// The average.
    pub average: f64,
    /// The largest value.
    pub peak: f64,
    /// How many values.
    pub count: usize,
}

impl Stats {
    /// The stats of `values`, `None` without values.
    fn of(values: impl Iterator<Item = f64>) -> Option<Self> {
        let (sum, peak, count) = values
            .fold((0.0, f64::MIN, 0_usize), |(sum, peak, count), value| {
                (sum + value, peak.max(value), count + 1)
            });
        (count > 0).then(|| {
            #[expect(clippy::cast_precision_loss, reason = "the count is at most WINDOW")]
            let average = sum / count as f64;
            Self {
                average,
                peak,
                count,
            }
        })
    }
}

/// The CPU of the recent runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CpuStats {
    /// Average and peak of the runs that did not hit the limit; `None` when all hit it.
    pub used: Option<Stats>,
    /// The runs that hit the CPU limit.
    pub limit_hits: usize,
}

/// The last [`WINDOW`] runs, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runs {
    runs: VecDeque<Run>,
}

impl Runs {
    /// Adds `run`; the oldest run goes when the window is full.
    pub fn push(&mut self, run: Run) {
        if self.runs.len() == WINDOW {
            self.runs.pop_front();
        }
        self.runs.push_back(run);
    }

    /// The latest run.
    pub fn latest(&self) -> Option<&Run> {
        self.runs.back()
    }

    /// How many runs it keeps.
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether no run arrived for [`IDLE_AFTER_MS`] at `now` (milliseconds since the Unix
    /// epoch); without runs it is idle.
    pub fn is_idle(&self, now: u64) -> bool {
        self.latest()
            .is_none_or(|run| now.saturating_sub(run.at) >= IDLE_AFTER_MS)
    }

    /// The CPU of the runs; `None` without runs.
    pub fn cpu(&self) -> Option<CpuStats> {
        (!self.runs.is_empty()).then(|| CpuStats {
            used: Stats::of(self.runs.iter().filter_map(|run| match run.cpu {
                RunCpu::Used(ms) => Some(ms),
                RunCpu::LimitHit => None,
            })),
            limit_hits: self
                .runs
                .iter()
                .filter(|run| run.cpu == RunCpu::LimitHit)
                .count(),
        })
    }

    /// The average and peak Memory size of the runs that report one, in bytes.
    pub fn memory(&self) -> Option<Stats> {
        Stats::of(self.memory_sizes())
    }

    /// The Memory size of each run that reports one, in bytes, oldest first.
    #[expect(
        clippy::cast_precision_loss,
        reason = "Memory sizes are far below 2^52"
    )]
    fn memory_sizes(&self) -> impl Iterator<Item = f64> {
        self.runs
            .iter()
            .filter_map(|run| run.memory.map(|bytes| bytes as f64))
    }

    /// The CPU of each run for a sparkline, oldest first: a run that hit the limit counts as
    /// infinite (at the top edge).
    pub fn cpu_series(&self) -> Vec<f64> {
        self.runs
            .iter()
            .map(|run| match run.cpu {
                RunCpu::Used(ms) => ms,
                RunCpu::LimitHit => f64::INFINITY,
            })
            .collect()
    }

    /// The Memory size of each run that reports one for a sparkline, in bytes, oldest first.
    pub fn memory_series(&self) -> Vec<f64> {
        self.memory_sizes().collect()
    }
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
/// the right edge, one step of `width / (WINDOW - 1)` for each value before it.
#[expect(
    clippy::cast_precision_loss,
    reason = "WINDOW and the indices are small"
)]
pub fn sparkline_x(index: usize, count: usize, width: f64) -> f64 {
    width - count.saturating_sub(index + 1) as f64 * width / (WINDOW - 1) as f64
}

/// The y of `value` in a sparkline `height` high with `top` at its top edge; a value above
/// `top` stays at the top edge.
pub fn sparkline_y(value: f64, top: f64, height: f64) -> f64 {
    height - height * share(value, Some(top)).unwrap_or(0.0).clamp(0.0, 1.0)
}

/// The points of a sparkline of `values` (oldest first, at most [`WINDOW`]) in a box
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

/// The CPU of a shard where the server allocates CPU to each shard, with its average tick.
#[derive(Clone, Debug, PartialEq)]
pub struct ShardCpu {
    /// The shard's name.
    pub name: String,
    /// The CPU limit the account assigns to the shard.
    pub assigned: f64,
    /// The most CPU the shard lets a player use, when the shard caps it.
    pub cap: Option<f64>,
    /// The shard's average tick duration in milliseconds, when the server lists it.
    pub average_tick: Option<f64>,
    /// The map shows this shard.
    pub shown: bool,
}

impl ShardCpu {
    /// The CPU the code can use on the shard ([`usable`]).
    pub fn usable(&self) -> f64 {
        usable(self.assigned, self.cap)
    }

    /// Whether the account assigns more CPU than the shard lets it use.
    pub fn over_cap(&self) -> bool {
        self.cap.is_some_and(|cap| self.assigned > cap)
    }
}

/// The CPU the code can use on a shard: the `assigned` CPU, at most the shard's `cap`.
pub fn usable(assigned: f64, cap: Option<f64>) -> f64 {
    cap.map_or(assigned, |cap| assigned.min(cap))
}

/// The CPU caps of the `shards` that cap the CPU, by shard name.
pub fn shard_caps(shards: &[ShardInfo]) -> BTreeMap<String, f64> {
    shards
        .iter()
        .filter_map(|shard| shard.cpu_limit.map(|cap| (shard.name.clone(), cap)))
        .collect()
}

/// The shards of a CPU allocation: the `server`'s shards in their order, then the shards that
/// only the allocation `assigned` names.
pub fn shard_order(server: &[String], assigned: &BTreeMap<String, f64>) -> Vec<String> {
    let extra = assigned.keys().filter(|name| !server.contains(name));
    server.iter().chain(extra).cloned().collect()
}

/// The CPU of each shard ([`shard_order`]; a shard missing from the allocation `assigned`
/// gets 0), with the cap and the average tick of the shard's entry in `info` and the `shown`
/// shard marked.
pub fn shard_cpu(
    shards: &[String],
    assigned: &BTreeMap<String, f64>,
    info: &[ShardInfo],
    shown: Option<&str>,
) -> Vec<ShardCpu> {
    shard_order(shards, assigned)
        .into_iter()
        .map(|name| {
            let entry = info.iter().find(|shard| shard.name == name);
            ShardCpu {
                assigned: assigned.get(&name).copied().unwrap_or(0.0),
                cap: entry.and_then(|shard| shard.cpu_limit),
                average_tick: entry.and_then(|shard| shard.tick),
                shown: shown == Some(name.as_str()),
                name,
            }
        })
        .collect()
}

/// The CPU limit of a run: the usable CPU of the shard on the map among `shards`, else the
/// `account` CPU.
pub fn cpu_limit(account: Option<f64>, shards: &[ShardCpu]) -> Option<f64> {
    shards
        .iter()
        .find(|shard| shard.shown)
        .map(ShardCpu::usable)
        .or(account)
}

/// [`MEMORY_LIMIT`] as a float, in bytes.
#[expect(clippy::cast_precision_loss, reason = "2 MiB is exact")]
const MEMORY_LIMIT_F64: f64 = MEMORY_LIMIT as f64;

/// `bytes` as a share of the Memory limit.
#[expect(
    clippy::cast_precision_loss,
    reason = "Memory sizes are far below 2^52"
)]
pub fn memory_level(bytes: u64) -> Level {
    Level::of(bytes as f64, Some(MEMORY_LIMIT_F64))
}

/// The width of the Memory bar of `bytes`, in percent.
#[expect(
    clippy::cast_precision_loss,
    reason = "Memory sizes are far below 2^52"
)]
pub fn memory_bar(bytes: u64) -> f64 {
    bar_width(bytes as f64, Some(MEMORY_LIMIT_F64))
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
        cpu_amount(kilobytes(MEMORY_LIMIT))
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
        percent(bytes as f64, Some(MEMORY_LIMIT_F64)).unwrap_or(0)
    )
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
        cpu_amount(kilobytes(MEMORY_LIMIT)),
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
    let idle = if idle {
        "No run in the last 10 s. "
    } else {
        ""
    };
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
    use std::collections::BTreeMap;

    use overseer::model::socket::{UserCpu, UserCpuCpu};
    use overseer::model::world::ShardInfo;

    use super::{
        IDLE_AFTER_MS, Level, MEMORY_LIMIT, Run, RunCpu, Runs, ShardCpu, Stats, WINDOW, ago,
        bar_width, cpu_amount, cpu_limit, cpu_long, cpu_short, kilobytes, memory_amount,
        memory_bar, memory_level, memory_long, memory_short, percent, run_text, shard_caps,
        shard_cpu, shard_order, sparkline, sparkline_top, sparkline_x, summary, usable,
    };

    #[test]
    fn caps_come_from_the_shards_that_cap_the_cpu() {
        let info = |name: &str, cpu_limit| ShardInfo {
            name: name.to_owned(),
            cpu_limit,
            tick: None,
        };
        assert_eq!(
            shard_caps(&[info("shard0", None), info("shard3", Some(20.0))]),
            BTreeMap::from([("shard3".to_owned(), 20.0)])
        );
        assert!((usable(30.0, Some(20.0)) - 20.0).abs() < 1e-9);
        assert!((usable(10.0, Some(20.0)) - 10.0).abs() < 1e-9);
        assert!((usable(30.0, None) - 30.0).abs() < 1e-9);
        let assigned = BTreeMap::from([("a".to_owned(), 1.0), ("shard1".to_owned(), 2.0)]);
        assert_eq!(
            shard_order(&["shard1".to_owned(), "shard0".to_owned()], &assigned),
            ["shard1", "shard0", "a"]
        );
    }

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
        assert_eq!(memory_level(MEMORY_LIMIT), Level::Error);
        assert_eq!(memory_level(39_014), Level::Ok);
        assert!((memory_bar(MEMORY_LIMIT / 4) - 25.0).abs() < 1e-9);
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
    fn the_greeting_of_a_new_subscription_is_no_run() {
        let event = |cpu| UserCpu {
            cpu,
            memory: Some(2048),
        };
        assert_eq!(Run::of(&event(UserCpuCpu::Number(0.0)), 5, true), None);
        assert_eq!(
            Run::of(&event(UserCpuCpu::Number(0.0)), 5, false),
            Some(run(0.0, 2048, 5))
        );
        assert_eq!(
            Run::of(&event(UserCpuCpu::Number(3.5)), 5, true),
            Some(run(3.5, 2048, 5))
        );
        assert_eq!(
            Run::of(&event(UserCpuCpu::Error), 5, true).map(|run| run.cpu),
            Some(RunCpu::LimitHit)
        );
    }

    #[test]
    fn the_window_keeps_the_latest_runs() {
        let mut runs = Runs::default();
        assert_eq!(runs.latest(), None);
        assert_eq!(runs.cpu(), None);
        assert_eq!(runs.memory(), None);
        for at in 0..WINDOW as u64 + 5 {
            #[expect(clippy::cast_precision_loss, reason = "small numbers")]
            runs.push(run(at as f64, 100, at));
        }
        assert_eq!(runs.len(), WINDOW);
        assert_eq!(runs.runs.front().map(|run| run.at), Some(5));
        assert_eq!(runs.latest().map(|run| run.at), Some(WINDOW as u64 + 4));
    }

    #[test]
    fn stats_average_and_peak_the_runs_and_count_limit_hits() {
        let mut runs = Runs::default();
        runs.push(run(2.0, 1000, 1));
        runs.push(run(6.0, 3000, 2));
        runs.push(Run {
            cpu: RunCpu::LimitHit,
            memory: None,
            at: 3,
        });
        runs.push(run(4.0, 2000, 4));
        let cpu = runs.cpu().expect("runs");
        assert_eq!(
            cpu.used,
            Some(Stats {
                average: 4.0,
                peak: 6.0,
                count: 3
            })
        );
        assert_eq!(cpu.limit_hits, 1);
        assert_eq!(
            runs.memory(),
            Some(Stats {
                average: 2000.0,
                peak: 3000.0,
                count: 3
            })
        );

        let mut hits = Runs::default();
        hits.push(Run {
            cpu: RunCpu::LimitHit,
            memory: None,
            at: 1,
        });
        let cpu = hits.cpu().expect("runs");
        assert_eq!(cpu.used, None);
        assert_eq!(cpu.limit_hits, 1);
        assert_eq!(hits.memory(), None);
    }

    #[test]
    fn the_usage_is_idle_10_seconds_after_the_latest_run() {
        let mut runs = Runs::default();
        assert!(runs.is_idle(0));
        runs.push(run(1.0, 1, 1_000));
        assert!(!runs.is_idle(1_000));
        assert!(!runs.is_idle(1_000 + IDLE_AFTER_MS - 1));
        assert!(runs.is_idle(1_000 + IDLE_AFTER_MS));
        // A clock behind the event is not idle.
        assert!(!runs.is_idle(500));
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
        let memory_limit = Some(MEMORY_LIMIT as f64);
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
        assert!((sparkline_x(0, WINDOW, 119.0)).abs() < 1e-9);
        assert!((sparkline_x(4, 5, 119.0) - 119.0).abs() < 1e-9);
        // Near the limit: the limit line shows; far below it: the peak sets the scale.
        assert!((sparkline_top(Some(20.0), Some(12.0)) - 25.0).abs() < 1e-9);
        assert!((sparkline_top(Some(20.0), Some(24.0)) - 30.0).abs() < 1e-9);
        assert!((sparkline_top(Some(20.0), Some(4.0)) - 5.0).abs() < 1e-9);
        assert!((sparkline_top(Some(20.0), None) - 25.0).abs() < 1e-9);
        assert!((sparkline_top(None, Some(10.0)) - 12.5).abs() < 1e-9);
        assert!((sparkline_top(None, None) - 1.0).abs() < 1e-9);

        let mut runs = Runs::default();
        runs.push(run(2.0, 100, 1));
        runs.push(Run {
            cpu: RunCpu::LimitHit,
            memory: None,
            at: 2,
        });
        assert_eq!(runs.cpu_series(), [2.0, f64::INFINITY]);
        assert_eq!(runs.memory_series(), [100.0]);
    }

    #[test]
    fn times_ago_round_down_to_their_unit() {
        assert_eq!(ago(999), "0 s ago");
        assert_eq!(ago(59_999), "59 s ago");
        assert_eq!(ago(60_000), "1 min ago");
        assert_eq!(ago(7_200_000), "2 h ago");
    }

    #[test]
    fn shards_list_the_servers_shards_then_the_allocations_with_their_caps() {
        let shards = ["shard0".to_owned(), "shard3".to_owned()];
        let assigned =
            BTreeMap::from([("shard3".to_owned(), 30.0), ("shardSeason".to_owned(), 5.0)]);
        let info = [
            ShardInfo {
                name: "shard0".to_owned(),
                cpu_limit: None,
                tick: Some(4100.0),
            },
            ShardInfo {
                name: "shard3".to_owned(),
                cpu_limit: Some(20.0),
                tick: Some(3150.5),
            },
        ];
        let rows = shard_cpu(&shards, &assigned, &info, Some("shard3"));
        assert_eq!(
            rows,
            [
                ShardCpu {
                    name: "shard0".to_owned(),
                    assigned: 0.0,
                    cap: None,
                    average_tick: Some(4100.0),
                    shown: false,
                },
                ShardCpu {
                    name: "shard3".to_owned(),
                    assigned: 30.0,
                    cap: Some(20.0),
                    average_tick: Some(3150.5),
                    shown: true,
                },
                ShardCpu {
                    name: "shardSeason".to_owned(),
                    assigned: 5.0,
                    cap: None,
                    average_tick: None,
                    shown: false,
                },
            ]
        );
        assert!((rows[1].usable() - 20.0).abs() < 1e-9);
        assert!(rows[1].over_cap());
        assert!(!rows[2].over_cap());
        assert!((rows[2].usable() - 5.0).abs() < 1e-9);
        // The run's limit: the usable CPU of the shard on the map, else the account's CPU.
        assert_eq!(cpu_limit(Some(35.0), &rows), Some(20.0));
        assert_eq!(cpu_limit(Some(35.0), &rows[..1]), Some(35.0));
        assert_eq!(cpu_limit(None, &[]), None);
    }
}
