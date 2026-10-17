//! The names of the signed-in user's game that the console completes ([`Names`]): own creeps,
//! spawns and rooms, and `Memory`.
//!
//! Nothing loads until a completion needs it ([`GameNames::ensure`]). A part loads again when
//! it is older than [`FRESH`] ([`FRESH_EMPTY`] when it gave no names) or was loaded for
//! another shard. Own rooms come from the user's room list (`Users::rooms`); own creeps and
//! spawns from the objects of the first [`MAX_ROOMS`] of those rooms (`Rooms::objects`), so
//! creeps outside these rooms are not in the list. `Memory` is all of `Memory` of the
//! [`Account`]: the Memory tab reads and changes the same value.
//!
//! # Pace
//!
//! On screeps.com every request of the token counts against its limit of 120 per minute, and
//! `Memory` reads have a daily limit. Each request of a load first waits for the session's
//! request budget (`Client::budget_ready_at`, priority background), shared with the world map
//! and the badges. Servers without rate limits have no budget: requests go at once.

use std::time::Duration;

use futures_util::future::join_all;
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Overseer;
use overseer::memory::MemoryValue;
use overseer::model::world::GameRoomObject;
use overseer::net::Priority;
use serde_json::Value;

use super::complete::{Names, Needs};
use crate::session::wait_for_budget;
use crate::ui::account::{Account, Load};

/// How long loaded names stay in use before a completion loads them again.
const FRESH: Duration = Duration::from_mins(5);
/// How long an empty result (or a failure) stays in use: a new player's first creep, or a
/// server that was busy, shows up sooner.
const FRESH_EMPTY: Duration = Duration::from_secs(30);
/// Most rooms whose objects one load reads.
const MAX_ROOMS: usize = 16;
/// Most room object requests at a time.
const PARALLEL: usize = 8;

/// The names of the signed-in user's game, loaded on demand; `Copy`.
#[derive(Clone, Copy)]
pub struct GameNames {
    /// The session, its shard and its `Memory`.
    account: Account,
    /// Own creeps, spawns and rooms loaded so far.
    names: RwSignal<Names>,
    /// The loads of [`Self::names`].
    objects: StoredValue<Load, LocalStorage>,
}

impl GameNames {
    /// No names yet, loaded from `account` on demand.
    pub fn new(account: Account) -> Self {
        Self {
            account,
            names: RwSignal::new(Names::default()),
            objects: StoredValue::new_local(Load::default()),
        }
    }

    /// The account that the names are of.
    pub const fn account(self) -> Account {
        self.account
    }

    /// Calls `f` with the names and all of `Memory` (`None` before it was read) on the
    /// account's shard. Tracks them.
    pub fn with<T>(self, f: impl FnOnce(&Names, Option<&Value>) -> T) -> T {
        self.names.with(|names| {
            self.account.with_root(|root| {
                let memory = root.and_then(|root| match &root.value {
                    MemoryValue::Json(value) => Some(value),
                    MemoryValue::Undefined | MemoryValue::IncorrectPath => None,
                });
                f(names, memory)
            })
        })
    }

    /// Loads what `needs` asks for on the account's shard unless it is fresh. Names of another
    /// shard are dropped at once.
    pub fn ensure(self, needs: Needs) {
        let shard = self.account.shard.get_untracked();
        if needs.objects
            && self
                .objects
                .with_value(|load| load.stale(shard.as_deref(), FRESH, FRESH_EMPTY))
        {
            self.load_objects(shard);
        }
        if needs.memory && self.account.root_stale(FRESH, FRESH_EMPTY) {
            // A failure leaves the keys out; the next completion after [`FRESH_EMPTY`] asks
            // again.
            let read = self.account.read_root(true);
            spawn_local(async move {
                let _ = read.await;
            });
        }
    }

    /// Loads own creeps, spawns and rooms on `shard`.
    fn load_objects(self, shard: Option<String>) {
        if !self
            .objects
            .with_value(|load| load.is_for(shard.as_deref()))
        {
            self.names.set(Names::default());
        }
        self.objects
            .update_value(|load| load.start(shard.as_deref()));
        let overseer = self.account.overseer();
        spawn_local(async move {
            let loaded = own_objects(&overseer, shard.as_deref()).await;
            let empty = loaded
                .as_ref()
                .is_none_or(|names| names.creeps.is_empty() && names.spawns.is_empty());
            if self
                .objects
                .try_update_value(|load| load.finish(empty))
                .is_none()
            {
                return;
            }
            if let Some(names) = loaded {
                self.names.try_set(names);
            }
        });
    }
}

/// The names of the signed-in user's creeps, spawns and rooms on `shard`, sorted; `None` when
/// the room list cannot be read. Creeps and spawns come from the first [`MAX_ROOMS`] rooms in
/// the order of the room list.
async fn own_objects(overseer: &Overseer, shard: Option<&str>) -> Option<Names> {
    let user = overseer.user_id()?.to_owned();
    wait_for_budget(overseer, Priority::Background, 0).await;
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
        wait_for_budget(overseer, Priority::Background, chunk.len() - 1).await;
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
    Some(Names {
        creeps,
        spawns,
        rooms,
    })
}
