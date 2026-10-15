//! The signed-in user's Memory and Memory segments.
//!
//! Memory is read and written by dot-separated path (`creeps.Harvester1.role`) per shard.
//! Writes are queued: the player's runtime applies them after a later tick, not before the
//! method returns. Segments are the 100 raw strings of `RawMemory.segments`.

use overseer_net::core::api::types::{
    UserMemorySegmentGetResponseData, UserMemorySegmentSetRequest, UserMemorySegmentSetRequestData,
    UserMemorySegmentSetRequestSegment,
};
use overseer_net::core::api::{MemoryValue, endpoints};
use overseer_net::core::socket::{Channel, Payload};

use crate::{Error, Overseer, Watch};

/// The highest Memory segment id; segments are numbered `0..=MAX_SEGMENT`.
pub const MAX_SEGMENT: u8 = 99;

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

    /// The value at `path` (`GET /api/user/memory`); an empty `path` reads all of Memory.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails or the value does not decode.
    pub async fn get(&self, path: &str, shard: Option<&str>) -> Result<MemoryValue, Error> {
        self.ready()?;
        let path = (!path.is_empty()).then_some(path);
        Ok(self.overseer.client().memory(shard, path).await?)
    }

    /// Queues the assignment of `value` at `path` (`POST /api/user/memory`). An empty `path`
    /// clears Memory and merges an object `value` into it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails.
    pub async fn set(
        &self,
        path: &str,
        value: serde_json::Value,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        self.ready()?;
        Ok(self
            .overseer
            .client()
            .set_memory(shard, path, value)
            .await?)
    }

    /// Queues the deletion of the value at `path` (`POST /api/user/memory` without a value).
    /// An empty `path` clears Memory.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the request fails.
    pub async fn delete(&self, path: &str, shard: Option<&str>) -> Result<(), Error> {
        self.ready()?;
        Ok(self.overseer.client().delete_memory(shard, path).await?)
    }

    /// The contents of segment `id` (`GET /api/user/memory-segment`); `None` for a segment
    /// never written.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for an `id` above [`MAX_SEGMENT`] and [`Error::Net`]
    /// when the request fails.
    pub async fn segment(&self, id: u8, shard: Option<&str>) -> Result<Option<String>, Error> {
        check_segment(id)?;
        let request = endpoints::UserMemorySegmentGet {
            segment: id.to_string(),
            shard: self.overseer.resolve_shard(shard),
        };
        Ok(match self.overseer.send(&request).await?.data {
            Some(UserMemorySegmentGetResponseData::String(data)) => Some(data),
            // screeps.com answers a list in request order.
            Some(UserMemorySegmentGetResponseData::List(list)) => list.into_iter().next().flatten(),
            None => None,
        })
    }

    /// Replaces the contents of segment `id` with `data` (`POST /api/user/memory-segment`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for an `id` above [`MAX_SEGMENT`] and [`Error::Net`]
    /// when the request fails; the server rejects `data` over 100 KiB.
    pub async fn set_segment(&self, id: u8, data: &str, shard: Option<&str>) -> Result<(), Error> {
        check_segment(id)?;
        let body = UserMemorySegmentSetRequest {
            data: UserMemorySegmentSetRequestData::String(data.to_owned()),
            segment: UserMemorySegmentSetRequestSegment::Integer(id.into()),
            shard: self.overseer.resolve_shard(shard),
        };
        self.overseer
            .send(&endpoints::UserMemorySegmentSet { body })
            .await?;
        Ok(())
    }

    /// Watches the value at `path` (`user:<user>/memory/…`): after every run of the player's
    /// code the server sends its JavaScript string form (`String(value)`: `42`, `text`,
    /// `[object Object]` for objects, `undefined` when absent, `Incorrect memory path` when the
    /// path cannot be traversed). Every [`Watch`] of a path shares one subscription.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for an empty `path`.
    pub fn watch(&self, path: &str, shard: Option<&str>) -> Result<Watch<String>, Error> {
        let user = self.ready()?.to_owned();
        if path.is_empty() {
            return Err(Error::InvalidInput(
                "an empty Memory path to watch".to_owned(),
            ));
        }
        let channel = Channel::Memory {
            user,
            shard: self.overseer.resolve_shard(shard),
            path: path.to_owned(),
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

/// Rejects segment ids above [`MAX_SEGMENT`].
fn check_segment(id: u8) -> Result<(), Error> {
    if id > MAX_SEGMENT {
        return Err(Error::InvalidInput(format!(
            "segment {id} is above {MAX_SEGMENT}"
        )));
    }
    Ok(())
}
