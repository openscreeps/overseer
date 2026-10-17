//! What the console and the Memory tab of one session share ([`Account`]): the shard that they
//! act on, and all of `Memory` on that shard.
//!
//! The drawer makes one [`Account`] for its Console and Memory tabs; the console window makes
//! its own. The two tabs act on one shard: the console completes the keys of the `Memory` that
//! the Memory tab shows. Thus both read `Memory` from one place: a key that the Memory tab adds
//! shows in the completion without a second read, and a read of one serves the other.

use std::time::Duration;

use leptos::prelude::*;
use overseer::memory::MemoryValue;
use overseer::net::Priority;
use overseer::net::rt::{Instant, SystemTime};
use overseer::{Memory, Overseer};

use crate::session;
use crate::ui::clock;
use crate::ui::style::{self, concatcp};

/// A value read from `Memory`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Read {
    /// The shard it was read on.
    pub shard: Option<String>,
    /// The value.
    pub value: MemoryValue,
    /// When it was read (`HH:MM:SS`).
    pub time: String,
}

impl Read {
    /// `value`, read on `shard` now.
    pub fn now(shard: Option<String>, value: MemoryValue) -> Self {
        Self {
            shard,
            value,
            time: clock(SystemTime::now()),
        }
    }
}

/// When a part of the server's data was last read, and for which shard.
#[derive(Debug, Default)]
pub struct Load {
    /// The shard it was read for.
    shard: Option<String>,
    /// When the read ended, also after a failure.
    at: Option<Instant>,
    /// The read gave nothing (or failed).
    empty: bool,
    /// A read runs.
    busy: bool,
}

impl Load {
    /// Whether the part must be read (again) for `shard`: no read runs, and the last one was
    /// for another shard or is older than `fresh` (`fresh_empty` when it gave nothing).
    pub fn stale(&self, shard: Option<&str>, fresh: Duration, fresh_empty: Duration) -> bool {
        let fresh = if self.empty { fresh_empty } else { fresh };
        !self.busy
            && (self.shard.as_deref() != shard || self.at.is_none_or(|at| at.elapsed() >= fresh))
    }

    /// Whether the last read was for `shard`.
    pub fn is_for(&self, shard: Option<&str>) -> bool {
        self.shard.as_deref() == shard
    }

    /// Marks a read for `shard` as started.
    pub fn start(&mut self, shard: Option<&str>) {
        self.shard = shard.map(str::to_owned);
        self.busy = true;
    }

    /// Marks the read as ended; `empty` when it gave nothing.
    pub fn finish(&mut self, empty: bool) {
        self.at = Some(Instant::now());
        self.empty = empty;
        self.busy = false;
    }
}

/// What an [`Account`] holds out of line: the session, the server's shards, and the reads of
/// all of `Memory`.
struct Session {
    /// The session.
    overseer: Overseer,
    /// The server's shards (empty on single-world servers).
    shards: Vec<String>,
    /// The reads of all of `Memory`.
    root_load: Load,
    /// The number of the latest read of all of `Memory`: the result of an older one is dropped.
    root_reads: u64,
}

/// The session, the shard and all of `Memory` there, for the console and the Memory tab;
/// `Copy`.
#[derive(Clone, Copy)]
pub struct Account {
    /// The session, the shards and the reads of [`Self::root`].
    session: StoredValue<Session, LocalStorage>,
    /// The shard to act on (`None`: the server's only world).
    pub shard: RwSignal<Option<String>>,
    /// All of `Memory`, as last read.
    root: RwSignal<Option<Read>>,
}

impl Account {
    /// The account of `overseer`'s user, on `shard` first. Reads nothing yet.
    pub fn new(overseer: &Overseer, shard: Option<String>) -> Self {
        Self {
            session: StoredValue::new_local(Session {
                overseer: overseer.clone(),
                shards: overseer.world().shards().to_vec(),
                root_load: Load::default(),
                root_reads: 0,
            }),
            shard: RwSignal::new(shard),
            root: RwSignal::new(None),
        }
    }

    /// The session.
    pub fn overseer(self) -> Overseer {
        self.session.with_value(|session| session.overseer.clone())
    }

    /// The library's `Memory` of the session.
    pub fn memory(self) -> Memory {
        self.session.with_value(|session| session.overseer.memory())
    }

    /// Calls `f` with all of `Memory` on the current shard; `None` before it was read there.
    /// Tracks both.
    pub fn with_root<T>(self, f: impl FnOnce(Option<&Read>) -> T) -> T {
        self.shard.with(|shard| {
            self.root
                .with(|root| f(root.as_ref().filter(|root| root.shard == *shard)))
        })
    }

    /// Whether all of `Memory` on the current shard must be read (again) for a completion: no
    /// read runs, and the last one was for another shard or is older than `fresh`
    /// (`fresh_empty` when it gave no keys or failed).
    pub fn root_stale(self, fresh: Duration, fresh_empty: Duration) -> bool {
        let shard = self.shard.get_untracked();
        self.session.with_value(|session| {
            session
                .root_load
                .stale(shard.as_deref(), fresh, fresh_empty)
        })
    }

    /// Reads all of `Memory` on the current shard. In the `background`, the read first waits
    /// for the session's request budget. The read starts now; the future gives its result.
    ///
    /// # Errors
    ///
    /// The future returns the failure of the read.
    pub fn read_root(
        self,
        background: bool,
    ) -> impl Future<Output = Result<(), overseer::Error>> + use<> {
        let shard = self.shard.get_untracked();
        self.session.update_value(|session| {
            session.root_reads += 1;
            session.root_load.start(shard.as_deref());
        });
        let (read, overseer) = self
            .session
            .with_value(|session| (session.root_reads, session.overseer.clone()));
        async move {
            if background {
                session::wait_for_budget(&overseer, Priority::Background, 0).await;
            }
            let result = overseer.memory().get("", shard.as_deref()).await;
            let empty = !matches!(&result, Ok(MemoryValue::Json(value)) if has_keys(value));
            let latest = self.session.try_update_value(|session| {
                let latest = session.root_reads == read;
                if latest {
                    session.root_load.finish(empty);
                }
                latest
            });
            if latest != Some(true) {
                return result.map(drop);
            }
            let value = result?;
            self.root.try_set(Some(Read::now(shard, value)));
            Ok(())
        }
    }

    /// Changes all of `Memory` with `change`, when it was read on `shard`.
    pub fn update_root(self, shard: Option<&str>, change: impl FnOnce(&mut MemoryValue)) {
        self.root.try_update(|root| {
            if let Some(root) = root.as_mut().filter(|root| root.shard.as_deref() == shard) {
                change(&mut root.value);
            }
        });
    }
}

/// Whether `value` is an object with keys.
fn has_keys(value: &serde_json::Value) -> bool {
    value.as_object().is_some_and(|map| !map.is_empty())
}

/// The shard selector of `account` on a sharded server; nothing on a single world.
#[component]
pub fn ShardSelect(account: Account) -> impl IntoView {
    let shards = account.session.with_value(|session| session.shards.clone());
    (!shards.is_empty()).then(|| {
        let options = shards
            .into_iter()
            .map(|shard| {
                let value = shard.clone();
                let selected = move || {
                    account
                        .shard
                        .with(|current| current.as_deref() == Some(value.as_str()))
                };
                let label = shard.clone();
                view! { <option value=shard prop:selected=selected>{label}</option> }
            })
            .collect_view();
        view! {
            <select
                class=concatcp!(style::INPUT_SM, " min-w-0 max-w-32")
                aria-label="Shard"
                on:change=move |event| account.shard.set(Some(event_target_value(&event)))
            >
                {options}
            </select>
        }
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::Load;

    const FRESH: Duration = Duration::from_mins(5);
    const FRESH_EMPTY: Duration = Duration::ZERO;

    #[test]
    fn a_part_is_read_again_for_another_shard_or_once_old() {
        let mut load = Load::default();
        assert!(load.stale(None, FRESH, FRESH_EMPTY));
        load.start(Some("shard0"));
        // A read runs: no second one.
        assert!(!load.stale(Some("shard0"), FRESH, FRESH_EMPTY));
        assert!(!load.stale(Some("shard1"), FRESH, FRESH_EMPTY));
        load.finish(false);
        assert!(load.is_for(Some("shard0")));
        assert!(!load.stale(Some("shard0"), FRESH, FRESH_EMPTY));
        assert!(load.stale(Some("shard1"), FRESH, FRESH_EMPTY));
        // An empty result is old at once here.
        load.start(Some("shard0"));
        load.finish(true);
        assert!(load.stale(Some("shard0"), FRESH, FRESH_EMPTY));
    }
}
