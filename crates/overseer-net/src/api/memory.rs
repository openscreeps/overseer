//! Memory area: the signed-in user's Memory and Memory segments.
//!
//! Every contract defines these operations identically (same paths, requests and replies), so
//! the per-contract impls differ only in the generated module they send through. Each names
//! the [`World`](crate::World)'s default shard when `shard` is `None` (official servers need
//! one; single-world servers never send one).

use std::borrow::Cow;

use overseer_core::api::{community, official, openscreeps};
use overseer_core::model::memory::{MemoryValue, segment_data};

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::Error;

/// The signed-in user's Memory, read and written by dot-separated path
/// (`creeps.Harvester1.role`) per shard, and its segments (`RawMemory.segments`, ids
/// `0..=`[`MAX_SEGMENT`](overseer_core::model::memory::MAX_SEGMENT)). Memory writes are
/// queued: the player's runtime applies them after a later tick, not before they return.
pub trait MemoryApi {
    /// The value at `path` (`GET /api/user/memory`), decoded from its `gz:` form; an empty
    /// `path` reads all of Memory.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send) and
    /// [`Error::Memory`] for a value that does not decode.
    fn memory(
        &self,
        path: &str,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<MemoryValue, Error>>;

    /// Queues the assignment of `value` at `path` (`POST /api/user/memory`). An empty `path`
    /// clears Memory and merges an object `value` into it.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn set_memory(
        &self,
        path: &str,
        value: serde_json::Value,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Queues the deletion of the value at `path` (`POST /api/user/memory` without a value).
    /// An empty `path` clears Memory.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn delete_memory(
        &self,
        path: &str,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<(), Error>>;

    /// The contents of segment `id` (`GET /api/user/memory-segment`); `None` for a segment
    /// never written.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); the server rejects ids
    /// above [`MAX_SEGMENT`](overseer_core::model::memory::MAX_SEGMENT).
    fn memory_segment(
        &self,
        id: u8,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<Option<String>, Error>>;

    /// Replaces the contents of segment `id` with `data` (`POST /api/user/memory-segment`),
    /// immediately rather than queued.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send); the server rejects ids
    /// above [`MAX_SEGMENT`](overseer_core::model::memory::MAX_SEGMENT) and `data` over
    /// 100 KiB.
    fn set_memory_segment(
        &self,
        id: u8,
        data: &str,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<(), Error>>;
}

impl MemoryApi for Api {
    async fn memory(&self, path: &str, shard: Option<&str>) -> Result<MemoryValue, Error> {
        delegate!(self, api => api.memory(path, shard).await)
    }

    async fn set_memory(
        &self,
        path: &str,
        value: serde_json::Value,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        delegate!(self, api => api.set_memory(path, value, shard).await)
    }

    async fn delete_memory(&self, path: &str, shard: Option<&str>) -> Result<(), Error> {
        delegate!(self, api => api.delete_memory(path, shard).await)
    }

    async fn memory_segment(&self, id: u8, shard: Option<&str>) -> Result<Option<String>, Error> {
        delegate!(self, api => api.memory_segment(id, shard).await)
    }

    async fn set_memory_segment(
        &self,
        id: u8,
        data: &str,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        delegate!(self, api => api.set_memory_segment(id, data, shard).await)
    }
}

/// `impl MemoryApi for $api`, sending the endpoints of the contract module `$contract`.
macro_rules! memory_api {
    ($($api:ty => $contract:ident),+ $(,)?) => {$(
        impl MemoryApi for $api {
            async fn memory(&self, path: &str, shard: Option<&str>) -> Result<MemoryValue, Error> {
                let read = $contract::endpoints::UserMemoryGet {
                    path: (!path.is_empty()).then(|| path.to_owned()),
                    shard: self.client.resolve_shard(shard).await?,
                };
                Ok(self.client.send(&read).await?.body.try_into()?)
            }

            async fn set_memory(
                &self,
                path: &str,
                value: serde_json::Value,
                shard: Option<&str>,
            ) -> Result<(), Error> {
                write_memory!(self, $contract, path, Some(value), shard)
            }

            async fn delete_memory(&self, path: &str, shard: Option<&str>) -> Result<(), Error> {
                write_memory!(self, $contract, path, None, shard)
            }

            async fn memory_segment(
                &self,
                id: u8,
                shard: Option<&str>,
            ) -> Result<Option<String>, Error> {
                let read = $contract::endpoints::UserMemorySegmentGet {
                    segment: id.to_string(),
                    shard: self.client.resolve_shard(shard).await?,
                };
                Ok(segment_data(self.client.send(&read).await?.body))
            }

            async fn set_memory_segment(
                &self,
                id: u8,
                data: &str,
                shard: Option<&str>,
            ) -> Result<(), Error> {
                let body = $contract::types::UserMemorySegmentSetRequest {
                    data: Cow::Owned($contract::types::UserMemorySegmentSetRequestData::String(
                        data.to_owned(),
                    )),
                    segment: Cow::Owned(
                        $contract::types::UserMemorySegmentSetRequestSegment::Integer(id.into()),
                    ),
                    shard: self.client.resolve_shard(shard).await?.map(Into::into),
                };
                self.client
                    .send(&$contract::endpoints::UserMemorySegmentSet { body })
                    .await?;
                Ok(())
            }
        }
    )+};
}

/// Sends `POST /api/user/memory` of `$contract` assigning `$value` (`None`: deleting) at
/// `$path`.
macro_rules! write_memory {
    ($self:ident, $contract:ident, $path:expr, $value:expr, $shard:expr) => {{
        let body = $contract::types::UserMemorySetRequest {
            memory: None,
            path: Some($path.into()),
            shard: $self.client.resolve_shard($shard).await?.map(Into::into),
            value: $value.map(Cow::Owned),
        };
        $self
            .client
            .send(&$contract::endpoints::UserMemorySet { body })
            .await?;
        Ok(())
    }};
}

memory_api!(Official => official, OpenScreeps => openscreeps, Community => community);
