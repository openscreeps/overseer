//! The world map: the continuous [`WorldMap`] and what it knows ([`WorldKnowledge`]), room
//! map views, map statistics, terrain, portals, player badges, world size and map visuals.

mod coord;
mod knowledge;
mod portal;
mod rect;
mod stat;
mod view;
mod world;

pub use coord::RoomCoord;
pub use knowledge::WorldKnowledge;
pub use overseer_net::core::api::Terrain;
pub use overseer_net::core::socket::types::RoomMap;
pub use portal::{Portal, PortalDestination, may_have_portals};
pub use rect::RoomRect;
pub use stat::{RoomOwner, RoomReservation, RoomStat, RoomStatus};
pub use view::{MapView, ViewChange};
pub use world::{
    DetailState, Interest, MINIMAP_CAP, PORTAL_BACKGROUND_SCANS, PORTAL_DECAYING_TTL,
    PORTAL_FETCH_TIMEOUT, PORTAL_IN_FLIGHT, PORTAL_SCAN_SETTLE, PORTAL_SCAN_TIMEOUT, PORTAL_TTL,
    PREFETCH_RING, STATS_BATCH_SECTORS, STATS_FAR_SHARE, STATS_FAR_TTL, STATS_IN_FLIGHT, STATS_TTL,
    STATS_VIEWED_FRESH, TERRAIN_BATCH, TERRAIN_IN_FLIGHT, WorldEvent, WorldMap, WorldProgress,
};

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use overseer_net::core::api::endpoints;
use overseer_net::core::api::types::{
    GameMapStatsResponse, GameRoomStatName, GameWorldSizeResponse,
};
use overseer_net::core::socket::types::MapVisualItem;
use overseer_net::core::socket::{Channel, Payload};

use crate::{Error, Overseer, StatInterval, Watch};

/// What [`Map::stats`] ranks besides ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapStat {
    /// Ownership only (`owner0`): status, owner, sign, safe mode and mineral per room.
    Owner,
    /// Ownership and the ranking of `stat` over `interval` (for example
    /// `energyHarvested8`).
    Ranked(GameRoomStatName, StatInterval),
}

impl MapStat {
    /// The request's `statName`.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::Owner => "owner0".to_owned(),
            Self::Ranked(stat, interval) => format!("{}{}", stat.as_str(), interval.minutes()),
        }
    }
}

/// The world map: the continuous [`WorldMap`], room map views, map statistics, terrain,
/// portals, player badges, world size and map visuals.
///
/// Created by [`Overseer::map`]; cheap to clone. `shard` names a shard (`None`: the default
/// shard); it is ignored by single-world servers. Room status is part of [`Self::stats`]
/// (and of [`Rooms::status`](crate::Rooms::status) for one room).
#[derive(Clone, Debug)]
pub struct Map {
    overseer: Overseer,
}

impl Map {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// An empty view of the map of `shard`; [`MapView::set_rooms`] chooses its rooms.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user (the server only lets signed-in
    /// connections subscribe to room map views) and [`Error::Closed`] after
    /// [`Overseer::shutdown`](crate::Overseer::shutdown).
    pub fn view(&self, shard: Option<&str>) -> Result<MapView, Error> {
        self.overseer.ensure_open()?;
        self.overseer.require_user()?;
        let shard = self.overseer.resolve_shard(shard);
        Ok(MapView::new(self.overseer.clone(), shard))
    }

    /// Map statistics of `rooms` (`POST /api/game/map-stats`): status, novice and respawn
    /// areas, owner, sign, safe mode and mineral per room, the ranking selected by `stat`, and
    /// the referenced users. Rooms the server does not know are absent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user, [`Error::Closed`] after
    /// [`Overseer::shutdown`](crate::Overseer::shutdown) and [`Error::Net`] when the request
    /// fails.
    pub async fn stats<R: Into<String>>(
        &self,
        rooms: impl IntoIterator<Item = R>,
        stat: MapStat,
        shard: Option<&str>,
    ) -> Result<GameMapStatsResponse, Error> {
        self.overseer.ensure_open()?;
        self.overseer.require_user()?;
        let shard = self.overseer.resolve_shard(shard);
        let client = self.overseer.client();
        Ok(client
            .map_stats(shard.as_deref(), rooms, &stat.name())
            .await?)
    }

    /// The decoded terrain of `rooms`, by room name. Rooms already fetched in this session
    /// (by any feature) come from the client's cache; the others are fetched with one
    /// `POST /api/game/rooms`. Rooms the server does not know are absent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails or a terrain does not decode.
    pub async fn terrain<R: Into<String>>(
        &self,
        rooms: impl IntoIterator<Item = R>,
        shard: Option<&str>,
    ) -> Result<BTreeMap<String, Arc<Terrain>>, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        let rooms = rooms
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<String>>();
        Ok(self
            .overseer
            .client()
            .terrains(shard.as_deref(), rooms)
            .await?)
    }

    /// The world's size in rooms (`GET /api/game/world-size`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails.
    pub async fn world_size(&self, shard: Option<&str>) -> Result<GameWorldSizeResponse, Error> {
        let request = endpoints::GameWorldSize {
            shard: self.overseer.resolve_shard(shard),
        };
        self.overseer.send(&request).await
    }

    /// The badge of the user named `username` as an SVG document, optionally with a border
    /// (`GET /api/user/badge-svg`): the icon of a room's [`RoomOwner`] or
    /// [`RoomReservation`], whose `badge` parameters tell when it changed. Badges are public;
    /// same as [`Users::badge_svg`](crate::Users::badge_svg).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails (for example HTTP 404 for an unknown user).
    pub async fn badge_svg(&self, username: &str, border: bool) -> Result<String, Error> {
        self.overseer.ensure_open()?;
        self.overseer.users().badge_svg(username, border).await
    }

    /// The portals of `room` with their destinations, from one snapshot of its objects
    /// (`GET /api/game/room-objects`), by tile row by row; empty when it has none.
    /// [`WorldMap`] finds the rooms with portals and asks for those.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when the request fails.
    pub async fn portals(&self, room: &str, shard: Option<&str>) -> Result<Vec<Portal>, Error> {
        let objects = self.overseer.rooms().objects(room, shard).await?;
        Ok(portal::portals_of(&objects.objects))
    }

    /// The signed-in user's map visuals of the previous tick (`Game.map.visual`), parsed into
    /// items; lines that do not match the reference are skipped by the socket.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user and [`Error::Closed`] after
    /// [`Overseer::shutdown`](crate::Overseer::shutdown).
    pub fn visuals(&self, shard: Option<&str>) -> Result<Watch<Vec<MapVisualItem>>, Error> {
        self.overseer.ensure_open()?;
        let user = self.overseer.require_user()?.to_owned();
        let channel = Channel::MapVisual {
            user,
            shard: self.overseer.resolve_shard(shard),
        };
        Ok(self.overseer.watch(channel, |payload| match payload {
            Payload::MapVisual(items) => Some(items),
            _ => None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use overseer_net::core::api::types::GameRoomStatName;

    use super::MapStat;
    use crate::StatInterval;

    #[test]
    fn map_stats_are_named_as_the_server_expects() {
        assert_eq!(MapStat::Owner.name(), "owner0");
        let harvested = MapStat::Ranked(GameRoomStatName::EnergyHarvested, StatInterval::OneDay);
        assert_eq!(harvested.name(), "energyHarvested1440");
        let lost = MapStat::Ranked(GameRoomStatName::CreepsLost, StatInterval::EightMinutes);
        assert_eq!(lost.name(), "creepsLost8");
    }
}
