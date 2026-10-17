//! Room information over HTTP: objects, terrain, status, statistics and decorations.

use std::sync::Arc;

use overseer_net::api::{RoomObjectsSource as _, WorldApi as _};
use overseer_net::model::platform::Subsystem;
use overseer_net::model::world::{
    GameRoomDecoration, GameRoomOverviewResponse, GameRoomStatus, RoomObjects, Terrain,
};

use super::Rooms;
use crate::Error;

/// The window of a statistic: the official client's 8-minute, 3-hour and 1-day views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StatInterval {
    /// The last 8 minutes.
    #[default]
    EightMinutes,
    /// The last 3 hours.
    ThreeHours,
    /// The last day.
    OneDay,
}

impl StatInterval {
    /// The window in minutes, as the server names it (8, 180 or 1440).
    #[must_use]
    pub const fn minutes(self) -> u64 {
        match self {
            Self::EightMinutes => 8,
            Self::ThreeHours => 180,
            Self::OneDay => 1440,
        }
    }
}

/// Room information over HTTP. `shard` names the room's shard (`None`: the default shard); it
/// is ignored by single-world servers.
impl Rooms {
    /// A one-off snapshot of every object in `room`, typed by the reference, and the public
    /// profiles of the users they reference (`GET /api/game/room-objects`), on servers with
    /// room objects over HTTP ([`Subsystem::RoomObjects`]). Flags are not included;
    /// [`Self::watch`] follows a room live.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails, with
    /// [`Unsupported`](overseer_net::Error::Unsupported) on a server without room objects
    /// (nothing is sent).
    pub async fn objects(&self, room: &str, shard: Option<&str>) -> Result<RoomObjects, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        let source = self
            .overseer
            .api()
            .room_objects_source()
            .ok_or(overseer_net::Error::Unsupported(Subsystem::RoomObjects))?;
        Ok(source.room_objects(shard.as_deref(), room).await?)
    }

    /// The decoded terrain of `room`, fetched once per session and then served from the
    /// client's cache (`GET /api/game/room-terrain`). The cache is shared with
    /// [`Self::watch`] and [`Map::terrain`](crate::Map::terrain).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails or the terrain does not decode.
    pub async fn terrain(&self, room: &str, shard: Option<&str>) -> Result<Arc<Terrain>, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        Ok(self
            .overseer
            .client()
            .terrain(shard.as_deref(), room)
            .await?)
    }

    /// The status of `room`: `normal` or `out of borders`, and the novice, respawn and opening
    /// times (`GET /api/game/room-status`). Every field is `None` for an unknown room.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user, [`Error::Closed`] after
    /// [`Overseer::shutdown`](crate::Overseer::shutdown) and [`Error::Net`] when the request
    /// fails.
    pub async fn status(&self, room: &str, shard: Option<&str>) -> Result<GameRoomStatus, Error> {
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        Ok(api.room_status(shard.as_deref(), room).await?)
    }

    /// The room overview: the controller owner and a graph of every room statistic over
    /// `interval` (`GET /api/game/room-overview`).
    ///
    /// # Errors
    ///
    /// As [`Self::status`].
    pub async fn overview(
        &self,
        room: &str,
        interval: StatInterval,
        shard: Option<&str>,
    ) -> Result<GameRoomOverviewResponse, Error> {
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        let overview = api.room_overview(shard.as_deref(), room, interval.minutes());
        Ok(overview.await?)
    }

    /// The decorations players activated in `room` (`GET /api/game/room-decorations`), on
    /// servers with room decorations ([`Subsystem::RoomDecorations`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails, with
    /// [`Unsupported`](overseer_net::Error::Unsupported) on a server without room decorations
    /// (nothing is sent).
    pub async fn decorations(
        &self,
        room: &str,
        shard: Option<&str>,
    ) -> Result<Vec<GameRoomDecoration>, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.api();
        Ok(api.room_decorations(shard.as_deref(), room).await?)
    }
}
