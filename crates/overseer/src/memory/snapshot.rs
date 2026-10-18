//! The session's shared reads of Memory paths ([`MemorySnapshot`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use overseer_net::Priority;
use overseer_net::api::MemoryApi as _;
use overseer_net::model::memory::{MemoryChange, MemoryPath, MemoryValue};
use overseer_net::rt::SystemTime;
use serde_json::Value;
use tokio::sync::watch;

use crate::cache::{Freshness, Load};
use crate::{Error, Overseer};

/// A value read from a Memory path, shared by every handle of its [`MemorySnapshot`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryRead {
    /// The value at the path.
    pub value: MemoryValue,
    /// When the server sent the value. A confirmed write that changed the value later keeps
    /// the time of the read it changed.
    pub read_at: SystemTime,
    /// The number of this value in its snapshot: every new read and every confirmed write
    /// that changed it adds one; the first read is 1.
    pub revision: u64,
}

/// The value at one Memory path on one shard, shared by the whole session: every handle of
/// the same path and shard ([`Memory::snapshot`](super::Memory::snapshot)), in any window or
/// task, sees the same reads. Cheap to clone.
///
/// Nothing is read until [`Self::refresh`] or [`Self::refresh_stale`]. Reads that overlap
/// share one request. A write confirmed by
/// [`Memory::write_and_observe`](super::Memory::write_and_observe) changes the snapshots of
/// its path, of the paths above it and, when the written value tells it, of the paths below
/// it. The session keeps a snapshot while a handle of it exists.
#[derive(Clone)]
pub struct MemorySnapshot {
    entry: Arc<Entry>,
}

/// The resolved shard and the path of a snapshot.
type Key = (Option<String>, MemoryPath);

/// The snapshots of the session, by shard and path.
#[derive(Default)]
pub(crate) struct Snapshots {
    entries: Mutex<HashMap<Key, Weak<Entry>>>,
}

/// One snapshot.
struct Entry {
    overseer: Overseer,
    /// The resolved shard (`None` on single-world servers).
    shard: Option<String>,
    path: MemoryPath,
    load: Mutex<Load>,
    /// The latest read.
    sender: watch::Sender<Option<Arc<MemoryRead>>>,
    /// Held while a request runs: a read that waited for it uses that request's result.
    reading: tokio::sync::Mutex<()>,
    /// The number of reads that ended, and whether the latest one succeeded; also held while
    /// a value is published.
    ended: Mutex<(u64, bool)>,
}

impl Snapshots {
    /// The snapshot of `path` on the resolved `shard`; a new one when none exists.
    pub(crate) fn get(
        &self,
        overseer: &Overseer,
        shard: Option<String>,
        path: &MemoryPath,
    ) -> MemorySnapshot {
        let mut entries = lock(&self.entries);
        entries.retain(|_, entry| entry.strong_count() > 0);
        let key = (shard, path.clone());
        if let Some(entry) = entries.get(&key).and_then(Weak::upgrade) {
            return MemorySnapshot { entry };
        }
        let entry = Arc::new(Entry {
            overseer: overseer.clone(),
            shard: key.0.clone(),
            path: path.clone(),
            load: Mutex::new(Load::default()),
            sender: watch::Sender::new(None),
            reading: tokio::sync::Mutex::new(()),
            ended: Mutex::new((0, false)),
        });
        entries.insert(key, Arc::downgrade(&entry));
        MemorySnapshot { entry }
    }

    /// Puts the confirmed `change` of `path` on the resolved `shard` into the snapshots of the
    /// paths above and below it. The snapshot of `path` itself has the read that confirmed it.
    pub(crate) fn apply(&self, shard: Option<&str>, path: &MemoryPath, change: &MemoryChange) {
        let entries: Vec<Arc<Entry>> = lock(&self.entries)
            .iter()
            .filter(|((entry_shard, entry_path), _)| {
                entry_shard.as_deref() == shard && entry_path != path
            })
            .filter_map(|(_, entry)| entry.upgrade())
            .collect();
        for entry in entries {
            if let Some(keys) = path.keys_below(&entry.path) {
                let Some(latest) = entry.latest() else {
                    continue;
                };
                let mut value = latest.value.clone();
                if change.apply(&mut value, &keys) {
                    entry.publish(value, latest.read_at);
                }
            } else if let Some(keys) = entry.path.keys_below(path)
                && let Some(value) = change.below(&keys)
            {
                entry.publish(value, SystemTime::now());
            }
        }
    }
}

impl MemorySnapshot {
    /// The path.
    #[must_use]
    pub fn path(&self) -> &MemoryPath {
        &self.entry.path
    }

    /// The shard (`None` on single-world servers).
    #[must_use]
    pub fn shard(&self) -> Option<&str> {
        self.entry.shard.as_deref()
    }

    /// The latest read; `None` before the first one ended.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<MemoryRead>> {
        self.entry.latest()
    }

    /// The latest read, to await changes: `changed().await` returns at the next read or
    /// confirmed write. The receiver ends with the last handle of the snapshot: keep a handle
    /// while the receiver is in use.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Option<Arc<MemoryRead>>> {
        self.entry.sender.subscribe()
    }

    /// Whether a read must be done: no read runs, and the last one is older than `freshness`
    /// allows. A read that gave nothing (no value, an empty object or array, `null`) or failed
    /// uses [`Freshness::fresh_empty`].
    #[must_use]
    pub fn is_stale(&self, freshness: &Freshness) -> bool {
        lock(&self.entry.load).stale(freshness)
    }

    /// Whether a read runs (or waits for the request budget).
    #[must_use]
    pub fn is_reading(&self) -> bool {
        lock(&self.entry.load).running()
    }

    /// Reads the value at the path (`GET /api/user/memory`) and gives it to every handle. A
    /// read of priority `pace` first waits for the session's request budget
    /// ([`Client::wait_for_budget`](overseer_net::Client::wait_for_budget)); `None` is the
    /// user's own read, which goes at once. A request of the snapshot that ends while this read
    /// waits (for the budget or for that request) gives its value to this read too: no second
    /// request. On screeps.com, Memory reads count against a daily limit.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] after
    /// [`Overseer::shutdown`](crate::Overseer::shutdown), and [`Error::Net`] when the request
    /// fails or the value does not decode.
    pub async fn refresh(&self, pace: Option<Priority>) -> Result<Arc<MemoryRead>, Error> {
        let entry = &self.entry;
        let mut running = Running::start(entry);
        // A read that ends after this call started is new enough.
        let ended = lock(&entry.ended).0;
        if let Some(priority) = pace {
            entry.overseer.client().wait_for_budget(priority, 0).await;
        }
        let _reading = entry.reading.lock().await;
        let (now_ended, succeeded) = *lock(&entry.ended);
        if now_ended != ended
            && succeeded
            && let Some(latest) = entry.latest()
        {
            return Ok(latest);
        }
        let result = match entry.overseer.user_api() {
            Ok(api) => api
                .memory(entry.path.as_str(), entry.shard.as_deref())
                .await
                .map_err(Error::from),
            Err(error) => Err(error),
        };
        *lock(&entry.ended) = (now_ended + 1, result.is_ok());
        let empty = result.as_ref().is_ok_and(is_empty);
        running.finish(empty || result.is_err());
        Ok(entry.publish(result?, SystemTime::now()))
    }

    /// [`Self::refresh`] when [`Self::is_stale`]; else the latest read.
    ///
    /// # Errors
    ///
    /// As [`Self::refresh`].
    pub async fn refresh_stale(
        &self,
        freshness: &Freshness,
        pace: Option<Priority>,
    ) -> Result<Option<Arc<MemoryRead>>, Error> {
        if self.is_stale(freshness) {
            self.refresh(pace).await.map(Some)
        } else {
            Ok(self.latest())
        }
    }
}

impl Entry {
    fn latest(&self) -> Option<Arc<MemoryRead>> {
        self.sender.borrow().clone()
    }

    /// Gives `value`, read at `read_at`, to every handle as the next revision.
    fn publish(&self, value: MemoryValue, read_at: SystemTime) -> Arc<MemoryRead> {
        // One publish at a time: each revision is used once.
        let _publishing = lock(&self.ended);
        let revision = self
            .sender
            .borrow()
            .as_ref()
            .map_or(0, |latest| latest.revision)
            + 1;
        let read = Arc::new(MemoryRead {
            value,
            read_at,
            revision,
        });
        self.sender.send_replace(Some(Arc::clone(&read)));
        read
    }
}

/// A running read of an entry: counted in its [`Load`] until it ends or is dropped.
struct Running<'a> {
    entry: &'a Entry,
    ended: bool,
}

impl<'a> Running<'a> {
    fn start(entry: &'a Entry) -> Self {
        lock(&entry.load).start();
        Self {
            entry,
            ended: false,
        }
    }

    fn finish(&mut self, empty: bool) {
        lock(&self.entry.load).finish(empty);
        self.ended = true;
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if !self.ended {
            lock(&self.entry.load).cancel();
        }
    }
}

/// Whether a read gave nothing: no value, `null`, or an empty object or array.
fn is_empty(value: &MemoryValue) -> bool {
    match value {
        MemoryValue::Json(Value::Object(map)) => map.is_empty(),
        MemoryValue::Json(Value::Array(items)) => items.is_empty(),
        MemoryValue::Json(Value::Null) | MemoryValue::Undefined | MemoryValue::IncorrectPath => {
            true
        }
        MemoryValue::Json(_) => false,
    }
}

/// Locks a mutex whose critical sections cannot panic halfway through an update.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
