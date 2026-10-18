//! The CPU and Memory usage of the signed-in user's code ([`Account::usage`]): the recent runs,
//! their average and peak, and the CPU limit of a run.
//!
//! The server sends one event for each run of the code (`user:<user>/cpu`,
//! [`Account::cpu`]): the CPU time used, or `error` when the run hit the CPU limit, and the
//! size of the serialised Memory. The event names no shard. The session keeps the last
//! [`RUNS_KEPT`] runs in one [`Usage`] that every consumer shares, together with the account's
//! CPU ([`Account::me`]) and, where the server allocates CPU to each shard
//! ([`Subsystem::CpuShards`]), the CPU of each shard with its cap and average tick
//! ([`Overseer::shards_info`]).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use overseer_net::model::account::Profile;
use overseer_net::model::account::cpu::{self, ShardCpu};
use overseer_net::model::platform::Subsystem;
use overseer_net::model::socket::{UserCpu, UserCpuCpu};
use overseer_net::model::world::ShardInfo;
use overseer_net::rt;
use tokio::sync::watch;

#[cfg(doc)]
use crate::Account;
use crate::{Overseer, Watch};

/// The runs that a [`Usage`] keeps.
pub const RUNS_KEPT: usize = 120;

/// After this time without a run the usage is idle ([`Usage::is_idle`]), in milliseconds.
/// This is the client's choice, as in the official client; the server sends no idle state.
pub const IDLE_AFTER_MS: u64 = 10_000;

/// The CPU of a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunCpu {
    /// The CPU time used, in milliseconds.
    Used(f64),
    /// The run hit the CPU limit (and the server stopped it): the event's `cpu` is `error`.
    LimitHit,
}

/// One run of the code.
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
    /// The run of `event`, arrived at `at`. The `first` event after subscribing is no run when
    /// its CPU is 0: the server sends it to every new subscriber.
    pub(crate) fn of(event: &UserCpu, at: u64, first: bool) -> Option<Self> {
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
    /// The stats of `values`; `None` without values.
    fn of(values: impl Iterator<Item = f64>) -> Option<Self> {
        let (sum, peak, count) = values
            .fold((0.0, f64::MIN, 0_usize), |(sum, peak, count), value| {
                (sum + value, peak.max(value), count + 1)
            });
        (count > 0).then(|| {
            #[expect(clippy::cast_precision_loss, reason = "the count is at most RUNS_KEPT")]
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
    /// How many runs hit the CPU limit.
    pub limit_hits: usize,
}

/// The last [`RUNS_KEPT`] runs, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Runs {
    runs: VecDeque<Run>,
}

impl Runs {
    /// Adds `run`; the oldest run goes when the window is full.
    pub(crate) fn push(&mut self, run: Run) {
        if self.runs.len() == RUNS_KEPT {
            self.runs.pop_front();
        }
        self.runs.push_back(run);
    }

    /// The latest run.
    #[must_use]
    pub fn latest(&self) -> Option<&Run> {
        self.runs.back()
    }

    /// The runs, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Run> {
        self.runs.iter()
    }

    /// How many runs it keeps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether it keeps no run.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Whether no run arrived for [`IDLE_AFTER_MS`] at `now` (milliseconds since the Unix
    /// epoch). Without runs it is idle; with a clock behind the latest run it is not.
    #[must_use]
    pub fn is_idle(&self, now: u64) -> bool {
        self.latest()
            .is_none_or(|run| now.saturating_sub(run.at) >= IDLE_AFTER_MS)
    }

    /// The CPU of the runs; `None` without runs.
    #[must_use]
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
    #[must_use]
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

    /// The CPU of each run, oldest first: a run that hit the limit counts as infinite.
    #[must_use]
    pub fn cpu_series(&self) -> Vec<f64> {
        self.runs
            .iter()
            .map(|run| match run.cpu {
                RunCpu::Used(ms) => ms,
                RunCpu::LimitHit => f64::INFINITY,
            })
            .collect()
    }

    /// The Memory size of each run that reports one, in bytes, oldest first.
    #[must_use]
    pub fn memory_series(&self) -> Vec<f64> {
        self.memory_sizes().collect()
    }
}

/// The usage of the session's account ([`Account::usage`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    /// The recent runs.
    pub runs: Runs,
    /// The account's CPU ([`Profile::cpu`]); `None` while the profile is not known or when
    /// the server does not report it.
    pub cpu: Option<f64>,
    /// The CPU of each shard where the server allocates CPU to each shard
    /// ([`Subsystem::CpuShards`]): the server's shards, then the shards that only the
    /// allocation names ([`cpu::shard_cpu`]). Empty on other servers and while the profile
    /// is not known.
    pub shards: Vec<ShardCpu>,
    /// The caps and average ticks of the shards are known ([`Overseer::shards_info`]). When
    /// not, [`ShardCpu::cap`] `None` means "not known" rather than "no cap".
    pub shards_known: bool,
}

impl Usage {
    /// The CPU limit of a run of the code on `shard`: the usable CPU of that shard
    /// ([`cpu::cpu_limit`]), else the account's CPU. The server does not tell the shard of a
    /// run: give the shard that the user looks at.
    #[must_use]
    pub fn cpu_limit(&self, shard: Option<&str>) -> Option<f64> {
        cpu::cpu_limit(self.cpu, &self.shards, shard)
    }

    /// Whether no run arrived for [`IDLE_AFTER_MS`] at `now` (milliseconds since the Unix
    /// epoch; [`Runs::is_idle`]).
    #[must_use]
    pub fn is_idle(&self, now: u64) -> bool {
        self.runs.is_idle(now)
    }
}

/// The sender of the session's usage while a task follows it.
pub(crate) type Slot = Mutex<Option<watch::Sender<Usage>>>;

/// A receiver of the session's usage: shares the running task, else starts one that follows
/// `cpu` (created only then).
pub(crate) fn subscribe(
    overseer: &Overseer,
    cpu: impl FnOnce() -> Watch<UserCpu>,
) -> watch::Receiver<Usage> {
    let mut slot = lock(&overseer.account_state().usage);
    if let Some(sender) = &*slot {
        return sender.subscribe();
    }
    let (sender, receiver) = watch::channel(Usage::default());
    *slot = Some(sender.clone());
    drop(slot);
    // Detached: the task ends when the last receiver is dropped or the session closes.
    let _task = rt::spawn(
        Follow {
            overseer: overseer.clone(),
            sender,
            cpu: cpu(),
        }
        .run(),
    );
    receiver
}

/// What a [`Usage`] is made of besides the runs.
struct Sources {
    /// The servers' shards ([`overseer_net::World::shards`]).
    server: Vec<String>,
    /// The server allocates CPU to each shard.
    per_shard: bool,
    profile: Option<Arc<Profile>>,
    info: Option<Arc<[ShardInfo]>>,
}

impl Sources {
    /// Puts the account's CPU and the CPU of each shard into `usage`.
    fn apply(&self, usage: &mut Usage) {
        usage.cpu = self.profile.as_ref().and_then(|profile| profile.cpu);
        usage.shards = match &self.profile {
            Some(profile) if self.per_shard => cpu::shard_cpu(
                &self.server,
                &profile.cpu_shards,
                self.info.as_deref().unwrap_or_default(),
            ),
            _ => Vec::new(),
        };
        usage.shards_known = self.info.is_some();
    }
}

/// The task that keeps the session's usage.
struct Follow {
    overseer: Overseer,
    sender: watch::Sender<Usage>,
    cpu: Watch<UserCpu>,
}

impl Follow {
    async fn run(mut self) {
        let account = self.overseer.account();
        let mut profile = account.profile_watch();
        let world = self.overseer.world();
        let mut sources = Sources {
            server: world.shards().to_vec(),
            per_shard: world.supports(Subsystem::CpuShards),
            profile: profile.borrow_and_update().clone(),
            info: None,
        };
        self.sender.send_modify(|usage| sources.apply(usage));
        let mut first = true;
        if let Some(event) = self.cpu.latest() {
            self.record(&event, &mut first);
        }
        // A failure leaves the CPU limit unknown; a later read of the profile publishes it.
        let load_profile = sources.profile.is_none();
        let me = async {
            if load_profile {
                let _ = account.me().await;
            }
        };
        let per_shard = sources.per_shard;
        let overseer = self.overseer.clone();
        // A failure leaves the caps and ticks unknown.
        let info = async move {
            if per_shard {
                overseer.shards_info().await.ok()
            } else {
                None
            }
        };
        tokio::pin!(me, info);
        let (mut me_done, mut info_done) = (false, false);
        loop {
            tokio::select! {
                biased;
                () = self.sender.closed() => {
                    if self.unused() {
                        return;
                    }
                }
                event = self.cpu.changed() => {
                    let Ok(event) = event else {
                        // The session closed.
                        *lock(&self.overseer.account_state().usage) = None;
                        return;
                    };
                    self.record(&event, &mut first);
                }
                Ok(()) = profile.changed() => {
                    sources.profile.clone_from(&profile.borrow_and_update());
                    self.sender.send_modify(|usage| sources.apply(usage));
                }
                () = &mut me, if !me_done => me_done = true,
                shards = &mut info, if !info_done => {
                    info_done = true;
                    sources.info = shards;
                    self.sender.send_modify(|usage| sources.apply(usage));
                }
            }
        }
    }

    /// Records the run of `event`, the `first` one after subscribing or not.
    fn record(&self, event: &UserCpu, first: &mut bool) {
        if let Some(run) = Run::of(event, super::epoch_millis(), *first) {
            self.sender.send_modify(|usage| usage.runs.push(run));
        }
        *first = false;
    }

    /// Whether no receiver is left; then the session forgets the task. Checked under the lock
    /// that [`subscribe`] takes, so a new receiver either gets this task or starts another.
    fn unused(&self) -> bool {
        let mut slot = lock(&self.overseer.account_state().usage);
        let unused = self.sender.receiver_count() == 0;
        if unused {
            *slot = None;
        }
        unused
    }
}

/// Locks a mutex whose critical sections cannot panic halfway through an update.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
