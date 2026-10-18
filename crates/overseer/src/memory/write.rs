//! Queued Memory writes and the reads that show them ([`Memory::write_and_observe`]).

use std::sync::Arc;
use std::time::Duration;

use futures_util::Stream;
use futures_util::stream;
use overseer_net::api::MemoryApi as _;
use overseer_net::model::memory::{MemoryChange, MemoryPath};
use overseer_net::rt;

use super::{Memory, MemoryRead, MemorySnapshot};
use crate::Error;

/// The most reads of a written path that [`ObservePolicy`] allows: screeps.com limits the
/// Memory reads of a user per day.
pub const MAX_OBSERVE_READS: usize = 3;

/// When [`Memory::write_and_observe`] reads the written path again: after each pause, at most
/// [`MAX_OBSERVE_READS`] times.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObservePolicy {
    pauses: [Duration; MAX_OBSERVE_READS],
    reads: usize,
}

impl ObservePolicy {
    /// Reads after each of `pauses` (each pause starts when the previous read ended); pauses
    /// after the first [`MAX_OBSERVE_READS`] are not used.
    #[must_use]
    pub fn new(pauses: impl IntoIterator<Item = Duration>) -> Self {
        let mut policy = Self {
            pauses: [Duration::ZERO; MAX_OBSERVE_READS],
            reads: 0,
        };
        for (slot, pause) in policy.pauses.iter_mut().zip(pauses) {
            *slot = pause;
            policy.reads += 1;
        }
        policy
    }

    /// The pauses before each read.
    #[must_use]
    pub fn pauses(&self) -> &[Duration] {
        &self.pauses[..self.reads]
    }
}

impl Default for ObservePolicy {
    /// Reads at about 2, 5 and 10 seconds after the server took the write: a few ticks.
    fn default() -> Self {
        Self::new([
            Duration::from_secs(2),
            Duration::from_secs(3),
            Duration::from_secs(5),
        ])
    }
}

/// A step of [`Memory::write_and_observe`].
#[derive(Debug)]
pub enum WriteProgress {
    /// The server took the write; the player's code applies it on a later tick.
    Queued,
    /// A read of the path shows the change (the last step). The snapshots of the session have
    /// the change.
    Observed(Arc<MemoryRead>),
    /// No read of the [`ObservePolicy`] showed the change (the last step). The write can
    /// still apply later, when the player's code runs.
    NotObserved,
    /// The server did not take the write (the last step).
    Failed(Error),
}

/// What a write and its observation need.
struct Observation {
    memory: Memory,
    snapshot: MemorySnapshot,
    change: MemoryChange,
    policy: ObservePolicy,
}

/// The next step of a write.
enum Step {
    /// Report this failure.
    Fail(Error),
    /// Send the write.
    Send(Observation),
    /// Read until a read shows the write.
    Observe(Observation),
    /// Nothing more.
    End,
}

impl Memory {
    /// Queues `change` of the value at `path` (`POST /api/user/memory`), then reads the path
    /// as `policy` says until a read shows the change ([`MemoryChange::is_shown_by`]). The
    /// stream gives [`WriteProgress::Queued`] and then [`WriteProgress::Observed`] or
    /// [`WriteProgress::NotObserved`], or only [`WriteProgress::Failed`]. A read that fails is
    /// skipped.
    ///
    /// Each read goes through the path's [`MemorySnapshot`]. When a read shows the change, the
    /// snapshots of the paths above and below get it too
    /// ([`MemoryChange::apply`]). A shown value tells only that Memory holds it, not that
    /// this write made it. Drop the stream to stop observing; the write stays queued.
    ///
    /// The change of all of `Memory` ([`MemoryPath::root`]) replaces every key: an app writes
    /// keys one by one.
    pub fn write_and_observe(
        &self,
        path: &MemoryPath,
        change: MemoryChange,
        shard: Option<&str>,
        policy: ObservePolicy,
    ) -> impl Stream<Item = WriteProgress> + use<> {
        let first = match self.snapshot(path, shard) {
            Ok(snapshot) => Step::Send(Observation {
                memory: self.clone(),
                snapshot,
                change,
                policy,
            }),
            Err(error) => Step::Fail(error),
        };
        stream::unfold(first, |step| async move {
            match step {
                Step::Fail(error) => Some((WriteProgress::Failed(error), Step::End)),
                Step::Send(observation) => Some(match observation.send().await {
                    Ok(()) => (WriteProgress::Queued, Step::Observe(observation)),
                    Err(error) => (WriteProgress::Failed(error), Step::End),
                }),
                Step::Observe(observation) => Some((observation.observe().await, Step::End)),
                Step::End => None,
            }
        })
    }
}

impl Observation {
    /// Sends the write.
    async fn send(&self) -> Result<(), Error> {
        let api = self.memory.overseer.user_api()?;
        let path = self.snapshot.path().as_str();
        let shard = self.snapshot.shard();
        match &self.change {
            MemoryChange::Set(value) => api.set_memory(path, value.clone(), shard).await?,
            MemoryChange::Delete => api.delete_memory(path, shard).await?,
        }
        Ok(())
    }

    /// Reads the path until a read shows the change.
    async fn observe(&self) -> WriteProgress {
        for pause in self.policy.pauses() {
            rt::sleep(*pause).await;
            let Ok(read) = self.snapshot.refresh(None).await else {
                continue;
            };
            if self.change.is_shown_by(&read.value) {
                self.memory.overseer.memory_snapshots().apply(
                    self.snapshot.shard(),
                    self.snapshot.path(),
                    &self.change,
                );
                return WriteProgress::Observed(read);
            }
        }
        WriteProgress::NotObserved
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{MAX_OBSERVE_READS, ObservePolicy};

    #[test]
    fn reads_a_written_path_at_most_three_times() {
        let second = Duration::from_secs(1);
        assert_eq!(
            ObservePolicy::default().pauses(),
            [2 * second, 3 * second, 5 * second]
        );
        let many = ObservePolicy::new([second; 10]);
        assert_eq!(many.pauses().len(), MAX_OBSERVE_READS);
        assert_eq!(ObservePolicy::new([second]).pauses(), [second]);
        assert_eq!(ObservePolicy::new([]).pauses(), [] as [Duration; 0]);
    }
}
