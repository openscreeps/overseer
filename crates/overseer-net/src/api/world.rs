//! World area: terrain, room information, map statistics, world size, room history and the
//! actions on rooms (flags, construction sites, names, intents, the first spawn, invaders),
//! plus the subsystem of reading a room's objects ([`RoomObjectsSource`]).
//!
//! Every contract shares the request and reply types of these operations except map
//! statistics (the ranking's shape), the created invader (`OpenScreeps` returns the creep)
//! and room history (screeps.com serves static chunks per shard, `OpenScreeps`
//! `GET /room-history`). The community contract lacks room decorations, the object lookup,
//! the random novice room, room objects and room history (its stock server keeps none);
//! screepsmod-admin-utils provides room objects and adapts the first spawn (a name is
//! required). Operations of a [`Subsystem`] the server lacks fail with
//! [`Error::Unsupported`] without a request.

use std::collections::BTreeMap;
use std::future::{Ready, ready};

use overseer_core::api::official::types::GameCoordinateInput;
use overseer_core::api::plugins::screepsmod_admin_utils;
use overseer_core::api::{ApiError, RateLimit, community, official, openscreeps};
use overseer_core::model::platform::Subsystem;
use overseer_core::model::world::PlatformHistory;
use overseer_core::model::world::{
    FlagColor, GameAddObjectIntentRequestIntent, GameBuildableStructureType,
    GameRandomNoviceRoomResponse, GameRoomDecoration, GameRoomOverviewResponse, GameRoomStatus,
    GameWorldSizeResponse, InvaderRequest, MapStats, RoomObjects, Terrain,
};
use serde_json::Value;

use super::{Api, Community, Official, OpenScreeps, ScreepsmodAdminUtils, delegate};
use crate::{Client, Error, RoomHistory};

/// The world: terrain, rooms, the map and the actions on rooms. `shard` names the shard of the
/// operation (`None`: the client's default shard); servers serving one world ignore it.
pub trait WorldApi {
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
    /// [`Subsystem::RoomDecorations`]).
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
    /// [replay](RoomHistory::replay) ([`Subsystem::RoomHistory`]). Chunks start at multiples
    /// of [`World::history_chunk_size`](crate::World::history_chunk_size); screeps.com serves
    /// them as static files (`GET /room-history/{shard}/{room}/{tick}.json`), `OpenScreeps`
    /// through `GET /room-history?room&time`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]: a chunk that is not kept yields an HTTP 404
    /// [`ApiError::Status`]; [`Error::Unsupported`] on servers without room history (the
    /// stock community server answers every chunk with HTTP 500 `not implemented`), and
    /// [`Error::UnexpectedReply`] for a chunk whose ticks are not numbers or a sharded server
    /// listing no shard.
    fn room_history(
        &self,
        shard: Option<&str>,
        room: &str,
        tick: u64,
    ) -> impl Future<Output = Result<RoomHistory, Error>>;

    /// The room of the object `id`, when it exists (`GET /api/game/find-object`;
    /// [`Subsystem::ObjectSearch`]).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]; [`Error::Unsupported`] on servers without
    /// object search.
    fn find_object(&self, id: &str) -> impl Future<Output = Result<Option<String>, Error>>;

    /// A random room of a novice area to start in (`GET /api/game/random-novice-room`;
    /// [`Subsystem::NoviceRooms`]).
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

impl Official {
    /// The shard an operation names: `shard`, else the client's default shard.
    async fn world_shard(&self, shard: Option<&str>) -> Result<Option<String>, Error> {
        self.client.resolve_shard(shard).await
    }
}

impl OpenScreeps {
    /// No shard: an `OpenScreeps` server serves one world.
    #[expect(
        clippy::unused_self,
        reason = "the shared operations ask every contract's API for the shard alike"
    )]
    fn world_shard(&self, _shard: Option<&str>) -> Ready<Result<Option<String>, Error>> {
        ready(Ok(None))
    }
}

impl Community {
    /// No shard: a community server serves one world.
    #[expect(
        clippy::unused_self,
        reason = "the shared operations ask every contract's API for the shard alike"
    )]
    fn world_shard(&self, _shard: Option<&str>) -> Ready<Result<Option<String>, Error>> {
        ready(Ok(None))
    }
}

/// A room coordinate as requests take it.
fn coordinate(value: u8) -> GameCoordinateInput {
    GameCoordinateInput::Integer(value.into())
}

/// A name check's reply: `name exists` is a taken name, not a failure.
fn name_free<T>(reply: Result<T, Error>) -> Result<bool, Error> {
    match reply {
        Ok(_) => Ok(true),
        Err(Error::Api(ApiError::Server(message))) if message == "name exists" => Ok(false),
        Err(error) => Err(error),
    }
}

/// The chunk of `room` that holds `tick`, which starts at `tick` rounded down to `chunk_size`.
const fn chunk_base(tick: u64, chunk_size: u64) -> u64 {
    if chunk_size == 0 {
        tick
    } else {
        tick - tick % chunk_size
    }
}

/// The operations whose request and reply types every contract shares, for the impl of
/// `$contract` (`official`, `openscreeps` or `community`): items of an `impl WorldApi`.
macro_rules! shared_operations {
    ($contract:ident) => {
        async fn room_terrain(&self, shard: Option<&str>, room: &str) -> Result<Terrain, Error> {
            let terrain = $contract::endpoints::GameRoomTerrain {
                room: room.to_owned(),
                encoded: Some("1".to_owned()),
                shard: self.world_shard(shard).await?,
            };
            Ok(Terrain::try_from(self.client.send(&terrain).await?.body)?)
        }

        async fn terrains(
            &self,
            shard: Option<&str>,
            rooms: Vec<String>,
        ) -> Result<Vec<(String, Terrain)>, Error> {
            let request = $contract::endpoints::GameRooms {
                shard: self.world_shard(shard).await?,
                body: $contract::types::GameRoomsRequest { rooms },
            };
            let documents = self.client.send(&request).await?.body.rooms;
            documents
                .into_iter()
                .map(|document| {
                    let terrain = Terrain::try_from(&document)?;
                    Ok((document.room, terrain))
                })
                .collect()
        }

        async fn room_status(
            &self,
            shard: Option<&str>,
            room: &str,
        ) -> Result<GameRoomStatus, Error> {
            let status = $contract::endpoints::GameRoomStatus {
                room: room.to_owned(),
                shard: self.world_shard(shard).await?,
            };
            Ok(self.client.send(&status).await?.body.room)
        }

        async fn room_overview(
            &self,
            shard: Option<&str>,
            room: &str,
            interval: u64,
        ) -> Result<GameRoomOverviewResponse, Error> {
            let overview = $contract::endpoints::GameRoomOverview {
                room: room.to_owned(),
                interval: Some(interval),
                shard: self.world_shard(shard).await?,
            };
            Ok(self.client.send(&overview).await?.body)
        }

        async fn map_stats(
            &self,
            shard: Option<&str>,
            rooms: Vec<String>,
            stat_name: &str,
        ) -> Result<MapStats, Error> {
            let body = $contract::types::GameMapStatsRequest {
                rooms,
                shard: self.world_shard(shard).await?,
                stat_name: stat_name.to_owned(),
            };
            let stats = $contract::endpoints::GameMapStats { body };
            Ok(self.client.send(&stats).await?.body.into())
        }

        async fn world_size(&self, shard: Option<&str>) -> Result<GameWorldSizeResponse, Error> {
            let size = $contract::endpoints::GameWorldSize {
                shard: self.world_shard(shard).await?,
            };
            Ok(self.client.send(&size).await?.body)
        }

        async fn game_time(&self, shard: Option<&str>) -> Result<u64, Error> {
            let time = $contract::endpoints::GameTime {
                shard: self.world_shard(shard).await?,
            };
            let time = self.client.send(&time).await?.body.time;
            u64::try_from(time).map_err(|_| Error::UnexpectedReply("negative game tick"))
        }

        async fn create_flag(
            &self,
            shard: Option<&str>,
            room: &str,
            (x, y): (u8, u8),
            name: &str,
            colors: (FlagColor, FlagColor),
        ) -> Result<(), Error> {
            let body = $contract::types::GameCreateFlagRequest {
                color: colors.0.code().into(),
                name: name.to_owned(),
                room: room.to_owned(),
                secondary_color: colors.1.code().into(),
                shard: self.world_shard(shard).await?,
                x: x.into(),
                y: y.into(),
            };
            self.client
                .send(&$contract::endpoints::GameCreateFlag { body })
                .await?;
            Ok(())
        }

        async fn change_flag_colors(
            &self,
            shard: Option<&str>,
            room: &str,
            name: &str,
            colors: (FlagColor, FlagColor),
        ) -> Result<(), Error> {
            let body = $contract::types::GameChangeFlagColorRequest {
                color: colors.0.code().into(),
                name: name.to_owned(),
                room: Some(room.to_owned()),
                secondary_color: colors.1.code().into(),
                shard: self.world_shard(shard).await?,
            };
            self.client
                .send(&$contract::endpoints::GameChangeFlagColor { body })
                .await?;
            Ok(())
        }

        async fn remove_flag(
            &self,
            shard: Option<&str>,
            room: &str,
            name: &str,
        ) -> Result<(), Error> {
            let body = $contract::types::GameRemoveFlagRequest {
                name: name.to_owned(),
                room: room.to_owned(),
                shard: self.world_shard(shard).await?,
            };
            self.client
                .send(&$contract::endpoints::GameRemoveFlag { body })
                .await?;
            Ok(())
        }

        async fn unique_flag_name(&self, shard: Option<&str>) -> Result<String, Error> {
            let body = $contract::types::GameShardOnlyRequest {
                shard: self.world_shard(shard).await?,
            };
            let request = $contract::endpoints::GameGenUniqueFlagName { body };
            Ok(self.client.send(&request).await?.body.name)
        }

        async fn is_flag_name_free(&self, shard: Option<&str>, name: &str) -> Result<bool, Error> {
            let body = $contract::types::GameCheckUniqueFlagNameRequest {
                name: name.to_owned(),
                shard: self.world_shard(shard).await?,
            };
            let request = $contract::endpoints::GameCheckUniqueFlagName { body };
            name_free(self.client.send(&request).await)
        }

        async fn unique_spawn_name(&self, shard: Option<&str>) -> Result<String, Error> {
            let body = $contract::types::GameGenUniqueObjectNameRequest {
                shard: self.world_shard(shard).await?,
                r#type: $contract::types::GameGenUniqueObjectNameRequestType::Spawn,
            };
            let request = $contract::endpoints::GameGenUniqueObjectName { body };
            Ok(self.client.send(&request).await?.body.name)
        }

        async fn is_spawn_name_free(&self, shard: Option<&str>, name: &str) -> Result<bool, Error> {
            let body = $contract::types::GameCheckUniqueObjectNameRequest {
                name: name.to_owned(),
                shard: self.world_shard(shard).await?,
                r#type: $contract::types::GameCheckUniqueObjectNameRequestType::Spawn,
            };
            let request = $contract::endpoints::GameCheckUniqueObjectName { body };
            name_free(self.client.send(&request).await)
        }

        async fn create_construction_site(
            &self,
            shard: Option<&str>,
            room: &str,
            (x, y): (u8, u8),
            structure: GameBuildableStructureType,
            name: Option<&str>,
        ) -> Result<Option<String>, Error> {
            let body = $contract::types::GameCreateConstructionRequest {
                name: name.map(str::to_owned),
                room: room.to_owned(),
                shard: self.world_shard(shard).await?,
                structure_type: structure,
                x: coordinate(x),
                y: coordinate(y),
            };
            let request = $contract::endpoints::GameCreateConstruction { body };
            Ok(self.client.send(&request).await?.body.id)
        }

        async fn set_notify_when_attacked(
            &self,
            shard: Option<&str>,
            id: &str,
            enabled: bool,
        ) -> Result<(), Error> {
            let body = $contract::types::GameSetNotifyWhenAttackedRequest {
                id: id.to_owned(),
                enabled: Some(enabled),
                shard: self.world_shard(shard).await?,
            };
            self.client
                .send(&$contract::endpoints::GameSetNotifyWhenAttacked { body })
                .await?;
            Ok(())
        }

        async fn add_object_intent(
            &self,
            shard: Option<&str>,
            (room, target): (&str, &str),
            name: &str,
            intent: GameAddObjectIntentRequestIntent,
        ) -> Result<(), Error> {
            let body = $contract::types::GameAddObjectIntentRequest {
                id: target.to_owned(),
                intent: Some(intent),
                name: name.to_owned(),
                room: room.to_owned(),
                shard: self.world_shard(shard).await?,
            };
            self.client
                .send(&$contract::endpoints::GameAddObjectIntent { body })
                .await?;
            Ok(())
        }

        async fn add_global_intent(
            &self,
            shard: Option<&str>,
            name: &str,
            intent: BTreeMap<String, Value>,
        ) -> Result<(), Error> {
            let body = $contract::types::GameAddGlobalIntentRequest {
                intent: Some(intent),
                name: name.to_owned(),
                shard: self.world_shard(shard).await?,
            };
            self.client
                .send(&$contract::endpoints::GameAddGlobalIntent { body })
                .await?;
            Ok(())
        }

        async fn remove_invader(&self, shard: Option<&str>, id: &str) -> Result<(), Error> {
            let body = $contract::types::GameRemoveInvaderRequest {
                id: id.to_owned(),
                shard: self.world_shard(shard).await?,
            };
            self.client
                .send(&$contract::endpoints::GameRemoveInvader { body })
                .await?;
            Ok(())
        }
    };
}

/// The request of `POST /api/game/create-invader`, shared by every contract.
fn invader_body(
    request: &InvaderRequest,
    shard: Option<String>,
) -> official::types::GameCreateInvaderRequest {
    official::types::GameCreateInvaderRequest {
        boosted: Some(request.boosted),
        room: request.room.clone(),
        shard,
        size: request.size,
        r#type: request.kind,
        x: coordinate(request.x),
        y: coordinate(request.y),
    }
}

/// The request of `POST /api/game/place-spawn` of the official and the community contract.
fn place_spawn_body(
    room: &str,
    (x, y): (u8, u8),
    name: Option<&str>,
    shard: Option<String>,
) -> official::types::GamePlaceSpawnRequest {
    official::types::GamePlaceSpawnRequest {
        name: name.map(str::to_owned),
        room: room.to_owned(),
        shard,
        x: coordinate(x),
        y: coordinate(y),
    }
}

impl WorldApi for Official {
    shared_operations!(official);

    async fn room_decorations(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> Result<Vec<GameRoomDecoration>, Error> {
        let decorations = official::endpoints::GameRoomDecorations {
            room: room.to_owned(),
            shard: self.world_shard(shard).await?,
        };
        Ok(self.client.send(&decorations).await?.body.decorations)
    }

    async fn room_history(
        &self,
        shard: Option<&str>,
        room: &str,
        tick: u64,
    ) -> Result<RoomHistory, Error> {
        let world = self.client.world().await?;
        let shard = world
            .resolve_shard(shard)
            .ok_or(Error::UnexpectedReply("a sharded server listing no shard"))?;
        let chunk = official::endpoints::PlatformOfficialRoomHistory {
            shard,
            room: room.to_owned(),
            tick: chunk_base(tick, world.history_chunk_size()),
        };
        let chunk: PlatformHistory = self.client.send(&chunk).await?.body;
        RoomHistory::new(chunk, room.to_owned())
    }

    async fn find_object(&self, id: &str) -> Result<Option<String>, Error> {
        let find = official::endpoints::GameFindObject { id: id.to_owned() };
        let reply = self.client.send(&find).await?.body;
        Ok(reply.object.and_then(|object| object.room))
    }

    async fn random_novice_room(
        &self,
        shard: Option<&str>,
    ) -> Result<GameRandomNoviceRoomResponse, Error> {
        let random = official::endpoints::GameRandomNoviceRoom {
            shard: self.world_shard(shard).await?,
        };
        Ok(self.client.send(&random).await?.body)
    }

    async fn place_spawn(
        &self,
        shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        name: Option<&str>,
    ) -> Result<(), Error> {
        let body = place_spawn_body(room, at, name, self.world_shard(shard).await?);
        self.client
            .send(&official::endpoints::GamePlaceSpawn { body })
            .await?;
        Ok(())
    }

    async fn create_invader(
        &self,
        shard: Option<&str>,
        request: &InvaderRequest,
    ) -> Result<Option<String>, Error> {
        let body = invader_body(request, self.world_shard(shard).await?);
        self.client
            .send(&official::endpoints::GameCreateInvader { body })
            .await?;
        Ok(None)
    }
}

impl WorldApi for OpenScreeps {
    shared_operations!(openscreeps);

    async fn room_decorations(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> Result<Vec<GameRoomDecoration>, Error> {
        let decorations = openscreeps::endpoints::GameRoomDecorations {
            room: room.to_owned(),
            shard: self.world_shard(shard).await?,
        };
        Ok(self.client.send(&decorations).await?.body.decorations)
    }

    async fn room_history(
        &self,
        _shard: Option<&str>,
        room: &str,
        tick: u64,
    ) -> Result<RoomHistory, Error> {
        let world = self.client.world().await?;
        let chunk = openscreeps::endpoints::PlatformRoomHistory {
            room: room.to_owned(),
            time: chunk_base(tick, world.history_chunk_size()),
        };
        let chunk: PlatformHistory = self.client.send(&chunk).await?.body;
        RoomHistory::new(chunk, room.to_owned())
    }

    async fn find_object(&self, id: &str) -> Result<Option<String>, Error> {
        let find = openscreeps::endpoints::GameFindObject { id: id.to_owned() };
        let reply = self.client.send(&find).await?.body;
        Ok(reply.object.and_then(|object| object.room))
    }

    async fn random_novice_room(
        &self,
        _shard: Option<&str>,
    ) -> Result<GameRandomNoviceRoomResponse, Error> {
        let random = openscreeps::endpoints::GameRandomNoviceRoom { shard: None };
        Ok(self.client.send(&random).await?.body)
    }

    async fn place_spawn(
        &self,
        _shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        name: Option<&str>,
    ) -> Result<(), Error> {
        let body = place_spawn_body(room, at, name, None);
        self.client
            .send(&openscreeps::endpoints::GamePlaceSpawn { body })
            .await?;
        Ok(())
    }

    async fn create_invader(
        &self,
        _shard: Option<&str>,
        request: &InvaderRequest,
    ) -> Result<Option<String>, Error> {
        let body = invader_body(request, None);
        let reply = self
            .client
            .send(&openscreeps::endpoints::GameCreateInvader { body })
            .await?;
        Ok(Some(reply.body.id))
    }
}

impl WorldApi for Community {
    shared_operations!(community);

    fn room_decorations(
        &self,
        _shard: Option<&str>,
        _room: &str,
    ) -> impl Future<Output = Result<Vec<GameRoomDecoration>, Error>> {
        ready(Err(Error::Unsupported(Subsystem::RoomDecorations)))
    }

    async fn room_history(
        &self,
        _shard: Option<&str>,
        room: &str,
        tick: u64,
    ) -> Result<RoomHistory, Error> {
        if !self.plugins.room_history {
            return Err(Error::Unsupported(Subsystem::RoomHistory));
        }
        let world = self.client.world().await?;
        let chunk = community::endpoints::PlatformRoomHistory {
            room: room.to_owned(),
            time: chunk_base(tick, world.history_chunk_size()),
        };
        let chunk: PlatformHistory = self.client.send(&chunk).await?.body;
        RoomHistory::new(chunk, room.to_owned())
    }

    fn find_object(&self, _id: &str) -> impl Future<Output = Result<Option<String>, Error>> {
        ready(Err(Error::Unsupported(Subsystem::ObjectSearch)))
    }

    fn random_novice_room(
        &self,
        _shard: Option<&str>,
    ) -> impl Future<Output = Result<GameRandomNoviceRoomResponse, Error>> {
        ready(Err(Error::Unsupported(Subsystem::NoviceRooms)))
    }

    async fn place_spawn(
        &self,
        shard: Option<&str>,
        room: &str,
        at: (u8, u8),
        name: Option<&str>,
    ) -> Result<(), Error> {
        if let Some(admin_utils) = self.screepsmod_admin_utils() {
            let name = match name {
                Some(name) => name.to_owned(),
                None => self.unique_spawn_name(shard).await?,
            };
            return admin_utils.place_spawn(room, at, name).await;
        }
        let body = place_spawn_body(room, at, name, None);
        self.client
            .send(&community::endpoints::GamePlaceSpawn { body })
            .await?;
        Ok(())
    }

    async fn create_invader(
        &self,
        _shard: Option<&str>,
        request: &InvaderRequest,
    ) -> Result<Option<String>, Error> {
        let body = invader_body(request, None);
        self.client
            .send(&community::endpoints::GameCreateInvader { body })
            .await?;
        Ok(None)
    }
}

impl ScreepsmodAdminUtils<'_> {
    /// screepsmod-admin-utils' first spawn: always named.
    async fn place_spawn(&self, room: &str, (x, y): (u8, u8), name: String) -> Result<(), Error> {
        let body = screepsmod_admin_utils::types::GamePlaceSpawnRequest {
            name,
            room: room.to_owned(),
            shard: None,
            x: Some(coordinate(x)),
            y: Some(coordinate(y)),
        };
        self.client
            .send(&screepsmod_admin_utils::endpoints::GamePlaceSpawn { body })
            .await?;
        Ok(())
    }
}

/// Every object of a room with the users they reference (`GET /api/game/room-objects`;
/// [`Subsystem::RoomObjects`]): native on screeps.com and `OpenScreeps`, provided by
/// screepsmod-admin-utils on community servers (the stock server has no such route). From
/// [`Api::room_objects_source`].
pub trait RoomObjectsSource {
    /// A snapshot of every object in `room` and the public profiles of the users they
    /// reference (`GET /api/game/room-objects`); `shard` as in [`WorldApi`]. Flags are not
    /// included.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`].
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
    /// ([`Subsystem::RoomObjects`]): screeps.com, `OpenScreeps`, and community servers with
    /// screepsmod-admin-utils.
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

#[cfg(test)]
mod tests {
    use super::chunk_base;

    #[test]
    fn chunks_start_at_multiples_of_the_chunk_size() {
        assert_eq!(chunk_base(1234, 100), 1200);
        assert_eq!(chunk_base(1200, 100), 1200);
        assert_eq!(chunk_base(19, 20), 0);
        assert_eq!(chunk_base(7, 0), 7);
    }
}
