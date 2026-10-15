//! Room information over HTTP: objects, terrain, status, statistics and decorations.

use std::sync::Arc;

use overseer_net::core::api::types::{
    GameRoomDecoration, GameRoomObjectsResponse, GameRoomOverviewResponse, GameRoomStatus,
};
use overseer_net::core::api::{Terrain, endpoints};

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
    /// profiles of the users they reference (`GET /api/game/room-objects`). Flags are not
    /// included; [`Self::watch`] follows a room live.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails.
    pub async fn objects(
        &self,
        room: &str,
        shard: Option<&str>,
    ) -> Result<GameRoomObjectsResponse, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        Ok(self
            .overseer
            .client()
            .room_objects(shard.as_deref(), room)
            .await?)
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
        let request = endpoints::GameRoomStatus {
            room: room.to_owned(),
            shard: self.overseer.resolve_shard(shard),
        };
        Ok(self.overseer.send(&request).await?.room)
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
        let request = endpoints::GameRoomOverview {
            room: room.to_owned(),
            interval: Some(interval.minutes()),
            shard: self.overseer.resolve_shard(shard),
        };
        self.overseer.send(&request).await
    }

    /// The decorations players activated in `room` (`GET /api/game/room-decorations`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails.
    pub async fn decorations(
        &self,
        room: &str,
        shard: Option<&str>,
    ) -> Result<Vec<GameRoomDecoration>, Error> {
        let request = endpoints::GameRoomDecorations {
            room: room.to_owned(),
            shard: self.overseer.resolve_shard(shard),
        };
        Ok(self.overseer.send(&request).await?.decorations)
    }
}
