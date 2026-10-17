//! How each kind of object is drawn: its emblem ([`Glyph`]), silhouette, size, height,
//! stacking layer and owner, and the live gauges of one object (store, hits, progress,
//! level, body parts).
//!
//! The palette is the viewer's own: owner colours are those of the world layers (own green,
//! other players red, Invaders orange, Source Keepers purple); structures are slate bodies
//! with an owner-coloured rim; resources and body parts have one colour each
//! ([`resource_color`], `PART_COLORS` in `objects.wgsl`).

use overseer::model::world::{
    GameRoomObject, GameRoomObjectStore, GameRoomObjectStoreCapacityResource,
};

use crate::mesh::Shape;

/// User id of the Invader NPC.
pub const INVADER: &str = "2";
/// User id of the Source Keeper NPC.
pub const SOURCE_KEEPER: &str = "3";

/// Owner colours (sRGB): the signed-in user, other players and the two NPCs.
const MINE: u32 = 0x4c_af_50;
const OTHER: u32 = 0xe5_39_35;
const INVADER_COLOR: u32 = 0xff_98_00;
const KEEPER_COLOR: u32 = 0xab_47_bc;
/// Prism sides (sRGB) of structures and of creeps.
const STRUCTURE_SIDE: u32 = 0x30_34_3b;
const CREEP_SIDE: u32 = 0x1d_20_25;

/// The emblem a token's fragment shader draws (`Token::kind`'s low byte; the order is that
/// of the `switch` in `objects.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Glyph {
    /// A creep: owner ring, body-part ring, carried resource.
    Creep,
    /// A power creep.
    PowerCreep,
    /// A spawn: store, spawning progress.
    Spawn,
    /// An extension.
    Extension,
    /// A tower.
    Tower,
    /// A storage.
    Storage,
    /// A terminal.
    Terminal,
    /// A container.
    Container,
    /// A link.
    Link,
    /// A lab.
    Lab,
    /// A factory.
    Factory,
    /// A power spawn.
    PowerSpawn,
    /// A nuker.
    Nuker,
    /// An observer.
    Observer,
    /// An extractor (a ring over its mineral).
    Extractor,
    /// A controller: level pips and progress.
    Controller,
    /// An energy source.
    Source,
    /// A mineral.
    Mineral,
    /// A deposit.
    Deposit,
    /// A power bank.
    PowerBank,
    /// A constructed wall (joins neighbouring walls).
    Wall,
    /// A rampart (joins neighbouring ramparts; translucent).
    Rampart,
    /// A road (links to neighbouring roads).
    Road,
    /// A construction site: progress.
    Site,
    /// A dropped resource.
    Resource,
    /// A tombstone.
    Tombstone,
    /// A ruin.
    Ruin,
    /// A nuke about to land.
    Nuke,
    /// A portal.
    Portal,
    /// A keeper lair.
    KeeperLair,
    /// An invader core.
    InvaderCore,
    /// Anything else.
    Other,
}

/// Flags of `Token::kind`'s high byte.
pub mod flag {
    /// A spawn is spawning (its progress gauge is the spawning progress).
    pub const SPAWNING: u8 = 1;
    /// A controller is reserved (its accent is the reserving user's).
    pub const RESERVED: u8 = 2;
    /// A ghost of a removed object (fading out).
    pub const GHOST: u8 = 4;
}

/// How neighbouring objects of the same kind join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// Separate.
    None,
    /// Roads: a link towards each neighbouring road (diagonals only where no orthogonal
    /// path exists).
    Link,
    /// Walls and ramparts: blocks merge with orthogonal neighbours, filling the corner
    /// between three.
    Fill,
}

/// The drawing of one kind of object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The emblem.
    pub glyph: Glyph,
    /// Prism silhouette in perspective (`None`: flat, the token only).
    pub shape: Option<Shape>,
    /// Footprint width (tiles).
    pub size: f32,
    /// Height of the top above the tile.
    pub height: f32,
    /// Pick order: objects of a higher layer are listed first when picked (drawing order
    /// follows the heights).
    pub layer: u8,
    /// Whether an owner colours the object.
    pub owned: bool,
    /// How neighbours of the same kind join.
    pub join: Join,
    /// Colour (sRGB) of the prism sides.
    pub side: u32,
}

/// Who owns an object, relative to the signed-in user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// No owner.
    None,
    /// The signed-in user.
    Mine,
    /// Another player.
    Other,
    /// The Invader NPC (user `2`).
    Invader,
    /// The Source Keeper NPC (user `3`).
    SourceKeeper,
}

impl Owner {
    /// The owner class of `user` when `me` is signed in.
    pub fn of(user: Option<&str>, me: Option<&str>) -> Self {
        match user {
            None => Self::None,
            Some(INVADER) => Self::Invader,
            Some(SOURCE_KEEPER) => Self::SourceKeeper,
            Some(user) if Some(user) == me => Self::Mine,
            Some(_) => Self::Other,
        }
    }

    /// The owner's colour (sRGB); `None` without an owner.
    pub const fn color(self) -> Option<u32> {
        match self {
            Self::None => None,
            Self::Mine => Some(MINE),
            Self::Other => Some(OTHER),
            Self::Invader => Some(INVADER_COLOR),
            Self::SourceKeeper => Some(KEEPER_COLOR),
        }
    }
}

impl Look {
    /// The look of an object.
    pub fn of(object: &GameRoomObject) -> Self {
        use GameRoomObject as O;
        use Glyph as G;
        use Shape::{Circle, Diamond, Hexagon, Octagon, Pentagon, Square, Triangle};
        let (glyph, shape, size, height, layer, owned) = match object {
            O::Creep(_) => (G::Creep, Some(Circle), 0.76, 0.32, 9, true),
            O::PowerCreep(_) => (G::PowerCreep, Some(Pentagon), 0.82, 0.36, 9, true),
            O::Spawn(_) => (G::Spawn, Some(Circle), 0.94, 0.42, 5, true),
            O::Extension(_) => (G::Extension, Some(Circle), 0.52, 0.2, 5, true),
            O::Tower(_) => (G::Tower, Some(Square), 0.66, 0.55, 5, true),
            O::Storage(_) => (G::Storage, Some(Square), 0.9, 0.5, 5, true),
            O::Terminal(_) => (G::Terminal, Some(Diamond), 0.96, 0.5, 5, true),
            O::Container(_) => (G::Container, Some(Square), 0.56, 0.28, 5, false),
            O::Link(_) => (G::Link, Some(Diamond), 0.52, 0.32, 5, true),
            O::Lab(_) => (G::Lab, Some(Circle), 0.72, 0.4, 5, true),
            O::Factory(_) => (G::Factory, Some(Hexagon), 0.92, 0.5, 5, true),
            O::PowerSpawn(_) => (G::PowerSpawn, Some(Octagon), 0.9, 0.45, 5, true),
            O::Nuker(_) => (G::Nuker, Some(Triangle), 0.96, 0.6, 5, true),
            O::Observer(_) => (G::Observer, Some(Circle), 0.44, 0.3, 5, true),
            O::Extractor(_) => (G::Extractor, None, 1.0, 0.36, 5, true),
            O::Controller(_) => (G::Controller, Some(Octagon), 0.9, 0.35, 5, true),
            O::Source(_) => (G::Source, Some(Square), 0.74, 0.28, 5, false),
            O::Mineral(_) => (G::Mineral, Some(Hexagon), 0.74, 0.28, 5, false),
            O::Deposit(_) => (G::Deposit, Some(Diamond), 0.84, 0.3, 5, false),
            O::PowerBank(_) => (G::PowerBank, Some(Octagon), 0.86, 0.45, 5, false),
            O::KeeperLair(_) => (G::KeeperLair, Some(Square), 0.86, 0.35, 5, true),
            O::InvaderCore(_) => (G::InvaderCore, Some(Hexagon), 0.88, 0.5, 5, true),
            O::Portal(_) => (G::Portal, None, 1.0, 0.04, 5, false),
            O::ConstructedWall(_) => (G::Wall, Some(Square), 1.0, 0.55, 5, false),
            O::Rampart(_) => (G::Rampart, None, 1.0, 0.8, 6, true),
            O::Road(_) => (G::Road, None, 1.0, 0.015, 2, false),
            O::ConstructionSite(_) => (G::Site, None, 0.7, 0.03, 3, true),
            O::Resource(_) => (G::Resource, None, 0.4, 0.05, 8, false),
            O::Tombstone(_) => (G::Tombstone, Some(Square), 0.38, 0.14, 7, false),
            O::Ruin(_) => (G::Ruin, None, 0.76, 0.05, 7, false),
            O::Nuke(_) => (G::Nuke, None, 1.0, 0.06, 8, false),
            O::Other(_) => (G::Other, Some(Circle), 0.4, 0.2, 1, false),
        };
        let join = match glyph {
            G::Road => Join::Link,
            G::Wall | G::Rampart => Join::Fill,
            _ => Join::None,
        };
        let side_color = if matches!(glyph, G::Creep | G::PowerCreep) {
            CREEP_SIDE
        } else {
            STRUCTURE_SIDE
        };
        Self {
            glyph,
            shape,
            size,
            height,
            layer,
            owned,
            join,
            side: side_color,
        }
    }

    /// The owner of an object of this look owned by `user` (`None` for kinds never
    /// coloured by owner).
    pub fn owner(&self, user: Option<&str>, me: Option<&str>) -> Owner {
        if self.owned {
            Owner::of(user, me)
        } else {
            Owner::None
        }
    }
}

/// The live state of one object as its token shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gauges {
    /// How full its store is (`0..1`) and the colour (sRGB) of its main resource.
    pub store: Option<(f32, u32)>,
    /// Hits left (`0..1`), when damaged.
    pub hits: Option<f32>,
    /// Progress (`0..1`): controller upgrade, construction, spawning.
    pub progress: Option<f32>,
    /// Controller level (`0..=8`).
    pub level: u8,
    /// Body part counts: move, work, carry, attack, ranged attack, heal, claim, tough.
    pub parts: [u8; 8],
    /// Flags (`flag`).
    pub flags: u8,
    /// Footprint width replacing the look's (dropped resources grow with the amount).
    pub size: Option<f32>,
    /// User whose colour the accent takes instead of the owner's (a reservation).
    pub accent_user: Option<String>,
    /// Whether the object hides (a creep still inside its spawn).
    pub hidden: bool,
}

/// Body part types in [`Gauges::parts`] order.
pub const PARTS: [&str; 8] = [
    "move",
    "work",
    "carry",
    "attack",
    "ranged_attack",
    "heal",
    "claim",
    "tough",
];

impl Gauges {
    /// The gauges of an object at game tick `time`.
    pub fn of(object: &GameRoomObject, time: Option<u64>) -> Self {
        use GameRoomObject as O;
        let mut gauges = Self::default();
        let hits = |hits: u64, max: u64| (hits < max && max > 0).then(|| ratio(hits, max));
        match object {
            O::Creep(creep) => {
                let carry = creep
                    .body
                    .iter()
                    .filter(|part| part.r#type == "carry")
                    .count();
                let capacity = creep
                    .store_capacity
                    .unwrap_or(u64::try_from(carry).unwrap_or(0) * 50);
                gauges.store = store(&creep.store, capacity);
                gauges.hits = hits(creep.hits, creep.hits_max);
                for part in &creep.body {
                    if let Some(index) = PARTS.iter().position(|name| *name == part.r#type) {
                        gauges.parts[index] = gauges.parts[index].saturating_add(1);
                    }
                }
                gauges.hidden = creep.spawning == Some(true);
            }
            O::PowerCreep(creep) => {
                gauges.store = store(&creep.store, creep.store_capacity.unwrap_or(0));
                gauges.hits = hits(creep.hits, creep.hits_max);
                gauges.level = u8::try_from(creep.level.min(25)).unwrap_or(0);
            }
            O::Spawn(spawn) => {
                gauges.store = store(&spawn.store, capacity(&spawn.store_capacity_resource));
                gauges.hits = hits(spawn.hits, spawn.hits_max);
                if let Some(spawning) = &spawn.spawning {
                    gauges.flags |= flag::SPAWNING;
                    let need = spawning.need_time.unwrap_or(0);
                    gauges.progress = Some(match time {
                        Some(time) if need > 0 => {
                            let left = spawning.spawn_time.saturating_sub(time).min(need);
                            ratio(need - left, need)
                        }
                        _ => 0.0,
                    });
                }
            }
            O::Extension(extension) => {
                gauges.store = store(
                    &extension.store,
                    capacity(&extension.store_capacity_resource),
                );
                gauges.hits = hits(extension.hits, extension.hits_max);
            }
            O::Tower(tower) => {
                gauges.store = store(&tower.store, capacity(&tower.store_capacity_resource));
                gauges.hits = hits(tower.hits, tower.hits_max);
            }
            O::Link(link) => {
                gauges.store = store(&link.store, capacity(&link.store_capacity_resource));
                gauges.hits = hits(link.hits, link.hits_max);
            }
            O::Storage(storage) => {
                gauges.store = store(&storage.store, storage.store_capacity.unwrap_or(1_000_000));
                gauges.hits = hits(storage.hits, storage.hits_max);
            }
            O::Container(container) => {
                gauges.store = store(&container.store, container.store_capacity.unwrap_or(2000));
                gauges.hits = hits(container.hits, container.hits_max);
            }
            O::Terminal(terminal) => {
                gauges.store = store(&terminal.store, terminal.store_capacity.unwrap_or(300_000));
            }
            O::Factory(factory) => {
                gauges.store = store(&factory.store, factory.store_capacity.unwrap_or(50_000));
                gauges.hits = hits(factory.hits, factory.hits_max);
                gauges.level = u8::try_from(factory.level.unwrap_or(0).min(5)).unwrap_or(0);
            }
            O::Lab(lab) => {
                // The mineral or compound a lab holds, else the energy that powers it.
                gauges.store = lab_mineral(&lab.store).or_else(|| {
                    let capacity = lab
                        .store_capacity
                        .unwrap_or_else(|| capacity(&lab.store_capacity_resource));
                    store(&lab.store, capacity)
                });
                gauges.hits = hits(lab.hits, lab.hits_max);
            }
            O::PowerSpawn(spawn) => {
                gauges.store = store(&spawn.store, capacity(&spawn.store_capacity_resource));
                gauges.hits = hits(spawn.hits, spawn.hits_max);
            }
            O::Nuker(nuker) => {
                gauges.store = store(&nuker.store, capacity(&nuker.store_capacity_resource));
                gauges.hits = hits(nuker.hits, nuker.hits_max);
            }
            O::Observer(observer) => gauges.hits = hits(observer.hits, observer.hits_max),
            O::Controller(controller) => {
                gauges.level = u8::try_from(controller.level.min(8)).unwrap_or(0);
                if let (Some(progress), Some(total)) =
                    (controller.progress, controller.progress_needed())
                {
                    gauges.progress = Some(ratio(progress.min(total), total));
                }
                if controller.user.is_none()
                    && let Some(reservation) = &controller.reservation
                {
                    gauges.flags |= flag::RESERVED;
                    gauges.accent_user = Some(reservation.user.clone());
                }
            }
            O::Source(source) => {
                if source.energy_capacity > 0 {
                    gauges.store = Some((
                        ratio(
                            source.energy.min(source.energy_capacity),
                            source.energy_capacity,
                        ),
                        resource_color("energy"),
                    ));
                }
            }
            O::Mineral(mineral) => {
                gauges.store = Some((
                    if mineral.mineral_amount > 0.0 {
                        1.0
                    } else {
                        0.0
                    },
                    resource_color(&mineral.mineral_type),
                ));
            }
            O::ConstructionSite(site) => {
                if site.progress_total > 0 {
                    gauges.progress = Some(ratio(
                        site.progress.min(site.progress_total),
                        site.progress_total,
                    ));
                }
            }
            O::Resource(resource) => {
                let kind = object.kind();
                let amount = resource.other.get(kind).copied().unwrap_or(0);
                gauges.store = Some((1.0, resource_color(kind)));
                // From a speck to most of a tile: 0.22 at 1, 0.6 at 10 000 or more.
                #[expect(clippy::cast_precision_loss, reason = "amounts far below 2^24")]
                let scale = ((amount.max(1) as f32).log10() / 4.0).clamp(0.0, 1.0);
                gauges.size = Some(0.22 + 0.38 * scale);
            }
            O::Tombstone(tombstone) => gauges.store = store(&tombstone.store, 0),
            O::Ruin(ruin) => gauges.store = store(&ruin.store, 0),
            O::PowerBank(bank) => {
                gauges.store = store(&bank.store, 0);
                gauges.hits = hits(bank.hits, bank.hits_max);
            }
            O::InvaderCore(core) => {
                gauges.hits = hits(core.hits, core.hits_max);
                gauges.level = u8::try_from(core.level.min(5)).unwrap_or(0);
            }
            O::ConstructedWall(_)
            | O::Rampart(_)
            | O::Road(_)
            | O::Extractor(_)
            | O::Deposit(_)
            | O::KeeperLair(_)
            | O::Portal(_)
            | O::Nuke(_)
            | O::Other(_) => {}
        }
        gauges
    }
}

/// `part / whole` as a fraction.
#[expect(
    clippy::cast_precision_loss,
    reason = "game amounts; f32 precision is plenty for a gauge"
)]
fn ratio(part: u64, whole: u64) -> f32 {
    (part as f32 / whole as f32).clamp(0.0, 1.0)
}

/// Total capacity of a per-resource capacity map.
fn capacity(capacity: &GameRoomObjectStoreCapacityResource) -> u64 {
    capacity.values().flatten().sum()
}

/// The resources of a store that it holds some of, with their amounts.
fn amounts(store: &GameRoomObjectStore) -> impl Iterator<Item = (&String, u64)> {
    store
        .iter()
        .filter_map(|(kind, amount)| Some((kind, u64::try_from((*amount)?).ok()?)))
        .filter(|(_, amount)| *amount > 0)
}

/// How full a store of `capacity` is (`capacity` 0: full when anything is in it) and the
/// colour of its largest resource; `None` when nothing is stored.
fn store(store: &GameRoomObjectStore, capacity: u64) -> Option<(f32, u32)> {
    let (total, largest) =
        amounts(store).fold((0_u64, None::<(&String, u64)>), |(total, largest), item| {
            let largest = match largest {
                Some(current) if current.1 >= item.1 => Some(current),
                _ => Some(item),
            };
            (total + item.1, largest)
        });
    let (kind, _) = largest?;
    let fill = if capacity == 0 {
        1.0
    } else {
        ratio(total.min(capacity), capacity)
    };
    Some((fill, resource_color(kind)))
}

/// Most of a mineral or compound a lab holds (`LAB_MINERAL_CAPACITY`).
const LAB_MINERAL_CAPACITY: u64 = 3000;

/// How full a lab is of the one mineral or compound it holds (not energy), and its colour;
/// `None` when it holds none.
fn lab_mineral(store: &GameRoomObjectStore) -> Option<(f32, u32)> {
    amounts(store)
        .filter(|(kind, _)| *kind != "energy")
        .max_by_key(|(_, amount)| *amount)
        .map(|(kind, amount)| {
            (
                ratio(amount.min(LAB_MINERAL_CAPACITY), LAB_MINERAL_CAPACITY),
                resource_color(kind),
            )
        })
}

/// The colour (sRGB) of a resource: energy amber, power crimson, each base mineral its own
/// hue, compounds the hue of their first mineral, commodities a muted teal.
pub fn resource_color(kind: &str) -> u32 {
    match kind {
        "energy" => 0xff_d1_3a,
        "power" | "ops" => 0xff_3d_6e,
        "battery" => 0xe8_c3_4a,
        _ => {
            // Compounds: skip the catalyst prefix of tier-3 boosts.
            let mineral = kind
                .strip_prefix('X')
                .filter(|rest| !rest.is_empty())
                .unwrap_or(kind);
            match mineral.chars().next() {
                Some('H') => 0xd5_dc_e3,
                Some('O') => 0xa9_b8_c6,
                Some('U') => 0x4f_c3_f7,
                Some('L') => 0x7c_e0_8a,
                Some('K') => 0xb3_88_ff,
                Some('Z') => 0xf3_b5_6b,
                Some('X') => 0xff_6f_7d,
                Some('G') => 0xf4_f4_f4,
                _ => 0x7f_b5_b0,
            }
        }
    }
}
