//! The typed fields of each kind of room object: [`inspect`] builds them.

use std::collections::BTreeMap;

use overseer::map::Portal;
use overseer::model::world::{GameRoomObject, GameRoomObjectStore};

use super::{
    Context, Field, Inspected, Part, Tone, Value, dropped, kind_key, kind_label, number, ticks,
    title,
};

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
        owner: user.map(|user| context.player(user)),
        own: context.is_me(user),
        fields: fields.fields,
    }
}
