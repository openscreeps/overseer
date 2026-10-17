//! The room objects subsystem
//! ([`Subsystem::RoomObjects`](crate::model::platform::Subsystem::RoomObjects)): the
//! [`RoomObjectsSource`] trait, its implementations and [`Api::room_objects_source`], which
//! selects the implementation.

use overseer_core::api::plugins::screepsmod_admin_utils;
use overseer_core::api::{official, openscreeps};
use overseer_core::model::world::RoomObjects;

use super::super::{Api, Official, OpenScreeps, ScreepsmodAdminUtils};
use crate::Error;

/// Every object of a room with the users they reference (`GET /api/game/room-objects`;
/// [`Subsystem::RoomObjects`](crate::model::platform::Subsystem::RoomObjects)): native on
/// screeps.com and `OpenScreeps`, provided by screepsmod-admin-utils on community servers (the
/// stock server has no such route). From
/// [`Api::room_objects_source`].
pub trait RoomObjectsSource {
    /// A snapshot of every object in `room` and the public profiles of the users they
    /// reference (`GET /api/game/room-objects`); `shard` as in [`WorldApi`](super::WorldApi).
    /// Flags are not included.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn room_objects(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> impl Future<Output = Result<RoomObjects, Error>>;
}

/// The API of a server offering [`RoomObjectsSource`]; from [`Api::room_objects_source`].
#[derive(Clone, Copy, Debug)]
pub enum RoomObjectsSourceApi<'a> {
    /// Native on screeps.com.
    Official(&'a Official),
    /// Native on `OpenScreeps`.
    OpenScreeps(&'a OpenScreeps),
    /// screepsmod-admin-utils on a community server.
    ScreepsmodAdminUtils(ScreepsmodAdminUtils<'a>),
}

impl Api {
    /// The room objects of a room ([`RoomObjectsSource`]), when the server offers them
    /// ([`Subsystem::RoomObjects`](crate::model::platform::Subsystem::RoomObjects)):
    /// screeps.com, `OpenScreeps`, and community servers with screepsmod-admin-utils.
    #[must_use]
    pub fn room_objects_source(&self) -> Option<RoomObjectsSourceApi<'_>> {
        match self {
            Self::Official(api) => Some(RoomObjectsSourceApi::Official(api)),
            Self::OpenScreeps(api) => Some(RoomObjectsSourceApi::OpenScreeps(api)),
            Self::Community(api) => api
                .screepsmod_admin_utils()
                .map(RoomObjectsSourceApi::ScreepsmodAdminUtils),
        }
    }
}

impl RoomObjectsSource for RoomObjectsSourceApi<'_> {
    async fn room_objects(&self, shard: Option<&str>, room: &str) -> Result<RoomObjects, Error> {
        match self {
            Self::Official(api) => api.room_objects(shard, room).await,
            Self::OpenScreeps(api) => api.room_objects(shard, room).await,
            Self::ScreepsmodAdminUtils(api) => api.room_objects(shard, room).await,
        }
    }
}

impl RoomObjectsSource for Official {
    async fn room_objects(&self, shard: Option<&str>, room: &str) -> Result<RoomObjects, Error> {
        let objects = official::endpoints::GameRoomObjects {
            room: room.to_owned(),
            shard: self.world_shard(shard).await?,
        };
        Ok(self.client.send(&objects).await?.body.into())
    }
}

impl RoomObjectsSource for OpenScreeps {
    async fn room_objects(&self, _shard: Option<&str>, room: &str) -> Result<RoomObjects, Error> {
        let objects = openscreeps::endpoints::GameRoomObjects {
            room: room.to_owned(),
            shard: None,
        };
        Ok(self.client.send(&objects).await?.body.into())
    }
}

impl RoomObjectsSource for ScreepsmodAdminUtils<'_> {
    async fn room_objects(&self, _shard: Option<&str>, room: &str) -> Result<RoomObjects, Error> {
        let objects = screepsmod_admin_utils::endpoints::GameRoomObjects {
            room: room.to_owned(),
            shard: None,
        };
        Ok(self.client.send(&objects).await?.body.into())
    }
}
