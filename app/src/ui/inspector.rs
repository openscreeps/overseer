//! The inspector: the selected room object as typed fields per kind (hits, store contents,
//! body parts with boosts, ticks to live, fatigue, cooldowns, progress, spawning, controller
//! level and downgrade, where a portal leads, …), the other objects on its tile, then its raw
//! JSON.
//!
//! [`inspect`] turns an object into [`Field`]s (pure, tested natively); [`Inspector`] shows
//! them, a portal's destination as a link through it when the inspector can take the user
//! there.

use std::collections::BTreeMap;
use std::sync::Arc;

use leptos::prelude::*;
use overseer::RoomSnapshot;
use overseer::core::api::types::{GameRoomObject, GameRoomObjectStore};
use overseer::core::socket::types::RoomUser;
use overseer::map::{Portal, PortalDestination};

use crate::ui::style;
use crate::ui::style::concatcp;

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
        if let Some(user) = self.users.get(id) {
            return user.username.clone();
        }
        match id {
            "2" => "Invader".to_owned(),
            "3" => "Source Keeper".to_owned(),
            _ => id.to_owned(),
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
    /// The owner's name.
    pub owner: Option<String>,
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

/// The fields of an object, in order.
struct Fields<'a> {
    context: &'a Context<'a>,
    fields: Vec<Field>,
}

impl Fields<'_> {
    fn text(&mut self, label: &'static str, value: impl Into<String>) {
        self.fields.push(Field {
            label,
            value: Value::Text(value.into()),
        });
    }

    fn meter(&mut self, label: &'static str, value: u64, max: u64, tone: Tone) {
        if max > 0 {
            self.fields.push(Field {
                label,
                value: Value::Meter { value, max, tone },
            });
        }
    }

    fn hits(&mut self, hits: Option<u64>, max: Option<u64>) {
        if let (Some(hits), Some(max)) = (hits, max) {
            self.meter("Hits", hits, max, Tone::Hits);
        }
    }

    /// "in N ticks" until the game tick `tick`, when in the future.
    fn until(&mut self, label: &'static str, tick: Option<u64>) {
        if let Some(left) = tick.and_then(|tick| self.context.until(tick)) {
            self.text(label, format!("in {}", ticks(left)));
        }
    }

    /// A plain count.
    fn count(&mut self, label: &'static str, count: Option<u64>) {
        if let Some(count) = count {
            self.text(label, number(count));
        }
    }

    /// A cooldown in ticks (shown while running).
    fn cooldown(&mut self, cooldown: u64) {
        if cooldown > 0 {
            self.text("Cooldown", ticks(cooldown));
        }
    }

    fn store(&mut self, store: &GameRoomObjectStore, capacity: Option<u64>) {
        let mut resources: Vec<(String, u64)> = store
            .iter()
            .filter_map(|(resource, amount)| {
                let amount = u64::try_from((*amount)?).ok()?;
                (amount > 0).then(|| (resource.clone(), amount))
            })
            .collect();
        resources.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let used = resources.iter().map(|(_, amount)| amount).sum();
        if resources.is_empty() && capacity.is_none_or(|capacity| capacity == 0) {
            return;
        }
        self.fields.push(Field {
            label: "Store",
            value: Value::Store {
                resources,
                used,
                capacity: capacity.filter(|capacity| *capacity > 0),
            },
        });
    }

    /// The energy of a structure that only holds energy, as a meter.
    fn energy(&mut self, store: &GameRoomObjectStore, capacity: &BTreeMap<String, Option<u64>>) {
        let energy = store
            .get("energy")
            .copied()
            .flatten()
            .and_then(|energy| u64::try_from(energy).ok())
            .unwrap_or_default();
        let max = capacity
            .get("energy")
            .copied()
            .flatten()
            .unwrap_or_default();
        self.meter("Energy", energy, max, Tone::Energy);
    }
}

/// The total of a per-resource capacity.
fn total_capacity(capacity: &BTreeMap<String, Option<u64>>) -> Option<u64> {
    let total: u64 = capacity.values().flatten().sum();
    (total > 0).then_some(total)
}

/// The inspector's view of `object`.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per object kind, each short; splitting them scatters the table"
)]
pub fn inspect(object: &GameRoomObject, context: &Context<'_>) -> Inspected {
    let user = object.user();
    let mut fields = Fields {
        context,
        fields: Vec::new(),
    };
    match object {
        GameRoomObject::Creep(creep) => {
            fields.hits(Some(creep.hits), Some(creep.hits_max));
            if creep.spawning == Some(true) {
                fields.text("Ticks to live", "Spawning");
            } else if let Some(left) = creep
                .age_time
                .and_then(|age| context.until(age))
                .or(creep.ticks_to_live)
            {
                fields.text("Ticks to live", number(left));
            }
            fields.count("Fatigue", creep.fatigue);
            fields.store(&creep.store, creep.store_capacity);
            fields.fields.push(Field {
                label: "Body",
                value: Value::Body(
                    creep
                        .body
                        .iter()
                        .map(|part| Part {
                            kind: part.r#type.clone(),
                            boost: part.boost.clone(),
                            alive: part.hits > 0,
                        })
                        .collect(),
                ),
            });
            if let Some(say) = creep
                .action_log
                .as_ref()
                .and_then(|log| log.get("say").cloned().flatten())
                .and_then(|say| serde_json::to_value(say).ok())
                .and_then(|say| {
                    say.get("message")
                        .and_then(|message| message.as_str().map(str::to_owned))
                })
            {
                fields.text("Saying", say);
            }
        }
        GameRoomObject::PowerCreep(creep) => {
            fields.text(
                "Class",
                format!("{} · level {}", creep.class_name, creep.level),
            );
            fields.hits(Some(creep.hits), Some(creep.hits_max));
            if let Some(left) = context.until(creep.age_time) {
                fields.text("Ticks to live", number(left));
            }
            fields.store(&creep.store, creep.store_capacity);
            let powers: Vec<String> = creep
                .powers
                .iter()
                .map(|(power, info)| format!("{power} (level {})", info.level))
                .collect();
            if !powers.is_empty() {
                fields.text("Powers", powers.join(", "));
            }
        }
        GameRoomObject::Spawn(spawn) => {
            fields.hits(Some(spawn.hits), Some(spawn.hits_max));
            fields.energy(&spawn.store, &spawn.store_capacity_resource);
            match &spawn.spawning {
                Some(spawning) => {
                    let left = context.until(spawning.spawn_time);
                    match (spawning.need_time, left) {
                        (Some(need), Some(left)) => {
                            fields.text("Spawning", spawning.name.clone());
                            fields.meter(
                                "Progress",
                                need.saturating_sub(left),
                                need,
                                Tone::Progress,
                            );
                        }
                        _ => fields.text("Spawning", spawning.name.clone()),
                    }
                }
                None => fields.text("Spawning", "Idle"),
            }
            if spawn.off == Some(true) {
                fields.text("State", "Inactive");
            }
        }
        GameRoomObject::Extension(extension) => {
            fields.hits(Some(extension.hits), Some(extension.hits_max));
            fields.energy(&extension.store, &extension.store_capacity_resource);
            if extension.off == Some(true) {
                fields.text("State", "Inactive");
            }
        }
        GameRoomObject::Controller(controller) => {
            fields.text("Level", controller.level.to_string());
            if let (Some(progress), Some(total)) =
                (controller.progress, controller.progress_needed())
            {
                fields.meter("Progress", progress, total, Tone::Progress);
            }
            fields.until("Downgrade", controller.downgrade_time);
            match controller.safe_mode.and_then(|end| context.until(end)) {
                Some(left) => fields.text("Safe mode", format!("Active, ends in {}", ticks(left))),
                None if controller.user.is_some() => fields.text("Safe mode", "Inactive"),
                None => {}
            }
            fields.count("Safe modes available", controller.safe_mode_available);
            fields.until("Safe mode cooldown", controller.safe_mode_cooldown);
            if let Some(reservation) = &controller.reservation {
                let name = context.name(&reservation.user);
                let text = match context.until(reservation.end_time) {
                    Some(left) => format!("{name}, {} left", ticks(left)),
                    None => name,
                };
                fields.text("Reserved by", text);
            }
            fields.until("Upgrade blocked", controller.upgrade_blocked);
            if let Some(sign) = &controller.sign {
                fields.text(
                    "Sign",
                    format!("“{}” — {}", sign.text, context.name(&sign.user)),
                );
            }
        }
        GameRoomObject::Source(source) => {
            fields.meter(
                "Energy",
                source.energy,
                source.energy_capacity,
                Tone::Energy,
            );
            fields.until("Regenerates", source.next_regeneration_time);
        }
        GameRoomObject::Mineral(mineral) => {
            fields.text("Type", mineral.mineral_type.clone());
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "mineral amounts are small non-negative whole numbers"
            )]
            let amount = mineral.mineral_amount.max(0.0).round() as u64;
            fields.text("Amount", number(amount));
            fields.text("Density", mineral.density.to_string());
            fields.until("Regenerates", mineral.next_regeneration_time);
        }
        GameRoomObject::Deposit(deposit) => {
            fields.text("Type", deposit.deposit_type.clone());
            fields.text("Harvested", number(deposit.harvested));
            fields.until("Cooldown", deposit.cooldown_time);
            fields.until("Decays", Some(deposit.decay_time));
        }
        GameRoomObject::ConstructionSite(site) => {
            fields.text("Structure", kind_label(&site.structure_type));
            fields.meter(
                "Progress",
                site.progress,
                site.progress_total,
                Tone::Progress,
            );
        }
        GameRoomObject::ConstructedWall(wall) => {
            fields.hits(wall.hits, wall.hits_max);
            if wall.newbie_wall == Some(true) {
                fields.text("Kind", "Novice area border");
            }
        }
        GameRoomObject::Rampart(rampart) => {
            fields.hits(Some(rampart.hits), Some(rampart.hits_max));
            fields.text(
                "Public",
                if rampart.is_public == Some(true) {
                    "Yes"
                } else {
                    "No"
                },
            );
            fields.until("Decays", Some(rampart.next_decay_time));
        }
        GameRoomObject::Road(road) => {
            fields.hits(Some(road.hits), Some(road.hits_max));
            fields.until("Decays", Some(road.next_decay_time));
        }
        GameRoomObject::Link(link) => {
            fields.hits(Some(link.hits), Some(link.hits_max));
            fields.energy(&link.store, &link.store_capacity_resource);
            fields.cooldown(link.cooldown);
        }
        GameRoomObject::Storage(storage) => {
            fields.hits(Some(storage.hits), Some(storage.hits_max));
            fields.store(&storage.store, storage.store_capacity);
        }
        GameRoomObject::Tower(tower) => {
            fields.hits(Some(tower.hits), Some(tower.hits_max));
            fields.energy(&tower.store, &tower.store_capacity_resource);
        }
        GameRoomObject::Observer(observer) => {
            fields.hits(Some(observer.hits), Some(observer.hits_max));
            if let Some(room) = &observer.observe_room {
                fields.text("Observing", room.clone());
            }
        }
        GameRoomObject::PowerBank(bank) => {
            fields.hits(Some(bank.hits), Some(bank.hits_max));
            fields.store(&bank.store, None);
            fields.until("Decays", Some(bank.decay_time));
        }
        GameRoomObject::PowerSpawn(spawn) => {
            fields.hits(Some(spawn.hits), Some(spawn.hits_max));
            fields.store(&spawn.store, total_capacity(&spawn.store_capacity_resource));
        }
        GameRoomObject::Extractor(extractor) => {
            fields.hits(extractor.hits, extractor.hits_max);
            fields.cooldown(extractor.cooldown.unwrap_or_default());
        }
        GameRoomObject::Lab(lab) => {
            fields.hits(Some(lab.hits), Some(lab.hits_max));
            fields.store(
                &lab.store,
                lab.store_capacity
                    .or_else(|| total_capacity(&lab.store_capacity_resource)),
            );
            fields.cooldown(lab.cooldown);
        }
        GameRoomObject::Terminal(terminal) => {
            fields.hits(terminal.hits, terminal.hits_max);
            fields.store(&terminal.store, terminal.store_capacity);
            fields.until("Cooldown", terminal.cooldown_time);
        }
        GameRoomObject::Container(container) => {
            fields.hits(Some(container.hits), Some(container.hits_max));
            fields.store(&container.store, container.store_capacity);
            fields.until("Decays", Some(container.next_decay_time));
        }
        GameRoomObject::Nuker(nuker) => {
            fields.hits(Some(nuker.hits), Some(nuker.hits_max));
            fields.store(&nuker.store, total_capacity(&nuker.store_capacity_resource));
            fields.until("Cooldown", Some(nuker.cooldown_time));
        }
        GameRoomObject::Factory(factory) => {
            fields.hits(Some(factory.hits), Some(factory.hits_max));
            fields.count("Level", factory.level);
            fields.store(&factory.store, factory.store_capacity);
            fields.cooldown(factory.cooldown);
        }
        GameRoomObject::InvaderCore(core) => {
            fields.text("Level", core.level.to_string());
            fields.hits(Some(core.hits), Some(core.hits_max));
            fields.until("Deploys", core.deploy_time);
            fields.until("Collapses", core.decay_time);
            if let Some(spawning) = &core.spawning {
                fields.text("Spawning", spawning.name.clone());
            }
        }
        GameRoomObject::KeeperLair(lair) => {
            fields.until("Next keeper", lair.next_spawn_time);
        }
        GameRoomObject::Portal(portal) => {
            let destination = &portal.destination;
            let mut text = destination.room.clone();
            if let Some(shard) = &destination.shard {
                text = format!("{text} on {shard}");
            }
            match Portal::new(portal).and_then(|portal| portal.destination) {
                Some(to) => fields.fields.push(Field {
                    label: "Leads to",
                    value: Value::Destination { text, to },
                }),
                None => fields.text("Leads to", text),
            }
            fields.until("Decays", portal.decay_time);
        }
        GameRoomObject::Ruin(ruin) => {
            fields.text("Was", kind_label(&ruin.structure.r#type));
            fields.store(&ruin.store, None);
            fields.until("Decays", Some(ruin.decay_time));
        }
        GameRoomObject::Tombstone(tombstone) => {
            fields.text("Died", format!("tick {}", tombstone.death_time));
            fields.store(&tombstone.store, None);
            fields.until("Decays", Some(tombstone.decay_time));
        }
        GameRoomObject::Nuke(nuke) => {
            fields.text("Launched from", nuke.launch_room_name.clone());
            fields.until("Lands", Some(nuke.land_time));
        }
        GameRoomObject::Resource(_) => {
            if let Some((kind, amount)) = dropped(object) {
                fields.text("Resource", kind);
                fields.text("Amount", number(amount));
            }
        }
        GameRoomObject::Other(_) => {}
    }
    Inspected {
        title: title(object),
        kind: kind_label(kind_key(object)),
        owner: user.map(|user| context.name(user)),
        own: context.is_me(user),
        fields: fields.fields,
    }
}

/// What every resource swatch shares.
const SWATCH: &str = "flex-none size-[0.7rem] rounded-xs";

/// The swatch of a resource's colour: `energy`, `power`, the base mineral of a mineral or
/// compound (`U` for `UH2O`), else muted.
pub fn resource_class(resource: &str) -> &'static str {
    match resource {
        "energy" => concatcp!(SWATCH, " bg-res-energy"),
        "power" => concatcp!(SWATCH, " bg-res-power"),
        "ops" => concatcp!(SWATCH, " bg-res-ops"),
        _ => {
            let base = resource
                .chars()
                .find(|character| matches!(character, 'U' | 'L' | 'K' | 'Z' | 'G'))
                .or_else(|| {
                    resource
                        .chars()
                        .next()
                        .filter(|first| matches!(first, 'H' | 'O' | 'X'))
                });
            match base {
                Some('U') => concatcp!(SWATCH, " bg-res-u"),
                Some('L') => concatcp!(SWATCH, " bg-res-l"),
                Some('K') => concatcp!(SWATCH, " bg-res-k"),
                Some('Z') => concatcp!(SWATCH, " bg-res-z"),
                Some('G') => concatcp!(SWATCH, " bg-res-g"),
                Some('X') => concatcp!(SWATCH, " bg-res-x"),
                Some('H' | 'O') => concatcp!(SWATCH, " bg-res-ho"),
                _ => concatcp!(SWATCH, " bg-muted"),
            }
        }
    }
}

/// What the inspector shows; recomputed with each room update, and an update that changes
/// nothing shown changes nothing on screen.
#[derive(Clone, Debug, PartialEq)]
enum Shown {
    /// No object is selected.
    Nothing,
    /// The room is not live: its objects are unknown.
    NotLive,
    /// The selected object left the room.
    Gone,
    /// The selected object.
    Object(Box<Detail>),
}

/// Which of [`Shown`] it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Nothing,
    NotLive,
    Gone,
    Object,
}

impl Shown {
    const fn state(&self) -> State {
        match self {
            Self::Nothing => State::Nothing,
            Self::NotLive => State::NotLive,
            Self::Gone => State::Gone,
            Self::Object(_) => State::Object,
        }
    }

    fn detail(&self) -> Option<&Detail> {
        match self {
            Self::Object(detail) => Some(detail),
            _ => None,
        }
    }
}

/// The selected object as shown.
#[derive(Clone, Debug, PartialEq)]
struct Detail {
    inspected: Inspected,
    /// Its tile (`12,34`).
    position: String,
    /// The tile's other objects: `(id, title)`.
    neighbours: Vec<(String, String)>,
    /// Its document, pretty-printed.
    json: String,
}
/// What every body part shares: dimmed once dead (`data-dead`), ringed when boosted
/// (`data-boosted`).
const PART: &str = "size-[0.85rem] rounded-xs data-dead:opacity-25 \
    data-boosted:shadow-[0_0_0_2px_var(--color-sunken),0_0_0_3px_var(--color-fg)]";

/// The square of a body part of kind `kind` (`move`, `ranged_attack`, …) in its colour, muted
/// for an unknown kind.
fn part_class(kind: &str) -> &'static str {
    match kind {
        "move" => concatcp!(PART, " bg-part-move"),
        "work" => concatcp!(PART, " bg-part-work"),
        "carry" => concatcp!(PART, " bg-part-carry"),
        "attack" => concatcp!(PART, " bg-part-attack"),
        "ranged_attack" => concatcp!(PART, " bg-part-ranged-attack"),
        "heal" => concatcp!(PART, " bg-part-heal"),
        "claim" => concatcp!(PART, " bg-part-claim"),
        "tough" => concatcp!(PART, " bg-part-tough"),
        _ => concatcp!(PART, " bg-muted"),
    }
}

/// An empty inspector's message.
const EMPTY: &str = "flex flex-col items-center gap-2 p-4 text-sm text-muted text-center";

/// The selected object of the room, its tile's other objects, and its raw JSON.
///
/// While an object is selected the panel is built once and each part follows the room's
/// updates in place, so an opened "Raw JSON" stays open and focus stays where it is; a field
/// changes on screen only when its value does.
#[component]
pub fn Inspector(
    /// The room's live state.
    #[prop(into)]
    snapshot: Signal<Option<Arc<RoomSnapshot>>>,
    /// The selected object id.
    selected: RwSignal<Option<String>>,
    /// The signed-in user id.
    user: Option<String>,
    /// Selects another object (an object of the same tile).
    on_select: Callback<String>,
    /// Takes the user through a portal (its destination is a link; plain text without).
    #[prop(optional_no_strip)]
    go_to: Option<Callback<PortalDestination>>,
) -> impl IntoView {
    let user = StoredValue::new(user);
    let shown = Memo::new(move |_| {
        let Some(id) = selected.get() else {
            return Shown::Nothing;
        };
        snapshot.with(|snapshot| {
            let Some(snapshot) = snapshot else {
                return Shown::NotLive;
            };
            let Some(object) = snapshot.objects.iter().find(|object| object.id() == id) else {
                return Shown::Gone;
            };
            user.with_value(|me| {
                let context = Context {
                    game_time: snapshot.game_time,
                    me: me.as_deref(),
                    users: &snapshot.users,
                };
                let (x, y) = (object.x(), object.y());
                Shown::Object(Box::new(Detail {
                    inspected: inspect(object, &context),
                    position: format!("{x},{y}"),
                    neighbours: snapshot
                        .objects
                        .iter()
                        .filter(|other| other.x() == x && other.y() == y && other.id() != id)
                        .map(|other| (other.id().to_owned(), title(other)))
                        .collect(),
                    json: serde_json::to_string_pretty(object).unwrap_or_default(),
                }))
            })
        })
    });
    let state = Memo::new(move |_| shown.with(Shown::state));
    let content = move || match state.get() {
        State::Nothing => view! {
            <p class=EMPTY>"Select an object in the list or on the map."</p>
        }
        .into_any(),
        State::NotLive => view! {
            <p class=EMPTY>"The room is not live: its objects are unknown."</p>
        }
        .into_any(),
        State::Gone => view! { <p class=EMPTY role="status">"The object is gone."</p> }.into_any(),
        State::Object => detail_view(shown, on_select, go_to).into_any(),
    };
    // A message alone takes only its height; an object takes the room the list leaves.
    view! {
        <section
            class="flex-[1_1_55%] has-[>p]:flex-none min-h-0 overflow-auto p-3 bg-panel-2 border-t \
                border-edge"
            aria-label="Inspector"
        >
            {content}
        </section>
    }
}

/// The selected object's header, fields, tile mates and raw JSON, each part following
/// `shown` in place.
fn detail_view(
    shown: Memo<Shown>,
    on_select: Callback<String>,
    go_to: Option<Callback<PortalDestination>>,
) -> impl IntoView {
    let part = move |read: fn(&Detail) -> String| {
        Memo::new(move |_| shown.with(|shown| shown.detail().map(read).unwrap_or_default()))
    };
    let heading = part(|detail| detail.inspected.title.clone());
    let kind = part(|detail| detail.inspected.kind.clone());
    let position = part(|detail| detail.position.clone());
    let json = part(|detail| detail.json.clone());
    let owner = Memo::new(move |_| {
        shown.with(|shown| {
            let inspected = &shown.detail()?.inspected;
            Some((inspected.owner.clone()?, inspected.own))
        })
    });
    let neighbours = Memo::new(move |_| {
        shown.with(|shown| {
            shown
                .detail()
                .map(|detail| detail.neighbours.clone())
                .unwrap_or_default()
        })
    });
    let fields = Memo::new(move |_| {
        shown.with(|shown| {
            shown
                .detail()
                .map(|detail| detail.inspected.fields.clone())
                .unwrap_or_default()
        })
    });
    // Fields keyed by place and label: each keeps its elements while its value is unchanged.
    let field_keys = move || {
        fields.with(|fields| {
            fields
                .iter()
                .enumerate()
                .map(|(index, field)| (index, field.label))
                .collect::<Vec<_>>()
        })
    };
    let field = move |(index, label): (usize, &'static str)| {
        let value = Memo::new(move |_| {
            fields.with(|fields| {
                fields
                    .get(index)
                    .filter(|field| field.label == label)
                    .map(|field| field.value.clone())
            })
        });
        view! {
            <dt class="text-muted">{label}</dt>
            <dd class="min-w-0 tabular-nums wrap-anywhere">
                {move || value.get().map(|value| value_view(label, value, go_to))}
            </dd>
        }
    };
    // The signed-in user's own objects are marked by colour.
    let owner = move || {
        owner.get().map(|(owner, own)| {
            let class = if own { "text-ok" } else { "" };
            view! { <span class=class>{owner}</span> }
        })
    };
    let neighbours = move || {
        let neighbours = neighbours.get();
        (!neighbours.is_empty()).then(|| {
            let chips = neighbours
                .into_iter()
                .map(|(id, title)| {
                    view! {
                        <li>
                            <button type="button" class=CHIP on:click=move |_| on_select.run(id.clone())>
                                {title}
                            </button>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <div class="flex flex-wrap items-center gap-2 mt-3 text-sm">
                    <span class="text-muted">"Also here"</span>
                    <ul class="flex flex-wrap gap-1">{chips}</ul>
                </div>
            }
        })
    };
    view! {
        <header>
            <h3 class="text-lg font-semibold leading-tight wrap-anywhere">{move || heading.get()}</h3>
            <p class="flex flex-wrap gap-x-3 gap-y-1 mt-1 mb-3 text-sm text-muted">
                <span>{move || kind.get()}</span>
                {owner}
                <span class="font-mono text-xs tabular-nums text-faint" title="Position">
                    {move || position.get()}
                </span>
            </p>
        </header>
        <dl class="grid grid-cols-[max-content_minmax(0,1fr)] gap-x-4 gap-y-2 items-baseline text-sm">
            <For each=field_keys key=|key| *key children=field />
        </dl>
        {neighbours}
        <details class="mt-3 text-sm">
            <summary class="text-muted cursor-pointer hover:text-fg">"Raw JSON"</summary>
            <pre class="max-h-72 mt-2 p-2 overflow-auto font-mono text-xs bg-sunken rounded-md">{move || json.get()}</pre>
        </details>
    }
}

/// A tile mate's chip: selects it.
const CHIP: &str = "inline-flex items-center justify-center gap-2 min-h-control-sm px-3 text-sm \
    font-normal leading-tight whitespace-nowrap text-fg bg-panel-2 border border-edge rounded-full \
    cursor-pointer transition-colors duration-(--duration-fast) ease-standard \
    enabled:hover:bg-panel-3 enabled:hover:border-edge-strong";

/// A field's value; a portal's destination is a link through it when `go_to` takes the user
/// there.
fn value_view(
    label: &'static str,
    value: Value,
    go_to: Option<Callback<PortalDestination>>,
) -> AnyView {
    match value {
        Value::Text(text) => view! { <span>{text}</span> }.into_any(),
        Value::Destination { text, to } => match go_to {
            Some(go_to) => {
                let name = format!("Go to {text}");
                view! {
                    <button
                        type="button"
                        class=concatcp!(style::LINK_BUTTON, " font-medium")
                        aria-label=name.clone()
                        title=name
                        on:click=move |_| go_to.run(to.clone())
                    >
                        {text}
                    </button>
                }
                .into_any()
            }
            None => view! { <span>{text}</span> }.into_any(),
        },
        Value::Meter { value, max, tone } => {
            let text = format!("{} / {}", number(value), number(max));
            view! {
                <span class=style::METER>
                    <progress
                        class=tone.class()
                        max=max.to_string()
                        value=value.min(max).to_string()
                        aria-label=label
                    ></progress>
                    <span class=style::METER_TEXT>{text}</span>
                </span>
            }
            .into_any()
        }
        Value::Store {
            resources,
            used,
            capacity,
        } => {
            let total = capacity.map_or_else(
                || number(used),
                |capacity| format!("{} / {}", number(used), number(capacity)),
            );
            let items = resources
                .into_iter()
                .map(|(resource, amount)| {
                    let class = resource_class(&resource);
                    view! {
                        <li class="flex items-center gap-2">
                            <span class=class aria-hidden="true"></span>
                            <span>{resource}</span>
                            <span class="ml-auto">{number(amount)}</span>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <span class="flex flex-col gap-1">
                    <span class="text-muted">{total}</span>
                    <ul class="grid grid-cols-[repeat(auto-fill,minmax(7rem,1fr))] gap-x-3 gap-y-1">
                        {items}
                    </ul>
                </span>
            }
            .into_any()
        }
        Value::Body(parts) => {
            let count = parts.len();
            let items = parts
                .into_iter()
                .map(|part| {
                    let name = match &part.boost {
                        Some(boost) => format!("{} ({boost})", part.kind),
                        None => part.kind.clone(),
                    };
                    let class = part_class(&part.kind);
                    let title = name.clone();
                    view! {
                        <li
                            class=class
                            data-dead=!part.alive
                            data-boosted=part.boost.is_some()
                            title=title
                        >
                            <span class="sr-only">{name}</span>
                        </li>
                    }
                })
                .collect_view();
            view! {
                <span>
                    <ul class="flex flex-wrap gap-[3px]" aria-label=format!("{count} body parts")>
                        {items}
                    </ul>
                </span>
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use overseer::core::api::types::GameRoomObject;
    use overseer::core::socket::types::RoomUser;
    use serde_json::{Value as Json, json};

    use super::{
        Context, Field, PART, Part, Tone, Value, inspect, kind_label, part_class, resource_class,
        title,
    };

    fn object(value: Json) -> GameRoomObject {
        serde_json::from_value(value).unwrap()
    }

    fn users() -> BTreeMap<String, RoomUser> {
        let mut users = BTreeMap::new();
        users.insert(
            "u1".to_owned(),
            serde_json::from_value(json!({"_id": "u1", "username": "alice"})).unwrap(),
        );
        users.insert(
            "u2".to_owned(),
            serde_json::from_value(json!({"_id": "u2", "username": "bob"})).unwrap(),
        );
        users
    }

    fn field<'a>(fields: &'a [Field], label: &str) -> &'a Value {
        &fields
            .iter()
            .find(|field| field.label == label)
            .unwrap_or_else(|| panic!("no field {label} in {fields:?}"))
            .value
    }

    fn text(value: &str) -> Value {
        Value::Text(value.to_owned())
    }

    #[test]
    fn creep_fields() {
        let users = users();
        let context = Context {
            game_time: Some(1000),
            me: Some("u1"),
            users: &users,
        };
        let creep = object(json!({
            "_id": "c1", "type": "creep", "room": "W1N1", "x": 7, "y": 43, "name": "w16",
            "user": "u1", "hits": 250, "hitsMax": 300, "ageTime": 2400, "fatigue": 2,
            "store": {"energy": 30, "H": null}, "storeCapacity": 50,
            "body": [
                {"type": "work", "hits": 100, "boost": "UO"},
                {"type": "carry", "hits": 100},
                {"type": "move", "hits": 0}
            ],
            "actionLog": {"say": {"message": "hi", "isPublic": true}}
        }));
        let inspected = inspect(&creep, &context);
        assert_eq!(inspected.title, "w16");
        assert_eq!(inspected.kind, "Creep");
        assert_eq!(inspected.owner.as_deref(), Some("alice"));
        assert!(inspected.own);
        let fields = &inspected.fields;
        assert_eq!(
            field(fields, "Hits"),
            &Value::Meter {
                value: 250,
                max: 300,
                tone: Tone::Hits
            }
        );
        assert_eq!(field(fields, "Ticks to live"), &text("1\u{202f}400"));
        assert_eq!(field(fields, "Fatigue"), &text("2"));
        assert_eq!(
            field(fields, "Store"),
            &Value::Store {
                resources: vec![("energy".to_owned(), 30)],
                used: 30,
                capacity: Some(50)
            }
        );
        assert_eq!(
            field(fields, "Body"),
            &Value::Body(vec![
                Part {
                    kind: "work".to_owned(),
                    boost: Some("UO".to_owned()),
                    alive: true
                },
                Part {
                    kind: "carry".to_owned(),
                    boost: None,
                    alive: true
                },
                Part {
                    kind: "move".to_owned(),
                    boost: None,
                    alive: false
                },
            ])
        );
        assert_eq!(field(fields, "Saying"), &text("hi"));

        let spawning = object(json!({
            "_id": "c2", "type": "creep", "room": "W1N1", "x": 1, "y": 1, "name": "new",
            "user": "u2", "hits": 100, "hitsMax": 100, "spawning": true, "store": {}, "body": []
        }));
        let inspected = inspect(&spawning, &context);
        assert!(!inspected.own);
        assert_eq!(inspected.owner.as_deref(), Some("bob"));
        assert_eq!(field(&inspected.fields, "Ticks to live"), &text("Spawning"));
    }

    #[test]
    fn spawn_and_extension_fields() {
        let users = users();
        let context = Context {
            game_time: Some(100),
            me: Some("u1"),
            users: &users,
        };
        let spawn = object(json!({
            "_id": "s1", "type": "spawn", "room": "W1N1", "x": 18, "y": 36, "name": "Spawn1",
            "user": "u1", "hits": 5000, "hitsMax": 5000, "notifyWhenAttacked": true,
            "store": {"energy": 120}, "storeCapacityResource": {"energy": 300},
            "spawning": {"name": "w200", "needTime": 9, "spawnTime": 103}
        }));
        let inspected = inspect(&spawn, &context);
        assert_eq!(inspected.title, "Spawn1");
        assert_eq!(
            field(&inspected.fields, "Energy"),
            &Value::Meter {
                value: 120,
                max: 300,
                tone: Tone::Energy
            }
        );
        assert_eq!(field(&inspected.fields, "Spawning"), &text("w200"));
        assert_eq!(
            field(&inspected.fields, "Progress"),
            &Value::Meter {
                value: 6,
                max: 9,
                tone: Tone::Progress
            }
        );
        let extension = object(json!({
            "_id": "e1", "type": "extension", "room": "W1N1", "x": 1, "y": 2, "user": "u1",
            "hits": 1000, "hitsMax": 1000, "notifyWhenAttacked": true,
            "store": {"energy": 50}, "storeCapacityResource": {"energy": 50}, "off": true
        }));
        let inspected = inspect(&extension, &context);
        assert_eq!(inspected.title, "Extension");
        assert_eq!(field(&inspected.fields, "State"), &text("Inactive"));
    }

    #[test]
    fn controller_fields() {
        let users = users();
        let context = Context {
            game_time: Some(5000),
            me: Some("u1"),
            users: &users,
        };
        let controller = object(json!({
            "_id": "k", "type": "controller", "room": "W1N1", "x": 21, "y": 39, "user": "u1",
            "level": 3, "progress": 1500, "progressTotal": 45000, "downgradeTime": 25000,
            "safeMode": 5100, "safeModeAvailable": 2,
            "sign": {"user": "u2", "text": "hello", "time": 1, "datetime": 1}
        }));
        let inspected = inspect(&controller, &context);
        assert_eq!(inspected.title, "Controller · level 3");
        let fields = &inspected.fields;
        assert_eq!(field(fields, "Level"), &text("3"));
        assert_eq!(
            field(fields, "Progress"),
            &Value::Meter {
                value: 1500,
                max: 45000,
                tone: Tone::Progress
            }
        );
        assert_eq!(field(fields, "Downgrade"), &text("in 20\u{202f}000 ticks"));
        assert_eq!(
            field(fields, "Safe mode"),
            &text("Active, ends in 100 ticks")
        );
        assert_eq!(field(fields, "Safe modes available"), &text("2"));
        assert_eq!(field(fields, "Sign"), &text("“hello” — bob"));

        let reserved = object(json!({
            "_id": "k2", "type": "controller", "room": "W2N1", "x": 1, "y": 1, "level": 0,
            "reservation": {"user": "u2", "endTime": 5001}
        }));
        let fields = inspect(&reserved, &context).fields;
        assert_eq!(field(&fields, "Reserved by"), &text("bob, 1 tick left"));
        assert!(
            fields.iter().all(|field| field.label != "Safe mode"),
            "unowned: no safe mode"
        );
    }

    #[test]
    fn sources_minerals_and_sites() {
        let users = users();
        let context = Context {
            game_time: Some(10),
            me: None,
            users: &users,
        };
        let source = object(json!({
            "_id": "src", "type": "source", "room": "W1N1", "x": 11, "y": 44, "energy": 1200,
            "energyCapacity": 3000, "ticksToRegeneration": 300, "nextRegenerationTime": 40
        }));
        let inspected = inspect(&source, &context);
        assert_eq!(inspected.owner, None);
        assert_eq!(
            field(&inspected.fields, "Energy"),
            &Value::Meter {
                value: 1200,
                max: 3000,
                tone: Tone::Energy
            }
        );
        assert_eq!(
            field(&inspected.fields, "Regenerates"),
            &text("in 30 ticks")
        );
        let mineral = object(json!({
            "_id": "m", "type": "mineral", "room": "W1N1", "x": 6, "y": 45, "density": 3,
            "mineralAmount": 35000.0, "mineralType": "O"
        }));
        let inspected = inspect(&mineral, &context);
        assert_eq!(inspected.title, "Mineral O");
        assert_eq!(field(&inspected.fields, "Amount"), &text("35\u{202f}000"));
        assert_eq!(field(&inspected.fields, "Density"), &text("3"));
        let site = object(json!({
            "_id": "cs", "type": "constructionSite", "room": "W1N1", "x": 2, "y": 2,
            "user": "u1", "structureType": "extension", "progress": 600, "progressTotal": 3000
        }));
        let inspected = inspect(&site, &context);
        assert_eq!(inspected.title, "Extension (site)");
        assert_eq!(inspected.kind, "Construction site");
        assert!(!inspected.own, "no signed-in user");
        assert_eq!(
            field(&inspected.fields, "Progress"),
            &Value::Meter {
                value: 600,
                max: 3000,
                tone: Tone::Progress
            }
        );
    }

    #[test]
    fn stores_dropped_resources_and_tombstones() {
        let users = users();
        let context = Context {
            game_time: Some(100),
            me: Some("u1"),
            users: &users,
        };
        let storage = object(json!({
            "_id": "st", "type": "storage", "room": "W1N1", "x": 3, "y": 3, "user": "u1",
            "hits": 10000, "hitsMax": 10000, "notifyWhenAttacked": true,
            "store": {"energy": 1000, "UH2O": 3000, "K": 0}, "storeCapacity": 1_000_000
        }));
        assert_eq!(
            field(&inspect(&storage, &context).fields, "Store"),
            &Value::Store {
                resources: vec![("UH2O".to_owned(), 3000), ("energy".to_owned(), 1000)],
                used: 4000,
                capacity: Some(1_000_000)
            }
        );
        let dropped = object(json!({
            "_id": "r", "type": "energy", "resourceType": "energy", "room": "W1N1", "x": 4,
            "y": 4, "energy": 75
        }));
        let inspected = inspect(&dropped, &context);
        assert_eq!(inspected.title, "75 energy");
        assert_eq!(inspected.kind, "Dropped resource");
        assert_eq!(field(&inspected.fields, "Amount"), &text("75"));
        let tombstone = object(json!({
            "_id": "t", "type": "tombstone", "room": "W1N1", "x": 5, "y": 5, "user": "u2",
            "creepName": "old", "deathTime": 90, "decayTime": 150, "store": {"energy": 10}
        }));
        let inspected = inspect(&tombstone, &context);
        assert_eq!(inspected.title, "Tombstone of old");
        assert_eq!(field(&inspected.fields, "Decays"), &text("in 50 ticks"));
        assert_eq!(field(&inspected.fields, "Died"), &text("tick 90"));
    }

    #[test]
    fn structures_with_cooldowns_and_decay() {
        let users = users();
        let context = Context {
            game_time: Some(100),
            me: Some("u1"),
            users: &users,
        };
        let link = object(json!({
            "_id": "l", "type": "link", "room": "W1N1", "x": 1, "y": 1, "user": "u1", "hits": 1000,
            "hitsMax": 1000, "notifyWhenAttacked": true, "cooldown": 3, "store": {"energy": 400},
            "storeCapacityResource": {"energy": 800}
        }));
        let fields = inspect(&link, &context).fields;
        assert_eq!(field(&fields, "Cooldown"), &text("3 ticks"));
        assert_eq!(
            field(&fields, "Energy"),
            &Value::Meter {
                value: 400,
                max: 800,
                tone: Tone::Energy
            }
        );
        let road = object(json!({
            "_id": "rd", "type": "road", "room": "W1N1", "x": 1, "y": 2, "hits": 2500,
            "hitsMax": 5000, "nextDecayTime": 1100, "notifyWhenAttacked": true
        }));
        let fields = inspect(&road, &context).fields;
        assert_eq!(field(&fields, "Decays"), &text("in 1\u{202f}000 ticks"));
        let rampart = object(json!({
            "_id": "rp", "type": "rampart", "room": "W1N1", "x": 1, "y": 3, "user": "u1",
            "hits": 1, "hitsMax": 300_000, "nextDecayTime": 99, "isPublic": true
        }));
        let fields = inspect(&rampart, &context).fields;
        assert_eq!(field(&fields, "Public"), &text("Yes"));
        assert!(
            fields.iter().all(|field| field.label != "Decays"),
            "past ticks are not shown"
        );
        let other =
            object(json!({"_id": "o", "type": "scoreContainer", "room": "W1N1", "x": 1, "y": 4}));
        let inspected = inspect(&other, &context);
        assert_eq!(inspected.kind, "Score container");
        assert_eq!(inspected.fields, []);
    }

    #[test]
    fn portals_lead_to_their_destination_on_this_or_another_shard() {
        let users = users();
        let context = Context {
            game_time: Some(1000),
            me: None,
            users: &users,
        };
        let within = object(json!({
            "_id": "p1", "type": "portal", "room": "W5N5", "x": 20, "y": 21,
            "destination": {"room": "W0N0", "x": 25, "y": 26}, "decayTime": 1500
        }));
        let fields = inspect(&within, &context).fields;
        let Value::Destination { text, to } = field(&fields, "Leads to") else {
            panic!("no destination link in {fields:?}");
        };
        assert_eq!(text, "W0N0");
        assert_eq!(to.room.to_string(), "W0N0");
        assert_eq!(
            (to.shard.as_deref(), to.x, to.y),
            (None, Some(25), Some(26))
        );
        assert_eq!(
            field(&fields, "Decays"),
            &Value::Text("in 500 ticks".to_owned())
        );

        let across = object(json!({
            "_id": "p2", "type": "portal", "room": "W0N0", "x": 10, "y": 10,
            "destination": {"room": "E10S20", "shard": "shard2"}
        }));
        let fields = inspect(&across, &context).fields;
        let Value::Destination { text, to } = field(&fields, "Leads to") else {
            panic!("no destination link in {fields:?}");
        };
        assert_eq!(text, "E10S20 on shard2");
        assert_eq!((to.shard.as_deref(), to.x), (Some("shard2"), None));

        // A destination that does not parse stays plain text.
        let broken = object(json!({
            "_id": "p3", "type": "portal", "room": "W0N0", "x": 10, "y": 10,
            "destination": {"room": "nowhere"}
        }));
        let fields = inspect(&broken, &context).fields;
        assert_eq!(
            field(&fields, "Leads to"),
            &Value::Text("nowhere".to_owned())
        );
    }

    #[test]
    fn labels_and_titles() {
        assert_eq!(kind_label("constructionSite"), "Construction site");
        assert_eq!(kind_label("powerSpawn"), "Power spawn");
        assert_eq!(kind_label("constructedWall"), "Wall");
        assert_eq!(kind_label("keeperLair"), "Keeper lair");
        let wall =
            object(json!({"_id": "w", "type": "constructedWall", "room": "W1N1", "x": 1, "y": 1}));
        assert_eq!(title(&wall), "Wall");
    }

    #[test]
    fn resource_colours() {
        let swatch = |colour: &str| format!("flex-none size-[0.7rem] rounded-xs {colour}");
        assert_eq!(resource_class("energy"), swatch("bg-res-energy"));
        assert_eq!(resource_class("power"), swatch("bg-res-power"));
        assert_eq!(resource_class("ops"), swatch("bg-res-ops"));
        assert_eq!(resource_class("UH2O"), swatch("bg-res-u"));
        assert_eq!(resource_class("LO"), swatch("bg-res-l"));
        assert_eq!(resource_class("XKHO2"), swatch("bg-res-k"));
        assert_eq!(resource_class("ZK"), swatch("bg-res-z"));
        assert_eq!(resource_class("OH"), swatch("bg-res-ho"));
        assert_eq!(resource_class("X"), swatch("bg-res-x"));
        assert_eq!(resource_class("G"), swatch("bg-res-g"));
        assert_eq!(resource_class("metal"), swatch("bg-muted"));
    }

    #[test]
    fn part_colours() {
        assert_eq!(
            PART,
            "size-[0.85rem] rounded-xs data-dead:opacity-25 \
             data-boosted:shadow-[0_0_0_2px_var(--color-sunken),0_0_0_3px_var(--color-fg)]"
        );
        let part = |colour: &str| format!("{PART} {colour}");
        assert_eq!(part_class("move"), part("bg-part-move"));
        assert_eq!(part_class("work"), part("bg-part-work"));
        assert_eq!(part_class("carry"), part("bg-part-carry"));
        assert_eq!(part_class("attack"), part("bg-part-attack"));
        assert_eq!(part_class("ranged_attack"), part("bg-part-ranged-attack"));
        assert_eq!(part_class("heal"), part("bg-part-heal"));
        assert_eq!(part_class("claim"), part("bg-part-claim"));
        assert_eq!(part_class("tough"), part("bg-part-tough"));
        assert_eq!(part_class("wing"), part("bg-muted"));
    }
}
