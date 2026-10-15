//! The actions of the room screen: flags, construction sites, intents, names and the first
//! spawn.

use std::collections::BTreeMap;

use overseer_net::core::api::types::{
    self, GameAddObjectIntentRequestIntent, GameBuildableStructureType, GameCoordinateInput,
};
use overseer_net::core::api::{ApiError, endpoints};
use serde::Serialize;
use serde_json::Value;

use super::Rooms;
use crate::Error;

/// A flag colour (`COLOR_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FlagColor {
    /// `COLOR_RED` (1).
    Red = 1,
    /// `COLOR_PURPLE` (2).
    Purple = 2,
    /// `COLOR_BLUE` (3).
    Blue = 3,
    /// `COLOR_CYAN` (4).
    Cyan = 4,
    /// `COLOR_GREEN` (5).
    Green = 5,
    /// `COLOR_YELLOW` (6).
    Yellow = 6,
    /// `COLOR_ORANGE` (7).
    Orange = 7,
    /// `COLOR_BROWN` (8).
    Brown = 8,
    /// `COLOR_GREY` (9).
    Grey = 9,
    /// `COLOR_WHITE` (10).
    White = 10,
}

impl FlagColor {
    /// Every colour, in the order of the game constants.
    pub const ALL: [Self; 10] = [
        Self::Red,
        Self::Purple,
        Self::Blue,
        Self::Cyan,
        Self::Green,
        Self::Yellow,
        Self::Orange,
        Self::Brown,
        Self::Grey,
        Self::White,
    ];

    /// The game constant's value (1 to 10), as in requests and in
    /// [`Flag::color`](overseer_net::core::socket::Flag::color).
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// The colour of a game constant's value; `None` outside 1 to 10.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            1 => Self::Red,
            2 => Self::Purple,
            3 => Self::Blue,
            4 => Self::Cyan,
            5 => Self::Green,
            6 => Self::Yellow,
            7 => Self::Orange,
            8 => Self::Brown,
            9 => Self::Grey,
            10 => Self::White,
            _ => return None,
        })
    }
}

/// The payload of an intent addressed to one room object, sent by [`Rooms::object_intent`]:
/// a generated `GameIntent*` type and the engine's name for it. Implement it for intents of
/// server mods (`customIntentTypes`).
pub trait ObjectIntent: Serialize {
    /// The engine's intent name, for example `harvest`.
    const NAME: &'static str;
}

/// The payload of a room-level intent (target `room`), sent by [`Rooms::room_intents`].
pub trait RoomIntent: Serialize {
    /// The engine's intent name, for example `destroyStructure`.
    const NAME: &'static str;
}

/// The payload of an account-level intent (market, power creeps), sent by
/// [`Rooms::global_intent`].
pub trait GlobalIntent: Serialize {
    /// The engine's intent name, for example `cancelOrder`.
    const NAME: &'static str;
}

macro_rules! intents {
    ($kind:ident: $($payload:ident => $name:literal),* $(,)?) => {
        $(
            impl $kind for types::$payload {
                const NAME: &'static str = $name;
            }
        )*
    };
}

intents!(ObjectIntent:
    GameIntentAttack => "attack",
    GameIntentAttackController => "attackController",
    GameIntentBoostCreep => "boostCreep",
    GameIntentBuild => "build",
    GameIntentClaimController => "claimController",
    GameIntentCreateCreep => "createCreep",
    GameIntentDismantle => "dismantle",
    GameIntentDrop => "drop",
    GameIntentEnableRoom => "enableRoom",
    GameIntentGenerateSafeMode => "generateSafeMode",
    GameIntentHarvest => "harvest",
    GameIntentHeal => "heal",
    GameIntentLaunchNuke => "launchNuke",
    GameIntentMove => "move",
    GameIntentNotifyWhenAttacked => "notifyWhenAttacked",
    GameIntentObserveRoom => "observeRoom",
    GameIntentPickup => "pickup",
    GameIntentProduce => "produce",
    GameIntentPull => "pull",
    GameIntentRangedAttack => "rangedAttack",
    GameIntentRangedHeal => "rangedHeal",
    GameIntentRecycleCreep => "recycleCreep",
    GameIntentRenew => "renew",
    GameIntentRenewCreep => "renewCreep",
    GameIntentRepair => "repair",
    GameIntentReserveController => "reserveController",
    GameIntentReverseReaction => "reverseReaction",
    GameIntentRunReaction => "runReaction",
    GameIntentSay => "say",
    GameIntentSend => "send",
    GameIntentSetPublic => "setPublic",
    GameIntentSetSpawnDirections => "setSpawnDirections",
    GameIntentSignController => "signController",
    GameIntentTransfer => "transfer",
    GameIntentUnboostCreep => "unboostCreep",
    GameIntentUpgradeController => "upgradeController",
    GameIntentUsePower => "usePower",
    GameIntentWithdraw => "withdraw",
);

intents!(RoomIntent:
    GameIntentCreateConstructionSite => "createConstructionSite",
    GameIntentCreateFlag => "createFlag",
    GameIntentDestroyStructure => "destroyStructure",
    GameIntentRemoveConstructionSite => "removeConstructionSite",
    GameIntentRemoveFlag => "removeFlag",
);

intents!(GlobalIntent:
    GameIntentCancelOrder => "cancelOrder",
    GameIntentChangeOrderPrice => "changeOrderPrice",
    GameIntentCreateOrder => "createOrder",
    GameIntentCreatePowerCreep => "createPowerCreep",
    GameIntentDeal => "deal",
    GameIntentDeletePowerCreep => "deletePowerCreep",
    GameIntentExtendOrder => "extendOrder",
    GameIntentRenamePowerCreep => "renamePowerCreep",
    GameIntentSpawnPowerCreep => "spawnPowerCreep",
    GameIntentSuicidePowerCreep => "suicidePowerCreep",
    GameIntentUpgradePowerCreep => "upgradePowerCreep",
);

/// The actions of the room screen, executed by the server without room visibility. `shard`
/// names the room's shard (`None`: the default shard); it is ignored by single-world servers.
/// Every action needs a signed-in user: without one it fails with [`Error::Anonymous`]; after
/// [`Overseer::shutdown`](crate::Overseer::shutdown) with [`Error::Closed`]. Refusals of the
/// server (`invalid location`, `not a controller owner`, …) are [`Error::Net`].
impl Rooms {
    /// Creates the flag `name` at `x`, `y` in `room`, or moves the user's flag of that name
    /// there (`POST /api/game/create-flag`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`]; [`Error::InvalidInput`] for a coordinate outside 0 to 49.
    pub async fn create_flag(
        &self,
        room: &str,
        x: u8,
        y: u8,
        name: &str,
        colors: (FlagColor, FlagColor),
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let body = types::GameCreateFlagRequest {
            color: colors.0.code().into(),
            name: name.to_owned(),
            room: room.to_owned(),
            secondary_color: colors.1.code().into(),
            shard: self.overseer.resolve_shard(shard),
            x: coordinate(x)?.into(),
            y: coordinate(y)?.into(),
        };
        self.overseer
            .send(&endpoints::GameCreateFlag { body })
            .await?;
        Ok(())
    }

    /// Recolours the user's flag `name` in `room` with `(primary, secondary)` colours
    /// (`POST /api/game/change-flag-color`). An unknown name changes nothing.
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn change_flag_colors(
        &self,
        room: &str,
        name: &str,
        colors: (FlagColor, FlagColor),
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let body = types::GameChangeFlagColorRequest {
            color: colors.0.code().into(),
            name: name.to_owned(),
            room: Some(room.to_owned()),
            secondary_color: colors.1.code().into(),
            shard: self.overseer.resolve_shard(shard),
        };
        self.overseer
            .send(&endpoints::GameChangeFlagColor { body })
            .await?;
        Ok(())
    }

    /// Removes the user's flag `name` from `room` (`POST /api/game/remove-flag`). An unknown
    /// name changes nothing.
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn remove_flag(
        &self,
        room: &str,
        name: &str,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let body = types::GameRemoveFlagRequest {
            name: name.to_owned(),
            room: room.to_owned(),
            shard: self.overseer.resolve_shard(shard),
        };
        self.overseer
            .send(&endpoints::GameRemoveFlag { body })
            .await?;
        Ok(())
    }

    /// The first `Flag<N>` name none of the user's flags has
    /// (`POST /api/game/gen-unique-flag-name`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn unique_flag_name(&self, shard: Option<&str>) -> Result<String, Error> {
        let body = types::GameShardOnlyRequest {
            shard: self.overseer.resolve_shard(shard),
        };
        let request = endpoints::GameGenUniqueFlagName { body };
        Ok(self.overseer.send(&request).await?.name)
    }

    /// Whether none of the user's flags is named `name`
    /// (`POST /api/game/check-unique-flag-name`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn is_flag_name_free(&self, name: &str, shard: Option<&str>) -> Result<bool, Error> {
        let body = types::GameCheckUniqueFlagNameRequest {
            name: name.to_owned(),
            shard: self.overseer.resolve_shard(shard),
        };
        name_free(
            self.overseer
                .send(&endpoints::GameCheckUniqueFlagName { body })
                .await,
        )
    }

    /// The first `Spawn<N>` name none of the user's spawns and spawn construction sites has
    /// (`POST /api/game/gen-unique-object-name`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn unique_spawn_name(&self, shard: Option<&str>) -> Result<String, Error> {
        let body = types::GameGenUniqueObjectNameRequest {
            shard: self.overseer.resolve_shard(shard),
            r#type: types::GameGenUniqueObjectNameRequestType::Spawn,
        };
        let request = endpoints::GameGenUniqueObjectName { body };
        Ok(self.overseer.send(&request).await?.name)
    }

    /// Whether none of the user's spawns and spawn construction sites is named `name`
    /// (`POST /api/game/check-unique-object-name`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn is_spawn_name_free(&self, name: &str, shard: Option<&str>) -> Result<bool, Error> {
        let body = types::GameCheckUniqueObjectNameRequest {
            name: name.to_owned(),
            shard: self.overseer.resolve_shard(shard),
            r#type: types::GameCheckUniqueObjectNameRequestType::Spawn,
        };
        name_free(
            self.overseer
                .send(&endpoints::GameCheckUniqueObjectName { body })
                .await,
        )
    }

    /// Places a construction site of `structure` at `x`, `y` in `room`
    /// (`POST /api/game/create-construction`); `name` names a spawn (required for spawns, see
    /// [`Self::unique_spawn_name`]). Returns the new site's id when the server reports it.
    ///
    /// # Errors
    ///
    /// See [`Rooms`]; [`Error::InvalidInput`] for a coordinate outside 0 to 49.
    pub async fn create_construction_site(
        &self,
        room: &str,
        x: u8,
        y: u8,
        structure: GameBuildableStructureType,
        name: Option<&str>,
        shard: Option<&str>,
    ) -> Result<Option<String>, Error> {
        let body = types::GameCreateConstructionRequest {
            name: name.map(str::to_owned),
            room: room.to_owned(),
            shard: self.overseer.resolve_shard(shard),
            structure_type: structure,
            x: GameCoordinateInput::Integer(coordinate(x)?.into()),
            y: GameCoordinateInput::Integer(coordinate(y)?.into()),
        };
        let request = endpoints::GameCreateConstruction { body };
        Ok(self.overseer.send(&request).await?.id)
    }

    /// Removes the user's construction site `site` in `room` next tick (room intent
    /// `removeConstructionSite`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn remove_construction_site(
        &self,
        room: &str,
        site: &str,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let intent = types::GameIntentRemoveConstructionSite {
            id: Some(site.to_owned()),
            room_name: Some(room.to_owned()),
        };
        self.room_intents(room, &[intent], shard).await
    }

    /// Destroys the user's structure `structure` in `room` next tick (room intent
    /// `destroyStructure`, as `Structure.destroy()`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn destroy_structure(
        &self,
        room: &str,
        structure: &str,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let intent = types::GameIntentDestroyStructure {
            id: Some(structure.to_owned()),
            room_name: Some(room.to_owned()),
        };
        self.room_intents(room, &[intent], shard).await
    }

    /// Kills the user's creep `creep` in `room` next tick (object intent `suicide`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn suicide(&self, room: &str, creep: &str, shard: Option<&str>) -> Result<(), Error> {
        self.send_object_intent(room, creep, "suicide", id_payload(creep), shard)
            .await
    }

    /// Gives up the user's controller `controller` in `room` next tick (object intent
    /// `unclaim`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn unclaim(
        &self,
        room: &str,
        controller: &str,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        self.send_object_intent(room, controller, "unclaim", id_payload(controller), shard)
            .await
    }

    /// Activates safe mode on the user's controller `controller` in `room` next tick (object
    /// intent `activateSafeMode`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`]; the server refuses while one of the user's controllers is in safe mode
    /// (`safe mode active already`).
    pub async fn activate_safe_mode(
        &self,
        room: &str,
        controller: &str,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let payload = id_payload(controller);
        self.send_object_intent(room, controller, "activateSafeMode", payload, shard)
            .await
    }

    /// Sets whether the user is notified when the object `object` is attacked
    /// (`POST /api/game/set-notify-when-attacked`). Takes effect at once.
    ///
    /// # Errors
    ///
    /// See [`Rooms`]; the server refuses objects of other users (`not owner`).
    pub async fn set_notify_when_attacked(
        &self,
        object: &str,
        enabled: bool,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let body = types::GameSetNotifyWhenAttackedRequest {
            id: object.to_owned(),
            enabled: Some(enabled),
            shard: self.overseer.resolve_shard(shard),
        };
        self.overseer
            .send(&endpoints::GameSetNotifyWhenAttacked { body })
            .await?;
        Ok(())
    }

    /// Queues `intent` for the object `object` in `room`, executed next tick as if the user's
    /// code issued it (`POST /api/game/add-object-intent`). For example
    /// [`GameIntentHarvest`](types::GameIntentHarvest) for a creep.
    ///
    /// # Errors
    ///
    /// See [`Rooms`]; [`Error::InvalidInput`] when `intent` does not serialize to a JSON
    /// object.
    pub async fn object_intent<I: ObjectIntent>(
        &self,
        room: &str,
        object: &str,
        intent: &I,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let payload = GameAddObjectIntentRequestIntent::Map(payload(intent)?);
        self.send_object_intent(room, object, I::NAME, payload, shard)
            .await
    }

    /// Queues room-level `intents` of one kind for `room` (target `room`), executed next tick
    /// (`POST /api/game/add-object-intent`).
    ///
    /// # Errors
    ///
    /// See [`Rooms::object_intent`].
    pub async fn room_intents<I: RoomIntent>(
        &self,
        room: &str,
        intents: &[I],
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let payloads = intents.iter().map(payload).collect::<Result<_, _>>()?;
        let payload = GameAddObjectIntentRequestIntent::List(payloads);
        self.send_object_intent(room, "room", I::NAME, payload, shard)
            .await
    }

    /// Queues the account-level `intent` (market orders, power creeps), executed next tick
    /// (`POST /api/game/add-global-intent`).
    ///
    /// # Errors
    ///
    /// See [`Rooms::object_intent`].
    pub async fn global_intent<I: GlobalIntent>(
        &self,
        intent: &I,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let body = types::GameAddGlobalIntentRequest {
            intent: Some(payload(intent)?),
            name: I::NAME.to_owned(),
            shard: self.overseer.resolve_shard(shard),
        };
        self.overseer
            .send(&endpoints::GameAddGlobalIntent { body })
            .await?;
        Ok(())
    }

    /// Places the user's first spawn at `x`, `y` in `room` and starts playing
    /// (`POST /api/game/place-spawn`); `name` names it (unnamed when `None`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`]; [`Error::InvalidInput`] for a coordinate outside 0 to 49. The server
    /// refuses when the user already plays (`already playing`), the room is taken
    /// (`room busy`) or the spot is invalid (`invalid location`).
    pub async fn place_spawn(
        &self,
        room: &str,
        x: u8,
        y: u8,
        name: Option<&str>,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let body = types::GamePlaceSpawnRequest {
            name: name.map(str::to_owned),
            room: room.to_owned(),
            shard: self.overseer.resolve_shard(shard),
            x: GameCoordinateInput::Integer(coordinate(x)?.into()),
            y: GameCoordinateInput::Integer(coordinate(y)?.into()),
        };
        self.overseer
            .send(&endpoints::GamePlaceSpawn { body })
            .await?;
        Ok(())
    }

    async fn send_object_intent(
        &self,
        room: &str,
        target: &str,
        name: &str,
        intent: GameAddObjectIntentRequestIntent,
        shard: Option<&str>,
    ) -> Result<(), Error> {
        let body = types::GameAddObjectIntentRequest {
            id: target.to_owned(),
            intent: Some(intent),
            name: name.to_owned(),
            room: room.to_owned(),
            shard: self.overseer.resolve_shard(shard),
        };
        self.overseer
            .send(&endpoints::GameAddObjectIntent { body })
            .await?;
        Ok(())
    }
}

/// `value` when it is a room coordinate (0 to 49).
fn coordinate(value: u8) -> Result<u8, Error> {
    if value <= 49 {
        Ok(value)
    } else {
        Err(Error::InvalidInput(format!(
            "room coordinate {value} is outside 0 to 49"
        )))
    }
}

/// The JSON object an intent payload serializes to.
fn payload<I: Serialize>(intent: &I) -> Result<BTreeMap<String, Value>, Error> {
    match serde_json::to_value(intent) {
        Ok(Value::Object(fields)) => Ok(fields.into_iter().collect()),
        Ok(_) => Err(Error::InvalidInput(
            "an intent payload must be a JSON object".to_owned(),
        )),
        Err(error) => Err(Error::InvalidInput(format!("intent payload: {error}"))),
    }
}

/// The `{id}` payload the official client sends with argument-less object intents.
fn id_payload(id: &str) -> GameAddObjectIntentRequestIntent {
    let payload = BTreeMap::from([("id".to_owned(), Value::String(id.to_owned()))]);
    GameAddObjectIntentRequestIntent::Map(payload)
}

/// A name check's reply: `name exists` is a taken name, not a failure.
fn name_free<T>(reply: Result<T, Error>) -> Result<bool, Error> {
    match reply {
        Ok(_) => Ok(true),
        Err(Error::Net(overseer_net::Error::Api(ApiError::Server(message))))
            if message == "name exists" =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::FlagColor;

    #[test]
    fn flag_colors_round_trip_their_codes() {
        for (index, color) in FlagColor::ALL.into_iter().enumerate() {
            let code = u8::try_from(index + 1).unwrap();
            assert_eq!(color.code(), code);
            assert_eq!(FlagColor::from_code(code), Some(color));
        }
        assert_eq!(FlagColor::from_code(0), None);
        assert_eq!(FlagColor::from_code(11), None);
    }
}
