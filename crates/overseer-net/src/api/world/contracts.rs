//! The [`WorldApi`] implementations of the three contracts: screeps.com ([`Official`]),
//! [`OpenScreeps`] and the community server ([`Community`]).
//!
//! A macro writes the operations that every contract shares. Each contract implements the
//! other operations itself. A [`Community`] server uses screepsmod-admin-utils for the shard
//! information and the first spawn when the plugin is present.

use std::collections::BTreeMap;
use std::future::{Ready, ready};

use overseer_core::api::official::types::GameCoordinateInput;
use overseer_core::api::plugins::screepsmod_admin_utils;
use overseer_core::api::{ApiError, community, official, openscreeps};
use overseer_core::model::platform::Subsystem;
use overseer_core::model::world::PlatformHistory;
use overseer_core::model::world::{
    FlagColor, GameAddObjectIntentRequestIntent, GameBuildableStructureType,
    GameRandomNoviceRoomResponse, GameRoomDecoration, GameRoomOverviewResponse, GameRoomStatus,
    GameWorldSizeResponse, InvaderRequest, MapStats, ShardInfo, Terrain,
};
use serde_json::Value;

use super::super::{Community, Official, OpenScreeps, ScreepsmodAdminUtils};
use super::WorldApi;
use crate::{Error, RoomHistory};

impl Official {
    /// The shard an operation names: `shard`, else the client's default shard.
    pub(super) async fn world_shard(&self, shard: Option<&str>) -> Result<Option<String>, Error> {
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

    async fn shards_info(&self) -> Result<Vec<ShardInfo>, Error> {
        let info = official::endpoints::GameShardsInfo;
        Ok(self.client.send(&info).await?.body.into())
    }

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

    fn shards_info(&self) -> impl Future<Output = Result<Vec<ShardInfo>, Error>> {
        ready(Err(Error::Unsupported(Subsystem::ShardsInfo)))
    }

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

    async fn shards_info(&self) -> Result<Vec<ShardInfo>, Error> {
        let admin_utils = self
            .screepsmod_admin_utils()
            .ok_or(Error::Unsupported(Subsystem::ShardsInfo))?;
        admin_utils.shards_info().await
    }

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
    /// screepsmod-admin-utils' shards: the named entries.
    async fn shards_info(&self) -> Result<Vec<ShardInfo>, Error> {
        let info = screepsmod_admin_utils::endpoints::GameShardsInfo;
        Ok(self.client.send(&info).await?.body.into())
    }

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
