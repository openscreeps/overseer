//! World area: shards, terrain, room information, map statistics, world size, room history and
//! the actions on rooms (flags, construction sites, names, intents, the first spawn,
//! invaders), plus the subsystem of reading a room's objects ([`RoomObjectsSource`]).
//!
//! Every contract shares the request and reply types of these operations except map
//! statistics (the ranking's shape), the created invader (`OpenScreeps` returns the creep)
//! and room history (screeps.com serves static chunks per shard, `OpenScreeps`
//! `GET /room-history`). The community contract lacks the shard information, room
//! decorations, the object lookup, the random novice room, room objects and room history (its
//! stock server keeps none); screepsmod-admin-utils provides the shard information and room
//! objects and adapts the first spawn (a name is required). `OpenScreeps` lacks the shard
//! information. Operations of a [`Subsystem`](crate::model::platform::Subsystem) the server
//! lacks fail with [`Error::Unsupported`] without a request.

mod contracts;
mod room_objects;

use std::collections::BTreeMap;

use overseer_core::api::{RateLimit, official};
use overseer_core::model::world::{
    FlagColor, GameAddObjectIntentRequestIntent, GameBuildableStructureType,
    GameRandomNoviceRoomResponse, GameRoomDecoration, GameRoomOverviewResponse, GameRoomStatus,
    GameWorldSizeResponse, InvaderRequest, MapStats, ShardInfo, Terrain,
};
use serde_json::Value;

use super::{Api, delegate};
use crate::{Client, Error, RoomHistory};

pub use room_objects::{RoomObjectsSource, RoomObjectsSourceApi};

/// The world: terrain, rooms, the map and the actions on rooms. `shard` names the shard of the
/// operation (`None`: the client's default shard); servers serving one world ignore it.
pub trait WorldApi {
    /// Every shard of the server with its CPU limit and average tick duration
    /// (`GET /api/game/shards/info`;
    /// [`Subsystem::ShardsInfo`](crate::model::platform::Subsystem::ShardsInfo)), in the
    /// server's order. screeps.com and its Season server list every shard;
    /// screepsmod-admin-utils lists the one world of a community server without a CPU limit,
    /// and no shard while its `shardName` setting is not set.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]; [`Error::Unsupported`] on servers without the
    /// shard information.
    fn shards_info(&self) -> impl Future<Output = Result<Vec<ShardInfo>, Error>>;

    /// The terrain of `room` (`GET /api/game/room-terrain?encoded=1`), read in whichever form
    /// the server answers (encoded, or wall and swamp tiles).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`], and [`Error::Terrain`] for a terrain that does
    /// not decode or a reply without terrain (an unknown room).
    fn room_terrain(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> impl Future<Output = Result<Terrain, Error>>;

    /// The terrain of each of `rooms` the server knows, with its room name
    /// (`POST /api/game/rooms`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`] and [`Error::Terrain`] for a terrain that does
    /// not decode.
    fn terrains(
        &self,
        shard: Option<&str>,
        rooms: Vec<String>,
    ) -> impl Future<Output = Result<Vec<(String, Terrain)>, Error>>;

    /// The status of `room`: `normal` or `out of borders`, and the novice, respawn and opening
    /// times (`GET /api/game/room-status`). Every field is `None` for an unknown room.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn room_status(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> impl Future<Output = Result<GameRoomStatus, Error>>;

    /// The room overview: the controller owner and a graph of every room statistic over
    /// `interval` minutes (8, 180 or 1440) (`GET /api/game/room-overview`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn room_overview(
        &self,
        shard: Option<&str>,
        room: &str,
        interval: u64,
    ) -> impl Future<Output = Result<GameRoomOverviewResponse, Error>>;

    /// The decorations players activated in `room` (`GET /api/game/room-decorations`;
    /// [`Subsystem::RoomDecorations`](crate::model::platform::Subsystem::RoomDecorations)).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]; [`Error::Unsupported`] on servers without room
    /// decorations.
    fn room_decorations(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> impl Future<Output = Result<Vec<GameRoomDecoration>, Error>>;

    /// World-map statistics of `rooms` (`POST /api/game/map-stats`): status, owner, sign, safe
    /// mode and mineral per room, plus the ranking of `stat_name` and the referenced users.
    /// `stat_name` is `owner0` for ownership only, or a statistic suffixed with an interval
    /// such as `energyHarvested8` ([`GameRoomStatName`](overseer_core::model::world::GameRoomStatName)).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn map_stats(
        &self,
        shard: Option<&str>,
        rooms: Vec<String>,
        stat_name: &str,
    ) -> impl Future<Output = Result<MapStats, Error>>;

    /// The world's size in rooms (`GET /api/game/world-size`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn world_size(
        &self,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<GameWorldSizeResponse, Error>>;

    /// The current game tick (`GET /api/game/time`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]; [`Error::UnexpectedReply`] for a negative tick.
    fn game_time(&self, shard: Option<&str>) -> impl Future<Output = Result<u64, Error>>;

    /// The room-history chunk of `room` that holds `tick`, ready to
    /// [replay](RoomHistory::replay)
    /// ([`Subsystem::RoomHistory`](crate::model::platform::Subsystem::RoomHistory)). Chunks
    /// start at multiples of [`World::history_chunk_size`](crate::World::history_chunk_size);
    /// screeps.com serves them as static files (`GET /room-history/{shard}/{room}/{tick}.json`),
    /// `OpenScreeps` through `GET /room-history?room&time`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]: a chunk that is not kept yields an HTTP 404
    /// [`ApiError::Status`](overseer_core::api::ApiError::Status), or HTTP 500 without a
    /// message on a community server with a room-history mod (backend-local sends the mod's
    /// error, an `Error` that serialises to `{}`, with HTTP 500); [`Error::Unsupported`] on
    /// servers without room history (the stock community server answers every chunk with HTTP
    /// 500 `not implemented`), and [`Error::UnexpectedReply`] for a chunk whose ticks are not
    /// numbers or a sharded server listing no shard.
    fn room_history(
        &self,
        shard: Option<&str>,
        room: &str,
        tick: u64,
    ) -> impl Future<Output = Result<RoomHistory, Error>>;

    /// The room of the object `id`, when it exists (`GET /api/game/find-object`;
    /// [`Subsystem::ObjectSearch`](crate::model::platform::Subsystem::ObjectSearch)).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]; [`Error::Unsupported`] on servers without
    /// object search.
    fn find_object(&self, id: &str) -> impl Future<Output = Result<Option<String>, Error>>;

    /// A random room of a novice area to start in (`GET /api/game/random-novice-room`;
    /// [`Subsystem::NoviceRooms`](crate::model::platform::Subsystem::NoviceRooms)).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]; [`Error::Unsupported`] on servers without
    /// random novice rooms.
    fn random_novice_room(
        &self,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<GameRandomNoviceRoomResponse, Error>>;

    /// Creates the flag `name` at `at` (`(x, y)`, 0 to 49) in `room` with `(primary, secondary)`
    /// colours, or moves the user's flag of that name there (`POST /api/game/create-flag`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn create_flag(
        &self,
        shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        name: &str,
        colors: (FlagColor, FlagColor),
    ) -> impl Future<Output = Result<(), Error>>;

    /// Recolours the user's flag `name` in `room` (`POST /api/game/change-flag-color`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn change_flag_colors(
        &self,
        shard: Option<&str>,
        room: &str,
        name: &str,
        colors: (FlagColor, FlagColor),
    ) -> impl Future<Output = Result<(), Error>>;

    /// Removes the user's flag `name` from `room` (`POST /api/game/remove-flag`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn remove_flag(
        &self,
        shard: Option<&str>,
        room: &str,
        name: &str,
    ) -> impl Future<Output = Result<(), Error>>;

    /// The first `Flag<N>` name none of the user's flags has
    /// (`POST /api/game/gen-unique-flag-name`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn unique_flag_name(&self, shard: Option<&str>) -> impl Future<Output = Result<String, Error>>;

    /// Whether none of the user's flags is named `name`
    /// (`POST /api/game/check-unique-flag-name`; `name exists` is a taken name).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn is_flag_name_free(
        &self,
        shard: Option<&str>,
        name: &str,
    ) -> impl Future<Output = Result<bool, Error>>;

    /// The first `Spawn<N>` name none of the user's spawns and spawn construction sites has
    /// (`POST /api/game/gen-unique-object-name`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn unique_spawn_name(&self, shard: Option<&str>)
    -> impl Future<Output = Result<String, Error>>;

    /// Whether none of the user's spawns and spawn construction sites is named `name`
    /// (`POST /api/game/check-unique-object-name`; `name exists` is a taken name).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn is_spawn_name_free(
        &self,
        shard: Option<&str>,
        name: &str,
    ) -> impl Future<Output = Result<bool, Error>>;

    /// Places a construction site of `structure` at `at` (`(x, y)`, 0 to 49) in `room`
    /// (`POST /api/game/create-construction`); `name` names a spawn. Returns the new site's
    /// id when the server reports it.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn create_construction_site(
        &self,
        shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        structure: GameBuildableStructureType,
        name: Option<&str>,
    ) -> impl Future<Output = Result<Option<String>, Error>>;

    /// Sets whether the user is notified when the object `id` is attacked
    /// (`POST /api/game/set-notify-when-attacked`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn set_notify_when_attacked(
        &self,
        shard: Option<&str>,
        id: &str,
        enabled: bool,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Queues the intent `name` with `intent` for the object of `target` (`(room, object id)`; the object id `room` for
    /// room-level intents, whose payload is a list), executed next tick
    /// (`POST /api/game/add-object-intent`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn add_object_intent(
        &self,
        shard: Option<&str>,
        target: (&str, &str),
        name: &str,
        intent: GameAddObjectIntentRequestIntent,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Queues the account-level intent `name` with `intent` (market orders, power creeps),
    /// executed next tick (`POST /api/game/add-global-intent`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn add_global_intent(
        &self,
        shard: Option<&str>,
        name: &str,
        intent: BTreeMap<String, Value>,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Places the user's first spawn at `at` (`(x, y)`, 0 to 49) in `room` and starts playing
    /// (`POST /api/game/place-spawn`); `name` names it. The spawn is unnamed when `None`,
    /// except on community servers with screepsmod-admin-utils, which require a name: the
    /// server's next free `Spawn<N>` name is used there.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn place_spawn(
        &self,
        shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        name: Option<&str>,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Summons the NPC invader `request` in a room the user owns or reserves
    /// (`POST /api/game/create-invader`); returns its object id when the server reports it
    /// (`OpenScreeps`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn create_invader(
        &self,
        shard: Option<&str>,
        request: &InvaderRequest,
    ) -> impl Future<Output = Result<Option<String>, Error>>;

    /// Removes the invader `id` the user summoned (`POST /api/game/remove-invader`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
    fn remove_invader(
        &self,
        shard: Option<&str>,
        id: &str,
    ) -> impl Future<Output = Result<(), Error>>;
}

impl WorldApi for Api {
    async fn shards_info(&self) -> Result<Vec<ShardInfo>, Error> {
        delegate!(self, api => api.shards_info().await)
    }

    async fn room_terrain(&self, shard: Option<&str>, room: &str) -> Result<Terrain, Error> {
        delegate!(self, api => api.room_terrain(shard, room).await)
    }

    async fn terrains(
        &self,
        shard: Option<&str>,
        rooms: Vec<String>,
    ) -> Result<Vec<(String, Terrain)>, Error> {
        delegate!(self, api => api.terrains(shard, rooms).await)
    }

    async fn room_status(&self, shard: Option<&str>, room: &str) -> Result<GameRoomStatus, Error> {
        delegate!(self, api => api.room_status(shard, room).await)
    }

    async fn room_overview(
        &self,
        shard: Option<&str>,
        room: &str,
        interval: u64,
    ) -> Result<GameRoomOverviewResponse, Error> {
        delegate!(self, api => api.room_overview(shard, room, interval).await)
    }

    async fn room_decorations(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> Result<Vec<GameRoomDecoration>, Error> {
        delegate!(self, api => api.room_decorations(shard, room).await)
    }

    async fn map_stats(
        &self,
        shard: Option<&str>,
        rooms: Vec<String>,
        stat_name: &str,
    ) -> Result<MapStats, Error> {
        delegate!(self, api => api.map_stats(shard, rooms, stat_name).await)
    }

    async fn world_size(&self, shard: Option<&str>) -> Result<GameWorldSizeResponse, Error> {
        delegate!(self, api => api.world_size(shard).await)
    }

    async fn game_time(&self, shard: Option<&str>) -> Result<u64, Error> {
        delegate!(self, api => api.game_time(shard).await)
    }

    async fn room_history(
        &self,
        shard: Option<&str>,
        room: &str,
        tick: u64,
    ) -> Result<RoomHistory, Error> {
        delegate!(self, api => api.room_history(shard, room, tick).await)
    }

    async fn find_object(&self, id: &str) -> Result<Option<String>, Error> {
        delegate!(self, api => api.find_object(id).await)
    }

    async fn random_novice_room(
        &self,
        shard: Option<&str>,
    ) -> Result<GameRandomNoviceRoomResponse, Error> {
        delegate!(self, api => api.random_novice_room(shard).await)
    }

    async fn create_flag(
        &self,
        shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        name: &str,
        colors: (FlagColor, FlagColor),
    ) -> Result<(), Error> {
        delegate!(self, api => api.create_flag(shard, room, at, name, colors).await)
    }

    async fn change_flag_colors(
        &self,
        shard: Option<&str>,
        room: &str,
        name: &str,
        colors: (FlagColor, FlagColor),
    ) -> Result<(), Error> {
        delegate!(self, api => api.change_flag_colors(shard, room, name, colors).await)
    }

    async fn remove_flag(&self, shard: Option<&str>, room: &str, name: &str) -> Result<(), Error> {
        delegate!(self, api => api.remove_flag(shard, room, name).await)
    }

    async fn unique_flag_name(&self, shard: Option<&str>) -> Result<String, Error> {
        delegate!(self, api => api.unique_flag_name(shard).await)
    }

    async fn is_flag_name_free(&self, shard: Option<&str>, name: &str) -> Result<bool, Error> {
        delegate!(self, api => api.is_flag_name_free(shard, name).await)
    }

    async fn unique_spawn_name(&self, shard: Option<&str>) -> Result<String, Error> {
        delegate!(self, api => api.unique_spawn_name(shard).await)
    }

    async fn is_spawn_name_free(&self, shard: Option<&str>, name: &str) -> Result<bool, Error> {
        delegate!(self, api => api.is_spawn_name_free(shard, name).await)
    }

    async fn create_construction_site(
        &self,
        shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        structure: GameBuildableStructureType,
        name: Option<&str>,
    ) -> Result<Option<String>, Error> {
        delegate!(self, api => {
            api.create_construction_site(shard, room, at, structure, name).await
        })
    }

    async fn set_notify_when_attacked(
        &self,
        shard: Option<&str>,
        id: &str,
        enabled: bool,
    ) -> Result<(), Error> {
        delegate!(self, api => api.set_notify_when_attacked(shard, id, enabled).await)
    }

    async fn add_object_intent(
        &self,
        shard: Option<&str>,
        target: (&str, &str),
        name: &str,
        intent: GameAddObjectIntentRequestIntent,
    ) -> Result<(), Error> {
        delegate!(self, api => api.add_object_intent(shard, target, name, intent).await)
    }

    async fn add_global_intent(
        &self,
        shard: Option<&str>,
        name: &str,
        intent: BTreeMap<String, Value>,
    ) -> Result<(), Error> {
        delegate!(self, api => api.add_global_intent(shard, name, intent).await)
    }

    async fn place_spawn(
        &self,
        shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        name: Option<&str>,
    ) -> Result<(), Error> {
        delegate!(self, api => api.place_spawn(shard, room, at, name).await)
    }

    async fn create_invader(
        &self,
        shard: Option<&str>,
        request: &InvaderRequest,
    ) -> Result<Option<String>, Error> {
        delegate!(self, api => api.create_invader(shard, request).await)
    }

    async fn remove_invader(&self, shard: Option<&str>, id: &str) -> Result<(), Error> {
        delegate!(self, api => api.remove_invader(shard, id).await)
    }
}

/// World operations whose rate limit a caller paces its own requests by (screeps.com limits
/// them per token). Every contract names them by the same method and path, so their limits
/// are the same whichever the server speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorldOperation {
    /// [`WorldApi::map_stats`] (`POST /api/game/map-stats`).
    MapStats,
    /// [`RoomObjectsSource::room_objects`] (`GET /api/game/room-objects`).
    RoomObjects,
    /// [`WorldApi::terrains`] (`POST /api/game/rooms`).
    Terrains,
}

impl WorldOperation {
    /// The latest rate limit `client`'s server reported for the operation
    /// ([`Client::rate_limit`]).
    #[must_use]
    pub fn rate_limit(self, client: &Client) -> Option<RateLimit> {
        match self {
            Self::MapStats => client.rate_limit::<official::endpoints::GameMapStats>(),
            Self::RoomObjects => client.rate_limit::<official::endpoints::GameRoomObjects>(),
            Self::Terrains => client.rate_limit::<official::endpoints::GameRooms>(),
        }
    }

    /// Forgets the operation's latest rate limit ([`Client::forget_rate_limit`]).
    pub fn forget_rate_limit(self, client: &Client) {
        match self {
            Self::MapStats => client.forget_rate_limit::<official::endpoints::GameMapStats>(),
            Self::RoomObjects => {
                client.forget_rate_limit::<official::endpoints::GameRoomObjects>();
            }
            Self::Terrains => client.forget_rate_limit::<official::endpoints::GameRooms>(),
        }
    }
}
