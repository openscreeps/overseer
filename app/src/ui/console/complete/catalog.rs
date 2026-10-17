//! The Screeps API that the console completes without asking the server: the global names,
//! the members of the game objects, and the common constants.
//!
//! The values of the resource, structure and body part constants come from the generated
//! enums of `overseer-core`, the flag colours from its `FlagColor`.

use overseer::core::api::official::types::{GameIntentCreateCreepBodyItem as Part, ResourceType};
use overseer::model::world::{FlagColor, GameBuildableStructureType as Structure};

/// What a member is, shown after its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An object with members.
    Object,
    /// A function.
    Method,
    /// A number.
    Number,
    /// A string.
    Text,
    /// A boolean.
    Bool,
    /// An array.
    Array,
    /// A constructor.
    Class,
}

impl Kind {
    /// The word shown after the member's name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Method => "method",
            Self::Number => "number",
            Self::Text => "string",
            Self::Bool => "boolean",
            Self::Array => "array",
            Self::Class => "class",
        }
    }
}

/// The type of a value: which members it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Type {
    /// Members not in the catalog.
    Opaque,
    /// `Game`.
    Game,
    /// `Game.cpu`.
    Cpu,
    /// `Game.gcl` and `Game.gpl`.
    Level,
    /// `Game.map`.
    GameMap,
    /// `Game.market`.
    Market,
    /// `Game.shard`.
    Shard,
    /// `Memory`: its keys come from the server.
    Memory,
    /// `RawMemory`.
    RawMemory,
    /// `PathFinder`.
    PathFinder,
    /// `InterShardMemory`.
    InterShardMemory,
    /// `JSON`.
    Json,
    /// `console`.
    Console,
    /// `_` (lodash).
    Lodash,
    /// `Object`.
    Object,
    /// `Math`.
    Math,
    /// `Creep`.
    CreepClass,
    /// `StructureSpawn`.
    SpawnClass,
    /// `RoomPosition`.
    PositionClass,
    /// `Room`.
    RoomClass,
    /// `Game.creeps`: creeps by name.
    Creeps,
    /// `Game.spawns`: spawns by name.
    Spawns,
    /// `Game.rooms`: rooms by name.
    Rooms,
    /// `Game.flags`: flags by name.
    Flags,
    /// A creep.
    Creep,
    /// A spawn.
    Spawn,
    /// A room.
    Room,
    /// A room position.
    Position,
    /// A store.
    Store,
    /// A controller.
    Controller,
    /// A flag.
    Flag,
}

/// A named member of an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member {
    /// Its name.
    pub name: &'static str,
    /// What it is.
    pub kind: Kind,
    /// Its type.
    pub of: Type,
}

/// A member `name` of kind `kind` and type `of`.
const fn member(name: &'static str, kind: Kind, of: Type) -> Member {
    Member { name, kind, of }
}

/// A member `name` of kind `kind` that has no members in the catalog.
const fn leaf(name: &'static str, kind: Kind) -> Member {
    Member {
        name,
        kind,
        of: Type::Opaque,
    }
}

/// A method `name`.
const fn method(name: &'static str) -> Member {
    leaf(name, Kind::Method)
}

/// The value of a constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    /// A number.
    Number(i32),
    /// A string.
    Text(&'static str),
}

/// A global constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constant {
    /// Its name.
    pub name: &'static str,
    /// Its value.
    pub value: Value,
}

/// A number constant.
const fn number(name: &'static str, value: i32) -> Constant {
    Constant {
        name,
        value: Value::Number(value),
    }
}

/// A string constant.
const fn text(name: &'static str, value: &'static str) -> Constant {
    Constant {
        name,
        value: Value::Text(value),
    }
}

/// A flag colour constant.
const fn color(name: &'static str, color: FlagColor) -> Constant {
    number(name, color.code() as i32)
}

/// The global names, the most used first.
pub const GLOBALS: &[Member] = &[
    member("Game", Kind::Object, Type::Game),
    member("Memory", Kind::Object, Type::Memory),
    member("RawMemory", Kind::Object, Type::RawMemory),
    member("PathFinder", Kind::Object, Type::PathFinder),
    member("InterShardMemory", Kind::Object, Type::InterShardMemory),
    member("Creep", Kind::Class, Type::CreepClass),
    member("Room", Kind::Class, Type::RoomClass),
    member("RoomPosition", Kind::Class, Type::PositionClass),
    member("StructureSpawn", Kind::Class, Type::SpawnClass),
    member("_", Kind::Object, Type::Lodash),
    member("JSON", Kind::Object, Type::Json),
    member("console", Kind::Object, Type::Console),
    member("Object", Kind::Class, Type::Object),
    member("Math", Kind::Object, Type::Math),
];

/// `Game`.
const GAME: &[Member] = &[
    member("creeps", Kind::Object, Type::Creeps),
    member("spawns", Kind::Object, Type::Spawns),
    member("rooms", Kind::Object, Type::Rooms),
    leaf("time", Kind::Number),
    member("cpu", Kind::Object, Type::Cpu),
    leaf("structures", Kind::Object),
    leaf("constructionSites", Kind::Object),
    member("flags", Kind::Object, Type::Flags),
    leaf("powerCreeps", Kind::Object),
    member("gcl", Kind::Object, Type::Level),
    member("gpl", Kind::Object, Type::Level),
    member("map", Kind::Object, Type::GameMap),
    member("market", Kind::Object, Type::Market),
    leaf("resources", Kind::Object),
    member("shard", Kind::Object, Type::Shard),
    method("getObjectById"),
    method("notify"),
];

/// `Game.cpu`.
const CPU: &[Member] = &[
    leaf("limit", Kind::Number),
    leaf("tickLimit", Kind::Number),
    leaf("bucket", Kind::Number),
    leaf("shardLimits", Kind::Object),
    leaf("unlocked", Kind::Bool),
    leaf("unlockedTime", Kind::Number),
    method("getUsed"),
    method("getHeapStatistics"),
    method("halt"),
    method("setShardLimits"),
    method("unlock"),
    method("generatePixel"),
];

/// `Game.gcl` and `Game.gpl`.
const LEVEL: &[Member] = &[
    leaf("level", Kind::Number),
    leaf("progress", Kind::Number),
    leaf("progressTotal", Kind::Number),
];

/// `Game.map`.
const GAME_MAP: &[Member] = &[
    method("describeExits"),
    method("findExit"),
    method("findRoute"),
    method("getRoomLinearDistance"),
    method("getRoomStatus"),
    method("getRoomTerrain"),
    method("getWorldSize"),
    leaf("visual", Kind::Object),
];

/// `Game.market`.
const MARKET: &[Member] = &[
    leaf("credits", Kind::Number),
    leaf("incomingTransactions", Kind::Array),
    leaf("outgoingTransactions", Kind::Array),
    leaf("orders", Kind::Object),
    method("calcTransactionCost"),
    method("cancelOrder"),
    method("changeOrderPrice"),
    method("createOrder"),
    method("deal"),
    method("extendOrder"),
    method("getAllOrders"),
    method("getHistory"),
    method("getOrderById"),
];

/// `Game.shard`.
const SHARD: &[Member] = &[
    leaf("name", Kind::Text),
    leaf("type", Kind::Text),
    leaf("ptr", Kind::Bool),
];

/// `RawMemory`.
const RAW_MEMORY: &[Member] = &[
    leaf("segments", Kind::Object),
    leaf("foreignSegment", Kind::Object),
    method("get"),
    method("set"),
    method("setActiveSegments"),
    method("setActiveForeignSegment"),
    method("setDefaultPublicSegment"),
    method("setPublicSegments"),
];

/// `PathFinder`.
const PATH_FINDER: &[Member] = &[method("search"), leaf("CostMatrix", Kind::Class)];

/// `InterShardMemory`.
const INTER_SHARD_MEMORY: &[Member] =
    &[method("getLocal"), method("setLocal"), method("getRemote")];

/// `JSON`.
const JSON: &[Member] = &[method("stringify"), method("parse")];

/// `console`.
const CONSOLE: &[Member] = &[method("log")];

/// `_`: the lodash 3 functions most used in a console.
const LODASH: &[Member] = &[
    method("filter"),
    method("map"),
    method("sum"),
    method("sortBy"),
    method("groupBy"),
    method("countBy"),
    method("find"),
    method("forEach"),
    method("keys"),
    method("values"),
    method("size"),
    method("max"),
    method("min"),
    method("uniq"),
    method("pluck"),
    method("indexBy"),
    method("mapValues"),
    method("flatten"),
    method("chunk"),
    method("compact"),
    method("difference"),
    method("every"),
    method("some"),
    method("first"),
    method("last"),
    method("includes"),
    method("omit"),
    method("pick"),
    method("pairs"),
    method("partition"),
    method("range"),
    method("reduce"),
    method("reject"),
    method("take"),
    method("union"),
    method("without"),
    method("zip"),
];

/// `Object`.
const OBJECT: &[Member] = &[
    method("keys"),
    method("values"),
    method("entries"),
    method("assign"),
];

/// `Math`.
const MATH: &[Member] = &[
    method("floor"),
    method("ceil"),
    method("round"),
    method("min"),
    method("max"),
    method("abs"),
    method("random"),
    method("sqrt"),
];

/// `Creep`.
const CREEP_CLASS: &[Member] = &[member("prototype", Kind::Object, Type::Creep)];

/// `StructureSpawn`.
const SPAWN_CLASS: &[Member] = &[member("prototype", Kind::Object, Type::Spawn)];

/// `RoomPosition`.
const POSITION_CLASS: &[Member] = &[member("prototype", Kind::Object, Type::Position)];

/// `Room`.
const ROOM_CLASS: &[Member] = &[
    member("prototype", Kind::Object, Type::Room),
    leaf("Terrain", Kind::Class),
    method("serializePath"),
    method("deserializePath"),
];

/// A creep.
const CREEP: &[Member] = &[
    leaf("name", Kind::Text),
    member("pos", Kind::Object, Type::Position),
    member("room", Kind::Object, Type::Room),
    member("store", Kind::Object, Type::Store),
    leaf("memory", Kind::Object),
    leaf("body", Kind::Array),
    leaf("fatigue", Kind::Number),
    leaf("hits", Kind::Number),
    leaf("hitsMax", Kind::Number),
    leaf("id", Kind::Text),
    leaf("my", Kind::Bool),
    leaf("owner", Kind::Object),
    leaf("saying", Kind::Text),
    leaf("spawning", Kind::Bool),
    leaf("ticksToLive", Kind::Number),
    method("moveTo"),
    method("move"),
    method("harvest"),
    method("transfer"),
    method("withdraw"),
    method("pickup"),
    method("drop"),
    method("build"),
    method("repair"),
    method("upgradeController"),
    method("attack"),
    method("rangedAttack"),
    method("rangedMassAttack"),
    method("heal"),
    method("rangedHeal"),
    method("dismantle"),
    method("claimController"),
    method("reserveController"),
    method("attackController"),
    method("signController"),
    method("generateSafeMode"),
    method("getActiveBodyparts"),
    method("moveByPath"),
    method("pull"),
    method("say"),
    method("suicide"),
    method("cancelOrder"),
    method("notifyWhenAttacked"),
];

/// A spawn.
const SPAWN: &[Member] = &[
    leaf("name", Kind::Text),
    leaf("spawning", Kind::Object),
    member("store", Kind::Object, Type::Store),
    member("pos", Kind::Object, Type::Position),
    member("room", Kind::Object, Type::Room),
    leaf("memory", Kind::Object),
    leaf("hits", Kind::Number),
    leaf("hitsMax", Kind::Number),
    leaf("id", Kind::Text),
    leaf("my", Kind::Bool),
    leaf("owner", Kind::Object),
    leaf("structureType", Kind::Text),
    method("spawnCreep"),
    method("recycleCreep"),
    method("renewCreep"),
    method("destroy"),
    method("isActive"),
    method("notifyWhenAttacked"),
];

/// A room.
const ROOM: &[Member] = &[
    leaf("name", Kind::Text),
    member("controller", Kind::Object, Type::Controller),
    leaf("energyAvailable", Kind::Number),
    leaf("energyCapacityAvailable", Kind::Number),
    leaf("memory", Kind::Object),
    leaf("storage", Kind::Object),
    leaf("terminal", Kind::Object),
    leaf("visual", Kind::Object),
    method("find"),
    method("lookAt"),
    method("lookAtArea"),
    method("lookForAt"),
    method("lookForAtArea"),
    method("createConstructionSite"),
    method("createFlag"),
    method("findExitTo"),
    method("findPath"),
    method("getEventLog"),
    method("getPositionAt"),
    method("getTerrain"),
];

/// A room position.
const POSITION: &[Member] = &[
    leaf("x", Kind::Number),
    leaf("y", Kind::Number),
    leaf("roomName", Kind::Text),
    method("findClosestByPath"),
    method("findClosestByRange"),
    method("findInRange"),
    method("findPathTo"),
    method("getDirectionTo"),
    method("getRangeTo"),
    method("inRangeTo"),
    method("isEqualTo"),
    method("isNearTo"),
    method("look"),
    method("lookFor"),
    method("createConstructionSite"),
    method("createFlag"),
];

/// A store.
const STORE: &[Member] = &[
    method("getUsedCapacity"),
    method("getFreeCapacity"),
    method("getCapacity"),
];

/// A controller.
const CONTROLLER: &[Member] = &[
    leaf("level", Kind::Number),
    leaf("progress", Kind::Number),
    leaf("progressTotal", Kind::Number),
    leaf("ticksToDowngrade", Kind::Number),
    leaf("safeMode", Kind::Number),
    leaf("safeModeAvailable", Kind::Number),
    leaf("safeModeCooldown", Kind::Number),
    leaf("upgradeBlocked", Kind::Number),
    leaf("owner", Kind::Object),
    leaf("reservation", Kind::Object),
    leaf("sign", Kind::Object),
    leaf("isPowerEnabled", Kind::Bool),
    member("pos", Kind::Object, Type::Position),
    member("room", Kind::Object, Type::Room),
    leaf("id", Kind::Text),
    method("activateSafeMode"),
    method("unclaim"),
];

/// A flag.
const FLAG: &[Member] = &[
    leaf("name", Kind::Text),
    leaf("color", Kind::Number),
    leaf("secondaryColor", Kind::Number),
    leaf("memory", Kind::Object),
    member("pos", Kind::Object, Type::Position),
    member("room", Kind::Object, Type::Room),
    method("remove"),
    method("setColor"),
    method("setPosition"),
];

/// The members of a value of type `of` that the catalog knows.
pub const fn members(of: Type) -> &'static [Member] {
    match of {
        Type::Opaque | Type::Memory | Type::Creeps | Type::Spawns | Type::Rooms | Type::Flags => {
            &[]
        }
        Type::Game => GAME,
        Type::Cpu => CPU,
        Type::Level => LEVEL,
        Type::GameMap => GAME_MAP,
        Type::Market => MARKET,
        Type::Shard => SHARD,
        Type::RawMemory => RAW_MEMORY,
        Type::PathFinder => PATH_FINDER,
        Type::InterShardMemory => INTER_SHARD_MEMORY,
        Type::Json => JSON,
        Type::Console => CONSOLE,
        Type::Lodash => LODASH,
        Type::Object => OBJECT,
        Type::Math => MATH,
        Type::CreepClass => CREEP_CLASS,
        Type::SpawnClass => SPAWN_CLASS,
        Type::PositionClass => POSITION_CLASS,
        Type::RoomClass => ROOM_CLASS,
        Type::Creep => CREEP,
        Type::Spawn => SPAWN,
        Type::Room => ROOM,
        Type::Position => POSITION,
        Type::Store => STORE,
        Type::Controller => CONTROLLER,
        Type::Flag => FLAG,
    }
}

/// The type of the values of a collection by name (`Game.creeps`); `None` for other types.
pub const fn element(of: Type) -> Option<Type> {
    match of {
        Type::Creeps => Some(Type::Creep),
        Type::Spawns => Some(Type::Spawn),
        Type::Rooms => Some(Type::Room),
        Type::Flags => Some(Type::Flag),
        _ => None,
    }
}

/// The global constants, by group; the most used first in each group.
pub const CONSTANTS: &[Constant] = &[
    number("OK", 0),
    number("ERR_NOT_OWNER", -1),
    number("ERR_NO_PATH", -2),
    number("ERR_NAME_EXISTS", -3),
    number("ERR_BUSY", -4),
    number("ERR_NOT_FOUND", -5),
    number("ERR_NOT_ENOUGH_ENERGY", -6),
    number("ERR_NOT_ENOUGH_RESOURCES", -6),
    number("ERR_NOT_ENOUGH_EXTENSIONS", -6),
    number("ERR_INVALID_TARGET", -7),
    number("ERR_FULL", -8),
    number("ERR_NOT_IN_RANGE", -9),
    number("ERR_INVALID_ARGS", -10),
    number("ERR_TIRED", -11),
    number("ERR_NO_BODYPART", -12),
    number("ERR_RCL_NOT_ENOUGH", -14),
    number("ERR_GCL_NOT_ENOUGH", -15),
    number("FIND_SOURCES", 105),
    number("FIND_SOURCES_ACTIVE", 104),
    number("FIND_MY_CREEPS", 102),
    number("FIND_HOSTILE_CREEPS", 103),
    number("FIND_CREEPS", 101),
    number("FIND_STRUCTURES", 107),
    number("FIND_MY_STRUCTURES", 108),
    number("FIND_HOSTILE_STRUCTURES", 109),
    number("FIND_CONSTRUCTION_SITES", 111),
    number("FIND_MY_CONSTRUCTION_SITES", 114),
    number("FIND_HOSTILE_CONSTRUCTION_SITES", 115),
    number("FIND_DROPPED_RESOURCES", 106),
    number("FIND_MY_SPAWNS", 112),
    number("FIND_HOSTILE_SPAWNS", 113),
    number("FIND_TOMBSTONES", 118),
    number("FIND_RUINS", 123),
    number("FIND_MINERALS", 116),
    number("FIND_DEPOSITS", 122),
    number("FIND_FLAGS", 110),
    number("FIND_NUKES", 117),
    number("FIND_POWER_CREEPS", 119),
    number("FIND_MY_POWER_CREEPS", 120),
    number("FIND_HOSTILE_POWER_CREEPS", 121),
    number("FIND_EXIT", 10),
    number("FIND_EXIT_TOP", 1),
    number("FIND_EXIT_RIGHT", 3),
    number("FIND_EXIT_BOTTOM", 5),
    number("FIND_EXIT_LEFT", 7),
    text("LOOK_CREEPS", "creep"),
    text("LOOK_ENERGY", "energy"),
    text("LOOK_RESOURCES", "resource"),
    text("LOOK_SOURCES", "source"),
    text("LOOK_MINERALS", "mineral"),
    text("LOOK_DEPOSITS", "deposit"),
    text("LOOK_STRUCTURES", "structure"),
    text("LOOK_FLAGS", "flag"),
    text("LOOK_CONSTRUCTION_SITES", "constructionSite"),
    text("LOOK_NUKES", "nuke"),
    text("LOOK_TERRAIN", "terrain"),
    text("LOOK_TOMBSTONES", "tombstone"),
    text("LOOK_POWER_CREEPS", "powerCreep"),
    text("LOOK_RUINS", "ruin"),
    text("STRUCTURE_SPAWN", Structure::Spawn.as_str()),
    text("STRUCTURE_EXTENSION", Structure::Extension.as_str()),
    text("STRUCTURE_ROAD", Structure::Road.as_str()),
    text("STRUCTURE_WALL", Structure::ConstructedWall.as_str()),
    text("STRUCTURE_RAMPART", Structure::Rampart.as_str()),
    text("STRUCTURE_CONTAINER", Structure::Container.as_str()),
    text("STRUCTURE_STORAGE", Structure::Storage.as_str()),
    text("STRUCTURE_TOWER", Structure::Tower.as_str()),
    text("STRUCTURE_LINK", Structure::Link.as_str()),
    text("STRUCTURE_TERMINAL", Structure::Terminal.as_str()),
    text("STRUCTURE_EXTRACTOR", Structure::Extractor.as_str()),
    text("STRUCTURE_LAB", Structure::Lab.as_str()),
    text("STRUCTURE_FACTORY", Structure::Factory.as_str()),
    text("STRUCTURE_OBSERVER", Structure::Observer.as_str()),
    text("STRUCTURE_POWER_SPAWN", Structure::PowerSpawn.as_str()),
    text("STRUCTURE_NUKER", Structure::Nuker.as_str()),
    text("STRUCTURE_CONTROLLER", "controller"),
    text("STRUCTURE_KEEPER_LAIR", "keeperLair"),
    text("STRUCTURE_PORTAL", "portal"),
    text("STRUCTURE_POWER_BANK", "powerBank"),
    text("STRUCTURE_INVADER_CORE", "invaderCore"),
    text("MOVE", Part::Move.as_str()),
    text("WORK", Part::Work.as_str()),
    text("CARRY", Part::Carry.as_str()),
    text("ATTACK", Part::Attack.as_str()),
    text("RANGED_ATTACK", Part::RangedAttack.as_str()),
    text("HEAL", Part::Heal.as_str()),
    text("CLAIM", Part::Claim.as_str()),
    text("TOUGH", Part::Tough.as_str()),
    number("TERRAIN_MASK_WALL", 1),
    number("TERRAIN_MASK_SWAMP", 2),
    number("TERRAIN_MASK_LAVA", 4),
    number("TOP", 1),
    number("TOP_RIGHT", 2),
    number("RIGHT", 3),
    number("BOTTOM_RIGHT", 4),
    number("BOTTOM", 5),
    number("BOTTOM_LEFT", 6),
    number("LEFT", 7),
    number("TOP_LEFT", 8),
    color("COLOR_RED", FlagColor::Red),
    color("COLOR_PURPLE", FlagColor::Purple),
    color("COLOR_BLUE", FlagColor::Blue),
    color("COLOR_CYAN", FlagColor::Cyan),
    color("COLOR_GREEN", FlagColor::Green),
    color("COLOR_YELLOW", FlagColor::Yellow),
    color("COLOR_ORANGE", FlagColor::Orange),
    color("COLOR_BROWN", FlagColor::Brown),
    color("COLOR_GREY", FlagColor::Grey),
    color("COLOR_WHITE", FlagColor::White),
    text("RESOURCE_ENERGY", ResourceType::Energy.as_str()),
    text("RESOURCE_POWER", ResourceType::Power.as_str()),
    text("RESOURCE_OPS", ResourceType::Ops.as_str()),
    text("RESOURCE_HYDROGEN", ResourceType::H.as_str()),
    text("RESOURCE_OXYGEN", ResourceType::O.as_str()),
    text("RESOURCE_UTRIUM", ResourceType::U.as_str()),
    text("RESOURCE_LEMERGIUM", ResourceType::L.as_str()),
    text("RESOURCE_KEANIUM", ResourceType::K.as_str()),
    text("RESOURCE_ZYNTHIUM", ResourceType::Z.as_str()),
    text("RESOURCE_CATALYST", ResourceType::X.as_str()),
    text("RESOURCE_GHODIUM", ResourceType::G.as_str()),
    text("RESOURCE_HYDROXIDE", ResourceType::Oh.as_str()),
    text("RESOURCE_ZYNTHIUM_KEANITE", ResourceType::Zk.as_str()),
    text("RESOURCE_UTRIUM_LEMERGITE", ResourceType::Ul.as_str()),
    text("RESOURCE_UTRIUM_HYDRIDE", ResourceType::Uh.as_str()),
    text("RESOURCE_UTRIUM_OXIDE", ResourceType::Uo.as_str()),
    text("RESOURCE_KEANIUM_HYDRIDE", ResourceType::Kh.as_str()),
    text("RESOURCE_KEANIUM_OXIDE", ResourceType::Ko.as_str()),
    text("RESOURCE_LEMERGIUM_HYDRIDE", ResourceType::Lh.as_str()),
    text("RESOURCE_LEMERGIUM_OXIDE", ResourceType::Lo.as_str()),
    text("RESOURCE_ZYNTHIUM_HYDRIDE", ResourceType::Zh.as_str()),
    text("RESOURCE_ZYNTHIUM_OXIDE", ResourceType::Zo.as_str()),
    text("RESOURCE_GHODIUM_HYDRIDE", ResourceType::Gh.as_str()),
    text("RESOURCE_GHODIUM_OXIDE", ResourceType::Go.as_str()),
    text("RESOURCE_UTRIUM_ACID", ResourceType::Uh2O.as_str()),
    text("RESOURCE_UTRIUM_ALKALIDE", ResourceType::Uho2.as_str()),
    text("RESOURCE_KEANIUM_ACID", ResourceType::Kh2O.as_str()),
    text("RESOURCE_KEANIUM_ALKALIDE", ResourceType::Kho2.as_str()),
    text("RESOURCE_LEMERGIUM_ACID", ResourceType::Lh2O.as_str()),
    text("RESOURCE_LEMERGIUM_ALKALIDE", ResourceType::Lho2.as_str()),
    text("RESOURCE_ZYNTHIUM_ACID", ResourceType::Zh2O.as_str()),
    text("RESOURCE_ZYNTHIUM_ALKALIDE", ResourceType::Zho2.as_str()),
    text("RESOURCE_GHODIUM_ACID", ResourceType::Gh2O.as_str()),
    text("RESOURCE_GHODIUM_ALKALIDE", ResourceType::Gho2.as_str()),
    text(
        "RESOURCE_CATALYZED_UTRIUM_ACID",
        ResourceType::Xuh2O.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_UTRIUM_ALKALIDE",
        ResourceType::Xuho2.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_KEANIUM_ACID",
        ResourceType::Xkh2O.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_KEANIUM_ALKALIDE",
        ResourceType::Xkho2.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_LEMERGIUM_ACID",
        ResourceType::Xlh2O.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_LEMERGIUM_ALKALIDE",
        ResourceType::Xlho2.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_ZYNTHIUM_ACID",
        ResourceType::Xzh2O.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_ZYNTHIUM_ALKALIDE",
        ResourceType::Xzho2.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_GHODIUM_ACID",
        ResourceType::Xgh2O.as_str(),
    ),
    text(
        "RESOURCE_CATALYZED_GHODIUM_ALKALIDE",
        ResourceType::Xgho2.as_str(),
    ),
    text("RESOURCE_SILICON", ResourceType::Silicon.as_str()),
    text("RESOURCE_METAL", ResourceType::Metal.as_str()),
    text("RESOURCE_BIOMASS", ResourceType::Biomass.as_str()),
    text("RESOURCE_MIST", ResourceType::Mist.as_str()),
    text("RESOURCE_UTRIUM_BAR", ResourceType::UtriumBar.as_str()),
    text(
        "RESOURCE_LEMERGIUM_BAR",
        ResourceType::LemergiumBar.as_str(),
    ),
    text("RESOURCE_ZYNTHIUM_BAR", ResourceType::ZynthiumBar.as_str()),
    text("RESOURCE_KEANIUM_BAR", ResourceType::KeaniumBar.as_str()),
    text("RESOURCE_GHODIUM_MELT", ResourceType::GhodiumMelt.as_str()),
    text("RESOURCE_OXIDANT", ResourceType::Oxidant.as_str()),
    text("RESOURCE_REDUCTANT", ResourceType::Reductant.as_str()),
    text("RESOURCE_PURIFIER", ResourceType::Purifier.as_str()),
    text("RESOURCE_BATTERY", ResourceType::Battery.as_str()),
    text("RESOURCE_COMPOSITE", ResourceType::Composite.as_str()),
    text("RESOURCE_CRYSTAL", ResourceType::Crystal.as_str()),
    text("RESOURCE_LIQUID", ResourceType::Liquid.as_str()),
    text("RESOURCE_WIRE", ResourceType::Wire.as_str()),
    text("RESOURCE_SWITCH", ResourceType::Switch.as_str()),
    text("RESOURCE_TRANSISTOR", ResourceType::Transistor.as_str()),
    text("RESOURCE_MICROCHIP", ResourceType::Microchip.as_str()),
    text("RESOURCE_CIRCUIT", ResourceType::Circuit.as_str()),
    text("RESOURCE_DEVICE", ResourceType::Device.as_str()),
    text("RESOURCE_CELL", ResourceType::Cell.as_str()),
    text("RESOURCE_PHLEGM", ResourceType::Phlegm.as_str()),
    text("RESOURCE_TISSUE", ResourceType::Tissue.as_str()),
    text("RESOURCE_MUSCLE", ResourceType::Muscle.as_str()),
    text("RESOURCE_ORGANOID", ResourceType::Organoid.as_str()),
    text("RESOURCE_ORGANISM", ResourceType::Organism.as_str()),
    text("RESOURCE_ALLOY", ResourceType::Alloy.as_str()),
    text("RESOURCE_TUBE", ResourceType::Tube.as_str()),
    text("RESOURCE_FIXTURES", ResourceType::Fixtures.as_str()),
    text("RESOURCE_FRAME", ResourceType::Frame.as_str()),
    text("RESOURCE_HYDRAULICS", ResourceType::Hydraulics.as_str()),
    text("RESOURCE_MACHINE", ResourceType::Machine.as_str()),
    text("RESOURCE_CONDENSATE", ResourceType::Condensate.as_str()),
    text("RESOURCE_CONCENTRATE", ResourceType::Concentrate.as_str()),
    text("RESOURCE_EXTRACT", ResourceType::Extract.as_str()),
    text("RESOURCE_SPIRIT", ResourceType::Spirit.as_str()),
    text("RESOURCE_EMANATION", ResourceType::Emanation.as_str()),
    text("RESOURCE_ESSENCE", ResourceType::Essence.as_str()),
];
