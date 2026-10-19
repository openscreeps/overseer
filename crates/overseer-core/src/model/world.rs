//! World: the shards (their CPU limits and tick durations), the map (world size, map
//! statistics, room status, overview), rooms and their terrain, decorations and room objects,
//! room history, and the actions on rooms (flags, construction sites, spawns, intents,
//! invaders), and the measurement of the tick duration from game ticks ([`tick`]).
//!
//! Room objects, room users, room visuals, map visuals and room map views are the same in
//! every contract (codegen re-exports the official types from the others): they are
//! re-exported here as generated, and the socket's room state uses them too.

mod map_stats;
mod room_map;
mod room_object;
mod shards;
mod terrain;
pub mod tick;

use crate::api::official::types as official;
use crate::api::plugins::screepsmod_admin_utils::types as admin_utils;

pub use map_stats::{MapStats, MapStatsMineral, MapStatsOwner, MapStatsRoom};
pub use room_object::{store_amount, store_amounts, store_capacity};
pub use shards::ShardInfo;
pub use terrain::{EncodedTerrain, SpotError, Terrain, TerrainError, Tile};

/// Every contract's room objects are these types (the room-objects reply, the socket's room
/// state and room history hold them alike).
pub use crate::api::official::types::{
    GameRoomObject, GameRoomObjectActionLog, GameRoomObjectActionLogEntry,
    GameRoomObjectActionLogEntryId, GameRoomObjectActionLogValue, GameRoomObjectBodyPart,
    GameRoomObjectConstructedWall, GameRoomObjectConstructedWallDecayTime,
    GameRoomObjectConstructionSite, GameRoomObjectContainer, GameRoomObjectController,
    GameRoomObjectCreep, GameRoomObjectDeposit, GameRoomObjectEffect, GameRoomObjectEffects,
    GameRoomObjectExtension, GameRoomObjectExtractor, GameRoomObjectFactory,
    GameRoomObjectHardSign, GameRoomObjectInvaderCore, GameRoomObjectKeeperLair, GameRoomObjectLab,
    GameRoomObjectLink, GameRoomObjectMemoryMove, GameRoomObjectMineral, GameRoomObjectNuke,
    GameRoomObjectNuker, GameRoomObjectObserver, GameRoomObjectOther, GameRoomObjectPortal,
    GameRoomObjectPosition, GameRoomObjectPowerBank, GameRoomObjectPowerCreep,
    GameRoomObjectPowerInfo, GameRoomObjectPowerSpawn, GameRoomObjectRampart,
    GameRoomObjectReservation, GameRoomObjectResource, GameRoomObjectRoad, GameRoomObjectRuin,
    GameRoomObjectRuinStructure, GameRoomObjectSign, GameRoomObjectSource, GameRoomObjectSpawn,
    GameRoomObjectSpawning, GameRoomObjectStorage, GameRoomObjectStore,
    GameRoomObjectStoreCapacityResource, GameRoomObjectStrongholdPopulation,
    GameRoomObjectStrongholdPopulationEntry, GameRoomObjectTerminal, GameRoomObjectTerminalSend,
    GameRoomObjectTombstone, GameRoomObjectTower, GameRoomObjectWallDecay,
};

/// Every contract's intent payloads (`POST /api/game/add-object-intent` and
/// `add-global-intent`) are these types.
pub use crate::api::official::types::{
    GameIntentActivateAccess, GameIntentActivateSafeMode, GameIntentAttack,
    GameIntentAttackController, GameIntentBoostCreep, GameIntentBuild, GameIntentCancelOrder,
    GameIntentCancelSpawning, GameIntentChangeOrderPrice, GameIntentClaimController,
    GameIntentCreateConstructionSite, GameIntentCreateCreep, GameIntentCreateCreepBodyItem,
    GameIntentCreateFlag, GameIntentCreateOrder, GameIntentCreatePowerCreep, GameIntentDeal,
    GameIntentDeletePowerCreep, GameIntentDestroy, GameIntentDestroyStructure, GameIntentDismantle,
    GameIntentDrop, GameIntentEnableRoom, GameIntentExtendOrder, GameIntentGeneratePixel,
    GameIntentGenerateSafeMode, GameIntentHarvest, GameIntentHeal, GameIntentLaunchNuke,
    GameIntentMove, GameIntentNotify, GameIntentNotifyWhenAttacked, GameIntentObserveRoom,
    GameIntentPickup, GameIntentProcessPower, GameIntentProduce, GameIntentPull,
    GameIntentRangedAttack, GameIntentRangedHeal, GameIntentRangedMassAttack,
    GameIntentRecycleCreep, GameIntentRemove, GameIntentRemoveConstructionSite,
    GameIntentRemoveFlag, GameIntentRenamePowerCreep, GameIntentRenew, GameIntentRenewCreep,
    GameIntentRepair, GameIntentReserveController, GameIntentReverseReaction,
    GameIntentRunReaction, GameIntentSay, GameIntentSend, GameIntentSetColor,
    GameIntentSetPosition, GameIntentSetPublic, GameIntentSetSpawnDirections,
    GameIntentSignController, GameIntentSpawnPowerCreep, GameIntentSuicide,
    GameIntentSuicidePowerCreep, GameIntentTransfer, GameIntentUnboostCreep, GameIntentUnclaim,
    GameIntentUnlockCpu, GameIntentUpgradeController, GameIntentUpgradePowerCreep,
    GameIntentUsePower, GameIntentWithdraw,
};

/// Replies and values every contract shares: room status, overview, decorations, world size,
/// room-history chunks, the public users of map statistics and room objects, and the inputs
/// of room actions.
pub use crate::api::official::types::{
    GameAddObjectIntentRequestIntent, GameBuildableStructureType, GameControllerSign,
    GameCreateInvaderRequestSize, GameCreateInvaderRequestType, GameMapStatEntry, GamePublicUser,
    GamePublicUsers, GameRandomNoviceRoomResponse, GameRoomDecoration, GameRoomOverviewResponse,
    GameRoomOverviewResponseOwner, GameRoomStatName, GameRoomStatus, GameStatPoint,
    GameWorldSizeResponse, PlatformHistory, UserDecoration, UserDecorationActivation,
    UserDecorationActivationOther,
};

/// Every contract's socket room users, room visuals, map visuals and room map views are
/// these types.
pub use crate::socket::official::types::{
    MapVisualCircleStyle, MapVisualItem, MapVisualItemCircle, MapVisualItemLine, MapVisualItemPoly,
    MapVisualItemRect, MapVisualItemText, MapVisualLineStyle, MapVisualPosition,
    MapVisualShapeStyle, MapVisualTextStyle, Position, Positions, RoomMap, RoomUser,
    RoomVisualCircleStyle, RoomVisualItem, RoomVisualItemCircle, RoomVisualItemLine,
    RoomVisualItemPoly, RoomVisualItemRect, RoomVisualItemText, RoomVisualLineStyle,
    RoomVisualShapeStyle, RoomVisualTextStyle, RoomVisualTextStyleFont,
};

/// Every object of a room and the public profiles of the users they reference
/// (`GET /api/game/room-objects`): native on screeps.com and `OpenScreeps`, provided by
/// screepsmod-admin-utils on community servers (without the `ok` marker). Flags are not room
/// objects.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomObjects {
    /// Every object matching the reference, typed by its `type`.
    pub objects: Vec<GameRoomObject>,
    /// The referenced users by user id.
    pub users: GamePublicUsers,
}

/// `From` the room-objects reply of each contract offering it: the official one (shared by
/// `OpenScreeps`) and screepsmod-admin-utils'. They hold the same fields.
macro_rules! room_objects_from {
    ($($reply:ty),* $(,)?) => {
        $(
            impl From<$reply> for RoomObjects {
                fn from(reply: $reply) -> Self {
                    Self {
                        objects: reply.objects,
                        users: reply.users,
                    }
                }
            }
        )*
    };
}

room_objects_from!(
    official::GameRoomObjectsResponse,
    admin_utils::GameRoomObjectsResponse,
);

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
    /// [`Flag::color`](crate::socket::Flag::color).
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

/// An NPC invader to summon in a room the user owns or reserves
/// (`POST /api/game/create-invader`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvaderRequest {
    /// The room.
    pub room: String,
    /// The tile's x coordinate (0 to 49).
    pub x: u8,
    /// The tile's y coordinate (0 to 49).
    pub y: u8,
    /// Small or big body.
    pub size: GameCreateInvaderRequestSize,
    /// Melee, ranged or healer.
    pub kind: GameCreateInvaderRequestType,
    /// Whether its parts are boosted.
    pub boosted: bool,
}

#[cfg(test)]
mod tests {
    use super::{FlagColor, RoomObjects};
    use crate::api::official::types as official;
    use crate::api::plugins::screepsmod_admin_utils::types as admin_utils;

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

    /// A room's objects as screeps.com and `OpenScreeps` send them, and as
    /// screepsmod-admin-utils does (without `ok`, with storage bookkeeping fields).
    #[test]
    fn room_objects_replies_convert_alike() {
        let objects = r#""objects":[
            {"_id":"c1","type":"controller","room":"W1N1","x":10,"y":12,"level":3,"user":"u1","progress":5},
            {"_id":"s1","type":"source","room":"W1N1","x":20,"y":22,"energy":3000,"energyCapacity":3000,"ticksToRegeneration":300,"meta":{"revision":0},"$loki":4}],
            "users":{"u1":{"_id":"u1","username":"Alice"},"2":{"_id":"2","username":"Invader"}}"#;
        let official: official::GameRoomObjectsResponse =
            serde_json::from_str(&format!(r#"{{"ok":1,{objects}}}"#)).unwrap();
        let plugin: admin_utils::GameRoomObjectsResponse =
            serde_json::from_str(&format!("{{{objects}}}")).unwrap();
        let room = RoomObjects::from(official);
        assert_eq!(room.objects.len(), 2);
        assert_eq!(room.users["u1"].username.as_deref(), Some("Alice"));
        assert_eq!(RoomObjects::from(plugin), room);
    }
}
