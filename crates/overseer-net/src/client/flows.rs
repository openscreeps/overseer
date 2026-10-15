//! Everyday flows on top of [`Client::send`]: default shards, decoding and caching.
//!
//! Each helper names the [`World`](crate::World)'s default shard when `shard` is `None`
//! (official servers need one; single-world servers never send one) and returns decoded data
//! where the raw reply needs work.

use std::sync::Arc;

use overseer_core::api::{MemoryValue, Terrain, endpoints, types};

use super::{Client, lock};
use crate::{Error, RoomHistory};

impl Client {
    /// The decoded terrain of `room`, fetched once per `(shard, room)` and then served from the
    /// client's cache (terrain never changes). Uses `GET /api/game/room-terrain?encoded=1`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`], [`Error::Terrain`] for a malformed terrain and
    /// [`Error::UnexpectedReply`] when the server returns no encoded terrain document.
    pub async fn terrain(&self, shard: Option<&str>, room: &str) -> Result<Arc<Terrain>, Error> {
        let shard = self.resolve_shard(shard).await?;
        let key = (shard, room.to_owned());
        if let Some(terrain) = lock(&self.inner.terrain).get(&key) {
            return Ok(Arc::clone(terrain));
        }
        let reply = self
            .send(&endpoints::GameRoomTerrain {
                room: room.to_owned(),
                encoded: Some("1".to_owned()),
                shard: key.0.clone(),
            })
            .await?
            .body;
        let types::GameRoomTerrainResponse::Encoded(encoded) = reply else {
            return Err(Error::UnexpectedReply(
                "terrain tiles instead of encoded terrain",
            ));
        };
        let document = encoded
            .terrain
            .first()
            .ok_or(Error::UnexpectedReply("no terrain document"))?;
        let terrain = Arc::new(document.decode()?);
        lock(&self.inner.terrain).insert(key, Arc::clone(&terrain));
        Ok(terrain)
    }

    /// A snapshot of every object in `room` (typed by the reference) and the public profiles of
    /// the users they reference (`GET /api/game/room-objects`). For live updates subscribe to
    /// the room on a [`Socket`](crate::Socket) instead.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn room_objects(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> Result<types::GameRoomObjectsResponse, Error> {
        let shard = self.resolve_shard(shard).await?;
        let room = room.to_owned();
        Ok(self
            .send(&endpoints::GameRoomObjects { room, shard })
            .await?
            .body)
    }

    /// The signed-in user's Memory at the dot-separated `path` (`None`: all of Memory), decoded
    /// from the `gz:` form (`GET /api/user/memory`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`] and [`Error::Memory`] for a value that does not
    /// decode.
    pub async fn memory(
        &self,
        shard: Option<&str>,
        path: Option<&str>,
    ) -> Result<MemoryValue, Error> {
        let shard = self.resolve_shard(shard).await?;
        let path = path.map(str::to_owned);
        let reply = self.send(&endpoints::UserMemoryGet { path, shard }).await?;
        Ok(reply.body.value()?)
    }

    /// Queues the assignment of `value` at the dot-separated `path` in the signed-in user's
    /// Memory (`POST /api/user/memory`). An empty `path` clears Memory and merges an object
    /// `value` into it. The write runs in the player's runtime after a later tick, not before
    /// this returns.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn set_memory(
        &self,
        shard: Option<&str>,
        path: &str,
        value: serde_json::Value,
    ) -> Result<(), Error> {
        self.write_memory(shard, path, Some(value)).await
    }

    /// Queues the deletion of the value at the dot-separated `path` in the signed-in user's
    /// Memory (`POST /api/user/memory` without a value). An empty `path` clears Memory.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn delete_memory(&self, shard: Option<&str>, path: &str) -> Result<(), Error> {
        self.write_memory(shard, path, None).await
    }

    /// Queues a console expression for the signed-in user's runtime (`POST /api/user/console`).
    /// Its output and result arrive on the console channel
    /// ([`Socket::subscribe_console`](crate::Socket::subscribe_console)), not in the reply.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`]; an expression whose JSON form exceeds 1024
    /// characters is rejected by the server.
    pub async fn console(
        &self,
        shard: Option<&str>,
        expression: impl Into<String>,
    ) -> Result<(), Error> {
        let shard = self.resolve_shard(shard).await?;
        let body = types::UserConsoleRequest {
            expression: expression.into(),
            shard,
        };
        self.send(&endpoints::UserConsole { body }).await?;
        Ok(())
    }

    /// World-map statistics of `rooms` (`POST /api/game/map-stats`): status, owner, sign, safe
    /// mode and mineral per room, plus the ranking of `stat_name` and the referenced users.
    /// `stat_name` is `owner0` for ownership only, or a statistic suffixed with an interval such
    /// as `energyHarvested8`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn map_stats<R: Into<String>>(
        &self,
        shard: Option<&str>,
        rooms: impl IntoIterator<Item = R>,
        stat_name: &str,
    ) -> Result<types::GameMapStatsResponse, Error> {
        let shard = self.resolve_shard(shard).await?;
        let body = types::GameMapStatsRequest {
            rooms: rooms.into_iter().map(Into::into).collect(),
            shard,
            stat_name: stat_name.to_owned(),
        };
        Ok(self.send(&endpoints::GameMapStats { body }).await?.body)
    }

    /// The signed-in user's code branches, without module contents (`GET /api/user/branches`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn branches(&self) -> Result<Vec<types::UserCodeBranch>, Error> {
        let branches = endpoints::UserBranches {
            with_code: Some(false),
        };
        Ok(self.send(&branches).await?.body.list)
    }

    /// The modules of `branch` (`None`: the branch active in the world; `$activeSim` names the
    /// simulation branch) via `GET /api/user/code`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`]; the server answers `no code` for an unknown branch.
    pub async fn code(&self, branch: Option<&str>) -> Result<types::UserCodeGetResponse, Error> {
        let branch = branch.map(str::to_owned);
        Ok(self.send(&endpoints::UserCodeGet { branch }).await?.body)
    }

    /// Replaces every module of the existing `branch` (`None`: the branch active in the world)
    /// via `POST /api/user/code`; code of the world branch runs from a later tick on.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`]; the server rejects unknown branches and modules
    /// over 5 MiB.
    pub async fn set_code(
        &self,
        branch: Option<&str>,
        modules: types::UserCodeModules,
    ) -> Result<(), Error> {
        let body = types::UserCodeSetRequest {
            hash: None,
            branch: branch.map(str::to_owned),
            modules,
        };
        self.send(&endpoints::UserCodeSet { body }).await?;
        Ok(())
    }

    /// The signed-in user's statistics overview (`GET /api/user/overview`): per-room series of
    /// `stat` over `interval` minutes (8, 180 or 1440; the server's default when `None`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn overview(
        &self,
        interval: Option<u64>,
        stat: Option<types::UserRoomStatName>,
    ) -> Result<types::UserOverviewResponse, Error> {
        let overview = endpoints::UserOverview {
            interval,
            stat_name: stat,
        };
        Ok(self.send(&overview).await?.body)
    }

    /// The room-history chunk of `room` that holds `tick`, ready to [replay](RoomHistory::replay).
    ///
    /// Chunks start at multiples of [`World::history_chunk_size`](crate::World::history_chunk_size).
    /// Sharded (official) servers serve them as static files
    /// (`GET /room-history/{shard}/{room}/{tick}.json`), single-world servers through
    /// `GET /room-history?room&time`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`] (a chunk that is not kept yields an HTTP 404
    /// [`ApiError::Status`](overseer_core::api::ApiError::Status)) and
    /// [`Error::UnexpectedReply`] for a chunk whose ticks are not numbers.
    pub async fn room_history(
        &self,
        shard: Option<&str>,
        room: &str,
        tick: u64,
    ) -> Result<RoomHistory, Error> {
        let world = self.world().await?;
        let base = tick - tick % world.history_chunk_size();
        let room = room.to_owned();
        let chunk = if let Some(shard) = world.resolve_shard(shard) {
            let chunk = endpoints::PlatformOfficialRoomHistory {
                shard,
                room: room.clone(),
                tick: base,
            };
            self.send(&chunk).await?.body
        } else {
            let chunk = endpoints::PlatformRoomHistory {
                room: room.clone(),
                time: base,
            };
            self.send(&chunk).await?.body
        };
        RoomHistory::new(chunk, room)
    }

    async fn write_memory(
        &self,
        shard: Option<&str>,
        path: &str,
        value: Option<serde_json::Value>,
    ) -> Result<(), Error> {
        let shard = self.resolve_shard(shard).await?;
        let body = types::UserMemorySetRequest {
            memory: None,
            path: Some(path.to_owned()),
            shard,
            value,
        };
        self.send(&endpoints::UserMemorySet { body }).await?;
        Ok(())
    }
}
