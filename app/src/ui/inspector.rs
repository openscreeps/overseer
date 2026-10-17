//! The inspector: the selected room object as typed fields per kind (hits, store contents,
//! body parts with boosts, ticks to live, fatigue, cooldowns, progress, spawning, controller
//! level and downgrade, where a portal leads, …), the other objects on its tile, then its raw
//! JSON.
//!
//! [`inspect`](fields::inspect) turns an object into [`Field`]s (pure, tested natively);
//! [`Inspector`] shows them, a portal's destination as a link through it when the inspector
//! can take the user there.

use std::collections::BTreeMap;

use overseer::map::PortalDestination;
use overseer::model::world::{GameRoomObject, RoomUser};

use crate::ui::player::{Player, npc_name};
use crate::ui::style;

mod colours;
mod fields;
mod panel;
#[cfg(test)]
mod tests;

pub use panel::Inspector;

/// What an object's fields are read against.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    /// The room's game tick (turns absolute ticks into "in N ticks").
    pub game_time: Option<u64>,
    /// The signed-in user id.
    pub me: Option<&'a str>,
    /// The users the room's objects reference, by id.
    pub users: &'a BTreeMap<String, RoomUser>,
}

impl Context<'_> {
    /// The name of the user `id` (the room's users, the two NPC ids, else the id).
    pub fn name(&self, id: &str) -> String {
        self.users.get(id).map_or_else(
            || npc_name(id).map_or_else(|| id.to_owned(), str::to_owned),
            |user| user.username.clone(),
        )
    }

    /// The user `id` with its [`name`](Self::name).
    pub fn player(&self, id: &str) -> Player {
        Player {
            id: id.to_owned(),
            name: self.name(id),
        }
    }

    /// Whether `user` is the signed-in user.
    pub fn is_me(&self, user: Option<&str>) -> bool {
        user.is_some() && user == self.me
    }

    /// Ticks from now until the game tick `tick` (`None` without a game time or once past).
    pub fn until(&self, tick: u64) -> Option<u64> {
        tick.checked_sub(self.game_time?)
    }
}

/// How a meter is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Hit points.
    Hits,
    /// Energy.
    Energy,
    /// Progress (controller, construction, spawning).
    Progress,
}

impl Tone {
    /// The classes of the meter's bar (`<progress>`).
    pub const fn class(self) -> &'static str {
        match self {
            Self::Hits => style::METER_BAR_HITS,
            Self::Energy => style::METER_BAR_ENERGY,
            Self::Progress => style::METER_BAR_PROGRESS,
        }
    }
}

/// One body part of a creep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// `move`, `work`, `carry`, …
    pub kind: String,
    /// The boosting compound.
    pub boost: Option<String>,
    /// Still has hit points.
    pub alive: bool,
}

/// The value of a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// Plain text.
    Text(String),
    /// An amount of a maximum, drawn as a bar.
    Meter {
        /// The amount.
        value: u64,
        /// The maximum.
        max: u64,
        /// The bar's colour.
        tone: Tone,
    },
    /// Store contents, largest first.
    Store {
        /// `(resource, amount)`.
        resources: Vec<(String, u64)>,
        /// Total amount.
        used: u64,
        /// Total capacity, when known.
        capacity: Option<u64>,
    },
    /// Body parts, in body order.
    Body(Vec<Part>),
    /// Where a portal leads: its text (`W5N5`, `W5N5 on shard1`) and destination.
    Destination {
        /// The text shown.
        text: String,
        /// The destination, for a link through the portal.
        to: PortalDestination,
    },
}

/// A labelled value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The label.
    pub label: &'static str,
    /// The value.
    pub value: Value,
}

/// An object as the inspector shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspected {
    /// The name (creeps, spawns), else what it is.
    pub title: String,
    /// What it is (`Creep`, `Construction site`, …).
    pub kind: String,
    /// The owner.
    pub owner: Option<Player>,
    /// Owned by the signed-in user.
    pub own: bool,
    /// The typed fields.
    pub fields: Vec<Field>,
}

/// The key that groups `object` with its kind: its `type`, `resource` for dropped resources.
pub fn kind_key(object: &GameRoomObject) -> &str {
    match object {
        GameRoomObject::Resource(_) => "resource",
        _ => object.kind(),
    }
}

/// What objects of kind `key` ([`kind_key`]) are, singular: `constructionSite` →
/// `Construction site`.
pub fn kind_label(key: &str) -> String {
    match key {
        "constructedWall" => return "Wall".to_owned(),
        "resource" => return "Dropped resource".to_owned(),
        "invaderCore" => return "Invader core".to_owned(),
        _ => {}
    }
    let mut label = String::with_capacity(key.len() + 2);
    for (index, character) in key.chars().enumerate() {
        if index == 0 {
            label.push(character.to_ascii_uppercase());
        } else if character.is_ascii_uppercase() {
            label.push(' ');
            label.push(character.to_ascii_lowercase());
        } else {
            label.push(character);
        }
    }
    label
}

/// The resource and amount of a dropped resource.
fn dropped(object: &GameRoomObject) -> Option<(&'static str, u64)> {
    let GameRoomObject::Resource(resource) = object else {
        return None;
    };
    let kind = resource.resource_type.unwrap_or(resource.r#type).as_str();
    Some((kind, resource.other.get(kind).copied().unwrap_or_default()))
}

/// How the object list and the inspector name `object`.
pub fn title(object: &GameRoomObject) -> String {
    match object {
        GameRoomObject::Creep(creep) => creep.name.clone(),
        GameRoomObject::PowerCreep(creep) => creep.name.clone(),
        GameRoomObject::Spawn(spawn) => spawn
            .name
            .clone()
            .unwrap_or_else(|| kind_label(&spawn.r#type)),
        GameRoomObject::Controller(controller) => {
            format!("Controller · level {}", controller.level)
        }
        GameRoomObject::ConstructionSite(site) => {
            format!("{} (site)", kind_label(&site.structure_type))
        }
        GameRoomObject::Mineral(mineral) => format!("Mineral {}", mineral.mineral_type),
        GameRoomObject::Deposit(deposit) => format!("Deposit {}", deposit.deposit_type),
        GameRoomObject::Tombstone(tombstone) => tombstone
            .creep_name
            .as_ref()
            .or(tombstone.power_creep_name.as_ref())
            .map_or_else(
                || "Tombstone".to_owned(),
                |name| format!("Tombstone of {name}"),
            ),
        GameRoomObject::Ruin(ruin) => format!(
            "Ruin of {}",
            kind_label(&ruin.structure.r#type).to_lowercase()
        ),
        _ => match dropped(object) {
            Some((kind, amount)) => format!("{} {kind}", number(amount)),
            None => kind_label(kind_key(object)),
        },
    }
}

/// `count` with thousands separated by narrow no-break spaces (`12 345`).
pub fn number(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() * 2);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('\u{202f}');
        }
        grouped.push(digit);
    }
    grouped
}

/// `n ticks` (`1 tick`).
pub fn ticks(count: u64) -> String {
    if count == 1 {
        "1 tick".to_owned()
    } else {
        format!("{} ticks", number(count))
    }
}
