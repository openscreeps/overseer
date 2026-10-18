use overseer_net::model::account::cpu::ShardCpu;
use overseer_net::model::socket::{UserCpu, UserCpuCpu};

use super::{IDLE_AFTER_MS, RUNS_KEPT, Run, RunCpu, Runs, Stats, Usage};

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
    assert!(runs.is_empty());
    assert_eq!(runs.cpu(), None);
    assert_eq!(runs.memory(), None);
    for at in 0..RUNS_KEPT as u64 + 5 {
        #[expect(clippy::cast_precision_loss, reason = "small numbers")]
        runs.push(run(at as f64, 100, at));
    }
    assert_eq!(runs.len(), RUNS_KEPT);
    assert_eq!(runs.iter().next().map(|run| run.at), Some(5));
    assert_eq!(runs.latest().map(|run| run.at), Some(RUNS_KEPT as u64 + 4));
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
    // A limit hit counts as infinite in the series.
    assert_eq!(runs.cpu_series(), [2.0, 6.0, f64::INFINITY, 4.0]);
    assert_eq!(runs.memory_series(), [1000.0, 3000.0, 2000.0]);

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
    let mut usage = Usage::default();
    assert!(usage.is_idle(0));
    usage.runs.push(run(1.0, 1, 1_000));
    assert!(!usage.is_idle(1_000));
    assert!(!usage.is_idle(1_000 + IDLE_AFTER_MS - 1));
    assert!(usage.is_idle(1_000 + IDLE_AFTER_MS));
    // A clock behind the event is not idle.
    assert!(!usage.is_idle(500));
}

#[test]
fn the_cpu_limit_is_the_shards_usable_cpu_else_the_accounts() {
    let usage = Usage {
        cpu: Some(35.0),
        shards: vec![ShardCpu {
            name: "shard3".to_owned(),
            assigned: 30.0,
            cap: Some(20.0),
            average_tick: None,
        }],
        ..Usage::default()
    };
    assert_eq!(usage.cpu_limit(Some("shard3")), Some(20.0));
    assert_eq!(usage.cpu_limit(Some("shard0")), Some(35.0));
    assert_eq!(usage.cpu_limit(None), Some(35.0));
    assert_eq!(Usage::default().cpu_limit(Some("shard3")), None);
}
