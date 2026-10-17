//! The names of the signed-in user's game that the console completes ([`Names`]): own creeps,
//! spawns and rooms, and `Memory`.
//!
//! Nothing loads until a completion needs it ([`GameNames::ensure`]). A part loads again when
//! it is older than [`FRESH`] ([`FRESH_EMPTY`] when it gave no names) or was loaded for
//! another shard. Own rooms come from the user's room list (`Users::rooms`); own creeps and
//! spawns from the objects of the first [`MAX_ROOMS`] of those rooms (`Rooms::objects`), so
//! creeps outside these rooms are not in the list. `Memory` is read whole (`Memory::get` of
//! the root).
//!
//! # Pace
//!
//! On screeps.com every request of the token counts against its limit of 120 per minute, and
//! `Memory` reads have a daily limit. Each request of a load first waits for the session's
//! request budget (`Client::budget_ready_at`, priority background), shared with the world map
//! and the badges. Servers without rate limits have no budget: requests go at once.

use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Overseer;
use overseer::memory::MemoryValue;
use overseer::model::world::GameRoomObject;
use overseer::net::Priority;
use overseer::net::rt::{self, Instant};

use super::complete::{Names, Needs};

/// How long loaded names stay in use before a completion loads them again.
const FRESH: Duration = Duration::from_mins(5);
/// How long an empty result (or a failure) stays in use: a new player's first creep, or a
/// server that was busy, shows up sooner.
const FRESH_EMPTY: Duration = Duration::from_secs(30);
/// Most rooms whose objects one load reads.
const MAX_ROOMS: usize = 16;
/// Most room object requests at a time.
const PARALLEL: usize = 8;

/// The state of one part of the names.
#[derive(Default)]
struct Load {
    /// The shard it was loaded for.
    shard: Option<String>,
    /// When it was loaded, also after a failure.
    at: Option<Instant>,
    /// The load gave no names.
    empty: bool,
    /// A load runs.
    busy: bool,
}

impl Load {
    /// Whether the part must load (again) for `shard`.
    fn stale(&self, shard: Option<&str>) -> bool {
        let fresh = if self.empty { FRESH_EMPTY } else { FRESH };
        !self.busy
            && (self.shard.as_deref() != shard || self.at.is_none_or(|at| at.elapsed() >= fresh))
    }

    /// Marks a load for `shard` as started.
    fn start(&mut self, shard: Option<&str>) {
        self.shard = shard.map(str::to_owned);
        self.busy = true;
    }

    /// Marks the load as done; `empty` when it gave no names.
    fn finish(&mut self, empty: bool) {
        self.at = Some(Instant::now());
        self.empty = empty;
        self.busy = false;
    }
}

/// The names of the signed-in user's game, loaded on demand; `Copy`.
#[derive(Clone, Copy)]
pub struct GameNames {
    /// The session.
    overseer: StoredValue<Overseer, LocalStorage>,
    /// The names loaded so far.
    pub names: RwSignal<Names>,
    /// Own creeps, spawns and rooms.
    objects: StoredValue<Load, LocalStorage>,
    /// `Memory`.
    memory: StoredValue<Load, LocalStorage>,
}

impl GameNames {
    /// No names yet, loaded from `overseer` on demand.
    pub fn new(overseer: &Overseer) -> Self {
        Self {
            overseer: StoredValue::new_local(overseer.clone()),
            names: RwSignal::new(Names::default()),
            objects: StoredValue::new_local(Load::default()),
            memory: StoredValue::new_local(Load::default()),
        }
    }

    /// Loads what `needs` asks for on `shard` unless it is fresh. Names of another shard are
    /// dropped at once.
    pub fn ensure(self, needs: Needs, shard: Option<&str>) {
        let Some(overseer) = self.overseer.try_get_value() else {
            return;
        };
        if needs.objects && self.objects.with_value(|load| load.stale(shard)) {
            self.load_objects(overseer.clone(), shard);
        }
        if needs.memory && self.memory.with_value(|load| load.stale(shard)) {
            self.load_memory(overseer, shard);
        }
    }

    /// Loads own creeps, spawns and rooms on `shard`.
    fn load_objects(self, overseer: Overseer, shard: Option<&str>) {
        if self
            .objects
            .with_value(|load| load.shard.as_deref() != shard)
        {
            self.names.update(|names| {
                names.creeps.clear();
                names.spawns.clear();
                names.rooms.clear();
            });
        }
        self.objects.update_value(|load| load.start(shard));
        let shard = shard.map(str::to_owned);
        spawn_local(async move {
            let loaded = own_objects(&overseer, shard.as_deref()).await;
            let empty = loaded
                .as_ref()
                .is_none_or(|(creeps, spawns, _)| creeps.is_empty() && spawns.is_empty());
            if self
                .objects
                .try_update_value(|load| load.finish(empty))
                .is_none()
            {
                return;
            }
            if let Some((creeps, spawns, rooms)) = loaded {
                self.names.try_update(|names| {
                    names.creeps = creeps;
                    names.spawns = spawns;
                    names.rooms = rooms;
                });
            }
        });
    }

    /// Loads `Memory` on `shard`.
    fn load_memory(self, overseer: Overseer, shard: Option<&str>) {
        if self
            .memory
            .with_value(|load| load.shard.as_deref() != shard)
        {
            self.names.update(|names| names.memory = None);
        }
        self.memory.update_value(|load| load.start(shard));
        let shard = shard.map(str::to_owned);
        spawn_local(async move {
            wait_for_budget(&overseer, 0).await;
            let memory = overseer.memory().get("", shard.as_deref()).await;
            let memory = match memory {
                Ok(MemoryValue::Json(value)) => value,
                // No memory yet, or it cannot be read: no keys.
                _ => serde_json::Value::Null,
            };
            let empty = memory.as_object().is_none_or(serde_json::Map::is_empty);
            if self
                .memory
                .try_update_value(|load| load.finish(empty))
                .is_none()
            {
                return;
            }
            self.names
                .try_update(|names| names.memory = Some(Arc::new(memory)));
        });
    }
}

/// Waits until the session's request budget lets `pending + 1` more background requests go.
async fn wait_for_budget(overseer: &Overseer, pending: usize) {
    while let Some(at) = overseer
        .client()
        .budget_ready_at(Priority::Background, pending)
    {
        rt::sleep_until(at).await;
    }
}

/// The names of the signed-in user's creeps, spawns and rooms on `shard`, sorted; `None` when
/// the room list cannot be read. Creeps and spawns come from the first [`MAX_ROOMS`] rooms in
/// the order of the room list.
async fn own_objects(
    overseer: &Overseer,
    shard: Option<&str>,
) -> Option<(Vec<String>, Vec<String>, Vec<String>)> {
    let user = overseer.user_id()?.to_owned();
    wait_for_budget(overseer, 0).await;
    let mut rooms: Vec<String> = overseer
        .users()
        .rooms(&user)
        .await
        .ok()?
        .into_iter()
        .filter(|room| room.shard.is_none() || room.shard.as_deref() == shard)
        .map(|room| room.room)
        .collect();
    let facade = overseer.rooms();
    let mut creeps = Vec::new();
    let mut spawns = Vec::new();
    let read = &rooms[..rooms.len().min(MAX_ROOMS)];
    for chunk in read.chunks(PARALLEL) {
        // Room for the whole chunk: the last request of it may go.
        wait_for_budget(overseer, chunk.len() - 1).await;
        let replies = join_all(chunk.iter().map(|room| facade.objects(room, shard))).await;
        // A room that cannot be read adds no names.
        for objects in replies.into_iter().flatten() {
            for object in &objects.objects {
                if object.user() != Some(user.as_str()) {
                    continue;
                }
                match object {
                    GameRoomObject::Creep(creep) => creeps.push(creep.name.clone()),
                    GameRoomObject::Spawn(spawn) => spawns.extend(spawn.name.clone()),
                    _ => {}
                }
            }
        }
    }
    rooms.sort();
    creeps.sort();
    spawns.sort();
    Some((creeps, spawns, rooms))
}
