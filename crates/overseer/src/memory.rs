//! The signed-in user's Memory and Memory segments.
//!
//! Memory is read and written by [`MemoryPath`] (`creeps.Harvester1.role`) per shard. Writes
//! are queued: the player's runtime applies them after a later tick, not before the method
//! returns. [`Memory::write_and_observe`] queues a write and reads the path again until the
//! change shows. Segments are the 100 raw strings of `RawMemory.segments`, each at most
//! [`SEGMENT_MAX_UNITS`] long.
//!
//! [`Memory::snapshot`] gives the session's shared read of a path ([`MemorySnapshot`]): every
//! window and task of the session that shows a path reads it through one snapshot, and a
//! confirmed write shows in all of them.

mod snapshot;
mod write;

use overseer_net::api::MemoryApi as _;
use overseer_net::core::socket::{Channel, Payload};

pub use overseer_net::model::memory::{
    MAX_SEGMENT, MemoryChange, MemoryPath, MemoryPathError, MemoryValue, SEGMENT_MAX_UNITS,
    SegmentError, json_at, segment_units,
};
use overseer_net::model::memory::{check_segment, check_segment_data};
pub(crate) use snapshot::Snapshots;
pub use snapshot::{MemoryRead, MemorySnapshot};
pub use write::{MAX_OBSERVE_READS, ObservePolicy, WriteProgress};

use crate::{Error, Overseer, Watch};

/// The signed-in user's Memory and Memory segments.
///
/// Created by [`Overseer::memory`]; cheap to clone. Every method takes the `shard` to use
/// (`None`: the default shard; ignored by single-world servers), needs a signed-in user and
/// fails with [`Error::Anonymous`] otherwise, and with [`Error::Closed`] after
/// [`Overseer::shutdown`].
#[derive(Clone, Debug)]
pub struct Memory {
    overseer: Overseer,
}

impl Memory {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// The value at `path` (`GET /api/user/memory`), read now and not shared; [`Self::snapshot`]
    /// shares reads with the rest of the session.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails or the value does not decode.
    pub async fn get(&self, path: &MemoryPath, shard: Option<&str>) -> Result<MemoryValue, Error> {
        Ok(self
            .overseer
            .user_api()?
            .memory(path.as_str(), shard)
            .await?)
    }

    /// The session's shared read of the value at `path` on `shard`. Reads nothing: see
    /// [`MemorySnapshot::refresh`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] and [`Error::Closed`] only.
    pub fn snapshot(
        &self,
        path: &MemoryPath,
        shard: Option<&str>,
    ) -> Result<MemorySnapshot, Error> {
        self.ready()?;
        let shard = self.overseer.resolve_shard(shard);
        Ok(self
            .overseer
            .memory_snapshots()
            .get(&self.overseer, shard, path))
    }

    /// Queues the assignment of `value` at `path` (`POST /api/user/memory`). The root path
    /// clears Memory and merges an object `value` into it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails.
    pub async fn set(
        &self,
        path: &MemoryPath,
        value: serde_json::Value,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        Ok(self
            .overseer
            .user_api()?
            .set_memory(path.as_str(), value, shard)
            .await?)
    }

    /// Queues the deletion of the value at `path` (`POST /api/user/memory` without a value).
    /// The root path clears Memory.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails.
    pub async fn delete(&self, path: &MemoryPath, shard: Option<&str>) -> Result<(), Error> {
        Ok(self
            .overseer
            .user_api()?
            .delete_memory(path.as_str(), shard)
            .await?)
    }

    /// The contents of segment `id` (`GET /api/user/memory-segment`); `None` for a segment
    /// never written.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Segment`] for an `id` above [`MAX_SEGMENT`] and [`Error::Net`] when
    /// the request fails.
    pub async fn segment(&self, id: u8, shard: Option<&str>) -> Result<Option<String>, Error> {
        check_segment(id)?;
        Ok(self.overseer.user_api()?.memory_segment(id, shard).await?)
    }

    /// Replaces the contents of segment `id` with `data` (`POST /api/user/memory-segment`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Segment`] for an `id` above [`MAX_SEGMENT`] or `data` longer than
    /// [`SEGMENT_MAX_UNITS`] (nothing is sent), and [`Error::Net`] when the request fails.
    pub async fn set_segment(&self, id: u8, data: &str, shard: Option<&str>) -> Result<(), Error> {
        check_segment_data(id, data)?;
        Ok(self
            .overseer
            .user_api()?
            .set_memory_segment(id, data, shard)
            .await?)
    }

    /// Watches the value at `path` (`user:<user>/memory/…`): after every run of the player's
    /// code the server sends its JavaScript string form (`String(value)`: `42`, `text`,
    /// `[object Object]` for objects, `undefined` when absent, `Incorrect memory path` when the
    /// path cannot be traversed). Every [`Watch`] of a path shares one subscription.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for the root path.
    pub fn watch(&self, path: &MemoryPath, shard: Option<&str>) -> Result<Watch<String>, Error> {
        let user = self.ready()?.to_owned();
        if path.is_root() {
            return Err(Error::InvalidInput(
                "an empty Memory path to watch".to_owned(),
            ));
        }
        let channel = Channel::Memory {
            user,
            shard: self.overseer.resolve_shard(shard),
            path: path.as_str().to_owned(),
        };
        Ok(self.overseer.watch(channel, |payload| match payload {
            Payload::Memory(value) => Some(value),
            _ => None,
        }))
    }

    /// The signed-in user's id once the session is open.
    fn ready(&self) -> Result<&str, Error> {
        self.overseer.ensure_open()?;
        self.overseer.require_user()
    }
}
