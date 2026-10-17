//! The actions of the room screen: flags, construction sites, intents, names and the first
//! spawn.

use std::collections::BTreeMap;

use overseer_net::api::WorldApi as _;
use overseer_net::model::world::{
    self, FlagColor, GameAddObjectIntentRequestIntent, GameBuildableStructureType,
};
use serde::Serialize;
use serde_json::Value;

use super::Rooms;
use crate::Error;

/// The payload of an intent addressed to one room object, sent by [`Rooms::object_intent`]:
/// an intent type of [`overseer_net::model::world`] (`GameIntent*`) and the engine's name for
/// it. Implement it for intents of
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
            impl $kind for world::$payload {
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
        let at = (coordinate(x)?, coordinate(y)?);
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        api.create_flag(shard.as_deref(), room, at, name, colors)
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
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        api.change_flag_colors(shard.as_deref(), room, name, colors)
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
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        api.remove_flag(shard.as_deref(), room, name).await?;
        Ok(())
    }

    /// The first `Flag<N>` name none of the user's flags has
    /// (`POST /api/game/gen-unique-flag-name`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn unique_flag_name(&self, shard: Option<&str>) -> Result<String, Error> {
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        Ok(api.unique_flag_name(shard.as_deref()).await?)
    }

    /// Whether none of the user's flags is named `name`
    /// (`POST /api/game/check-unique-flag-name`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn is_flag_name_free(&self, name: &str, shard: Option<&str>) -> Result<bool, Error> {
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        Ok(api.is_flag_name_free(shard.as_deref(), name).await?)
    }

    /// The first `Spawn<N>` name none of the user's spawns and spawn construction sites has
    /// (`POST /api/game/gen-unique-object-name`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn unique_spawn_name(&self, shard: Option<&str>) -> Result<String, Error> {
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        Ok(api.unique_spawn_name(shard.as_deref()).await?)
    }

    /// Whether none of the user's spawns and spawn construction sites is named `name`
    /// (`POST /api/game/check-unique-object-name`).
    ///
    /// # Errors
    ///
    /// See [`Rooms`].
    pub async fn is_spawn_name_free(&self, name: &str, shard: Option<&str>) -> Result<bool, Error> {
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        Ok(api.is_spawn_name_free(shard.as_deref(), name).await?)
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
        let at = (coordinate(x)?, coordinate(y)?);
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        let site = api.create_construction_site(shard.as_deref(), room, at, structure, name);
        Ok(site.await?)
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
        let intent = world::GameIntentRemoveConstructionSite {
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
        let intent = world::GameIntentDestroyStructure {
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
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        api.set_notify_when_attacked(shard.as_deref(), object, enabled)
            .await?;
        Ok(())
    }

    /// Queues `intent` for the object `object` in `room`, executed next tick as if the user's
    /// code issued it (`POST /api/game/add-object-intent`). For example
    /// [`GameIntentHarvest`](world::GameIntentHarvest) for a creep.
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
        let payload = payload(intent)?;
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        api.add_global_intent(shard.as_deref(), I::NAME, payload)
            .await?;
        Ok(())
    }

    /// Places the user's first spawn at `x`, `y` in `room` and starts playing
    /// (`POST /api/game/place-spawn`); `name` names it (unnamed when `None`, except on
    /// community servers with screepsmod-admin-utils, which require a name: the next free
    /// `Spawn<N>` name is used there).
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
        let at = (coordinate(x)?, coordinate(y)?);
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        api.place_spawn(shard.as_deref(), room, at, name).await?;
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
        let shard = self.overseer.resolve_shard(shard);
        let api = self.overseer.user_api()?;
        api.add_object_intent(shard.as_deref(), (room, target), name, intent)
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
