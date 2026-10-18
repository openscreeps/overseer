//! What the console and the Memory tab of one session share ([`Account`]): the shard that they
//! act on, and all of `Memory` on that shard.
//!
//! The drawer makes one [`Account`] for its Console and Memory tabs; the console window makes
//! its own. All of `Memory` is the library's session snapshot
//! ([`overseer::Memory::snapshot`]): the tabs and the windows of the session read it, a read
//! of one serves the others, and a write that the Memory tab confirmed shows in the console's
//! completion. The account only follows that snapshot into a signal.

use std::sync::Arc;

use futures_util::future::{AbortHandle, abortable};
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::cache::Freshness;
use overseer::memory::{MemoryPath, MemoryRead, MemorySnapshot};
use overseer::net::Priority;
use overseer::{Memory, Overseer};

use crate::ui::style::{self, concatcp};

/// What an [`Account`] holds out of line: the session, the server's shards, and the task that
/// follows all of `Memory` on the shard.
struct Session {
    /// The session.
    overseer: Overseer,
    /// The server's shards (empty on single-world servers).
    shards: Vec<String>,
    /// Stops following the snapshot of all of `Memory` of the previous shard.
    follow: Option<AbortHandle>,
}

/// All of `Memory` as last read on a shard.
struct Root {
    /// The shard (`None`: the server's only world).
    shard: Option<String>,
    /// The read.
    read: Arc<MemoryRead>,
}

/// The session, the shard and all of `Memory` there, for the console and the Memory tab;
/// `Copy`.
#[derive(Clone, Copy)]
pub struct Account {
    /// The session, the shards and the task that follows [`Self::root`].
    session: StoredValue<Session, LocalStorage>,
    /// The shard to act on (`None`: the server's only world).
    pub shard: RwSignal<Option<String>>,
    /// All of `Memory`, as the session's snapshot last had it.
    root: RwSignal<Option<Root>, LocalStorage>,
}

impl Account {
    /// The account of `overseer`'s user, on `shard` first. Reads nothing; follows the session's
    /// snapshot of all of `Memory` on the shard.
    pub fn new(overseer: &Overseer, shard: Option<String>) -> Self {
        let account = Self {
            session: StoredValue::new_local(Session {
                overseer: overseer.clone(),
                shards: overseer.world().shards().to_vec(),
                follow: None,
            }),
            shard: RwSignal::new(shard),
            root: RwSignal::new_local(None),
        };
        Effect::new(move |_| {
            let shard = account.shard.get();
            let follow = account.root_snapshot().map(|snapshot| {
                follow(snapshot, move |read| {
                    let root = read.map(|read| Root {
                        shard: shard.clone(),
                        read,
                    });
                    account.root.try_set(root).is_none()
                })
            });
            account.session.update_value(|session| {
                if let Some(previous) = std::mem::replace(&mut session.follow, follow) {
                    previous.abort();
                }
            });
        });
        on_cleanup(move || {
            account.session.try_update_value(|session| {
                if let Some(follow) = session.follow.take() {
                    follow.abort();
                }
            });
        });
        account
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
    pub fn with_root<T>(self, f: impl FnOnce(Option<&MemoryRead>) -> T) -> T {
        self.shard.with(|shard| {
            self.root.with(|root| {
                f(root
                    .as_ref()
                    .filter(|root| root.shard == *shard)
                    .map(|root| &*root.read))
            })
        })
    }

    /// The session's snapshot of all of `Memory` on the current shard; `None` once the session
    /// is closed.
    fn root_snapshot(self) -> Option<MemorySnapshot> {
        let shard = self.shard.get_untracked();
        self.memory()
            .snapshot(&MemoryPath::root(), shard.as_deref())
            .ok()
    }

    /// Takes the latest read of all of `Memory` on the current shard from the session's
    /// snapshot now, before the task that follows it does: a view that the library just
    /// changed reads the change at once.
    pub fn sync_root(self) {
        if let Some(snapshot) = self.root_snapshot() {
            let shard = self.shard.get_untracked();
            self.root
                .try_set(snapshot.latest().map(|read| Root { shard, read }));
        }
    }

    /// Reads all of `Memory` on the current shard ([`MemorySnapshot::refresh`]): at once for
    /// `None`, else after the request budget of `pace`.
    ///
    /// # Errors
    ///
    /// The future returns the failure of the read.
    pub fn refresh_root(
        self,
        pace: Option<Priority>,
    ) -> impl Future<Output = Result<(), overseer::Error>> + use<> {
        let snapshot = self.root_snapshot();
        async move {
            match snapshot {
                Some(snapshot) => snapshot.refresh(pace).await.map(drop),
                None => Err(overseer::Error::Closed),
            }
        }
    }

    /// Reads all of `Memory` on the current shard in the background when no read runs and
    /// the last one is older than `freshness` allows. A failure leaves the last read; the next
    /// call after [`Freshness::fresh_empty`] reads again.
    pub fn refresh_root_stale(self, freshness: &Freshness) {
        let Some(snapshot) = self.root_snapshot() else {
            return;
        };
        if snapshot.is_stale(freshness) {
            spawn_local(async move {
                let _ = snapshot.refresh(Some(Priority::Background)).await;
            });
        }
    }
}

/// Gives the latest read of `snapshot`, then every later one, to `set` until it returns
/// `false` or the returned handle aborts. The task keeps the snapshot.
pub fn follow(
    snapshot: MemorySnapshot,
    mut set: impl FnMut(Option<Arc<MemoryRead>>) -> bool + 'static,
) -> AbortHandle {
    let (task, handle) = abortable(async move {
        if !set(snapshot.latest()) {
            return;
        }
        let mut changes = snapshot.watch();
        changes.borrow_and_update();
        while changes.changed().await.is_ok() {
            let read = changes.borrow_and_update().clone();
            if !set(read) {
                break;
            }
        }
    });
    spawn_local(async move {
        // `Err` only says it was aborted.
        let _ = task.await;
    });
    handle
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
