//! Everyday flows on top of the [domain API](crate::api): terrain, decoded once per room and
//! cached for the client's lifetime (terrain never changes).
//!
//! Each helper names the [`World`](crate::World)'s default shard when `shard` is `None`
//! (official servers need one; single-world servers never send one).

use std::collections::BTreeMap;
use std::sync::Arc;

use overseer_core::model::world::Terrain;

use super::{Client, lock};
use crate::Error;
use crate::api::WorldApi as _;

impl Client {
    /// The decoded terrain of `room`, fetched once per `(shard, room)` and then served from the
    /// client's cache (terrain never changes). Uses [`WorldApi::room_terrain`]
    /// (`GET /api/game/room-terrain?encoded=1`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`] and [`Error::Terrain`] for a malformed terrain or a
    /// reply without terrain (an unknown room).
    ///
    /// [`WorldApi::room_terrain`]: crate::api::WorldApi::room_terrain
    pub async fn terrain(&self, shard: Option<&str>, room: &str) -> Result<Arc<Terrain>, Error> {
        let shard = self.resolve_shard(shard).await?;
        let key = (shard, room.to_owned());
        if let Some(terrain) = lock(&self.inner.terrain).get(&key) {
            return Ok(Arc::clone(terrain));
        }
        let terrain = self
            .api()
            .await?
            .room_terrain(key.0.as_deref(), room)
            .await?;
        let terrain = Arc::new(terrain);
        lock(&self.inner.terrain).insert(key, Arc::clone(&terrain));
        Ok(terrain)
    }

    /// The decoded terrain of each of `rooms`, by room name, sharing the cache of
    /// [`Self::terrain`]: rooms not cached yet are fetched with one [`WorldApi::terrains`]
    /// (`POST /api/game/rooms`). Rooms the server does not know are absent from the result.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`] and [`Error::Terrain`] for a malformed terrain.
    ///
    /// [`WorldApi::terrains`]: crate::api::WorldApi::terrains
    pub async fn terrains<R: Into<String>>(
        &self,
        shard: Option<&str>,
        rooms: impl IntoIterator<Item = R>,
    ) -> Result<BTreeMap<String, Arc<Terrain>>, Error> {
        let shard = self.resolve_shard(shard).await?;
        let mut terrains = BTreeMap::new();
        let mut missing = Vec::new();
        {
            let cache = lock(&self.inner.terrain);
            for room in rooms {
                let room = room.into();
                match cache.get(&(shard.clone(), room.clone())) {
                    Some(terrain) => {
                        terrains.insert(room, Arc::clone(terrain));
                    }
                    None => missing.push(room),
                }
            }
        }
        missing.sort_unstable();
        missing.dedup();
        if missing.is_empty() {
            return Ok(terrains);
        }
        let decoded = self
            .api()
            .await?
            .terrains(shard.as_deref(), missing)
            .await?;
        let mut cache = lock(&self.inner.terrain);
        for (room, terrain) in decoded {
            let terrain = Arc::new(terrain);
            cache.insert((shard.clone(), room.clone()), Arc::clone(&terrain));
            terrains.insert(room, terrain);
        }
        Ok(terrains)
    }
}
