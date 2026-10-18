//! The account's CPU: its allocation to the shards (`POST /api/user/cpu-shards`, on servers
//! with [`Subsystem::CpuShards`](crate::model::platform::Subsystem::CpuShards)), the CPU that
//! the code can use on each shard, and when the allocation can change next.
//!
//! The limits of the shards must total the account's CPU ([`Profile::cpu`]), and the server
//! refuses a change for [`CHANGE_COOLDOWN_MS`] after the previous one
//! ([`Profile::cpu_shards_updated`]). Some shards cap the CPU that a player can use
//! ([`ShardInfo::cpu_limit`]; screeps.com's shard3 allows 20). An allocation above a shard's
//! cap is valid: the server accepts it, and the code can use only the cap
//! ([`usable`]).

use std::collections::BTreeMap;

use crate::model::account::Profile;
use crate::model::world::ShardInfo;

/// The time that the server makes the account wait between two changes of the allocation:
/// 12 hours, in milliseconds.
pub const CHANGE_COOLDOWN_MS: f64 = 12.0 * 60.0 * 60.0 * 1000.0;

/// The largest whole number that an `f64` holds exactly: the end of the range of
/// [`whole`].
const EXACT_MAX: f64 = 9_007_199_254_740_992.0;

/// The CPU that the code can use on a shard: the `assigned` CPU, at most the shard's `cap`.
#[must_use]
pub fn usable(assigned: f64, cap: Option<f64>) -> f64 {
    cap.map_or(assigned, |cap| assigned.min(cap))
}

/// The CPU of one shard of an allocation, with the cap and the average tick of the shard.
#[derive(Clone, Debug, PartialEq)]
pub struct ShardCpu {
    /// The shard's name.
    pub name: String,
    /// The CPU limit that the account assigns to the shard.
    pub assigned: f64,
    /// The most CPU that the shard lets a player use, when the shard caps it.
    pub cap: Option<f64>,
    /// The shard's average tick duration in milliseconds, when the server lists it.
    pub average_tick: Option<f64>,
}

impl ShardCpu {
    /// The CPU that the code can use on the shard ([`usable`]).
    #[must_use]
    pub fn usable(&self) -> f64 {
        usable(self.assigned, self.cap)
    }

    /// Whether the account assigns more CPU to the shard than the shard lets it use.
    #[must_use]
    pub fn over_cap(&self) -> bool {
        self.cap.is_some_and(|cap| self.assigned > cap)
    }
}

/// The shards of an allocation: the `server`'s shards in their order, then the shards that
/// only the allocation `assigned` names.
#[must_use]
pub fn shard_order(server: &[String], assigned: &BTreeMap<String, f64>) -> Vec<String> {
    let extra = assigned.keys().filter(|name| !server.contains(name));
    server.iter().chain(extra).cloned().collect()
}

/// The CPU of each shard in [`shard_order`] (a shard that the allocation `assigned` does not
/// name gets 0), with the cap and the average tick of the shard's entry in `info`.
#[must_use]
pub fn shard_cpu(
    server: &[String],
    assigned: &BTreeMap<String, f64>,
    info: &[ShardInfo],
) -> Vec<ShardCpu> {
    shard_order(server, assigned)
        .into_iter()
        .map(|name| {
            let entry = info.iter().find(|shard| shard.name == name);
            ShardCpu {
                assigned: assigned.get(&name).copied().unwrap_or(0.0),
                cap: entry.and_then(|shard| shard.cpu_limit),
                average_tick: entry.and_then(|shard| shard.tick),
                name,
            }
        })
        .collect()
}

/// The CPU limit of a run of the code on `shard`: the usable CPU of that shard among
/// `shards`, else the account's `cpu`.
#[must_use]
pub fn cpu_limit(cpu: Option<f64>, shards: &[ShardCpu], shard: Option<&str>) -> Option<f64> {
    shard
        .and_then(|shard| shards.iter().find(|entry| entry.name == shard))
        .map(ShardCpu::usable)
        .or(cpu)
}

/// Why the server refuses an allocation.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum AllocationError {
    /// The server does not report the account's CPU.
    #[error("the server does not report the account's CPU")]
    UnknownCpu,
    /// The limits do not total the account's CPU.
    #[error("the shards get {total} CPU in total, not the account's {cpu} CPU")]
    Total {
        /// The sum of the limits.
        total: u64,
        /// The account's CPU.
        cpu: f64,
    },
    /// The previous change is too recent.
    #[error("the CPU allocation changed less than 12 hours ago")]
    Cooldown {
        /// When the server allows the next change, in milliseconds since the Unix epoch.
        next: f64,
    },
}

/// The allocation rules of an account: its CPU and the time of the previous change.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuAllocation {
    /// The account's CPU ([`Profile::cpu`]), when reported.
    pub cpu: Option<f64>,
    /// When the allocation changed last ([`Profile::cpu_shards_updated`]), in milliseconds
    /// since the Unix epoch, when reported.
    pub changed: Option<f64>,
}

impl CpuAllocation {
    /// The allocation rules of `profile`.
    #[must_use]
    pub const fn of(profile: &Profile) -> Self {
        Self {
            cpu: profile.cpu,
            changed: profile.cpu_shards_updated,
        }
    }

    /// The account's CPU as a whole number: rounded, 0 or more. `None` when the server does
    /// not report it.
    #[must_use]
    pub fn total(&self) -> Option<u64> {
        self.cpu.map(whole)
    }

    /// When the server allows the next change, in milliseconds since the Unix epoch:
    /// [`CHANGE_COOLDOWN_MS`] after the previous change. `None` when that time is not after
    /// `now`, or when no previous change is known.
    #[must_use]
    pub fn next_change_at(&self, now: f64) -> Option<f64> {
        self.changed
            .map(|changed| changed + CHANGE_COOLDOWN_MS)
            .filter(|next| *next > now)
    }

    /// The most CPU that `shard` can get in `allocation` (CPU by shard; an absent shard has
    /// 0): what the other shards leave of the account's CPU. `None` when the server does not
    /// report the account's CPU.
    #[must_use]
    pub fn remaining_for(&self, allocation: &BTreeMap<String, u64>, shard: &str) -> Option<u64> {
        let others = allocation
            .iter()
            .filter(|(name, _)| *name != shard)
            .map(|(_, cpu)| *cpu)
            .fold(0, u64::saturating_add);
        self.total().map(|total| total.saturating_sub(others))
    }

    /// Checks `allocation` (CPU by shard) at `now` (milliseconds since the Unix epoch) as the
    /// server does: the account's CPU is known, the limits total it, and the cooldown after
    /// the previous change is over. A limit above a shard's cap is valid.
    ///
    /// The cooldown is checked against the local clock: the server's answer stays the
    /// authority.
    ///
    /// # Errors
    ///
    /// Returns the first [`AllocationError`] that applies, in the order of its variants.
    pub fn validate(
        &self,
        allocation: &BTreeMap<String, u64>,
        now: f64,
    ) -> Result<(), AllocationError> {
        let cpu = self.cpu.ok_or(AllocationError::UnknownCpu)?;
        let total = allocation.values().copied().fold(0, u64::saturating_add);
        if Some(total) != self.total() {
            return Err(AllocationError::Total { total, cpu });
        }
        match self.next_change_at(now) {
            Some(next) => Err(AllocationError::Cooldown { next }),
            None => Ok(()),
        }
    }
}

/// `cpu` as a whole number of CPU: rounded, 0 or more.
fn whole(cpu: f64) -> u64 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the exact range of f64 first"
    )]
    let whole = cpu.clamp(0.0, EXACT_MAX).round() as u64;
    whole
}

#[cfg(test)]
mod tests;
