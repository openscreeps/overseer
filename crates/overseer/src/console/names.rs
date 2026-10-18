//! The names of the signed-in user's game that a console completes ([`GameNames`]): own rooms,
//! and own creeps and spawns, per shard.
//!
//! [`Console::names`] loads them on demand and keeps them for the session, shared by every
//! console of the session (the drawer, pop-out windows). Names stay in use for
//! [`COMPLETION_FRESHNESS`]: 5 minutes, 30 seconds after a load that found no creeps and no
//! spawns or could not read a room (a first creep, or a busy server, then shows up sooner).
//!
//! Own rooms come from the user's room list (`GET /api/user/rooms`), filtered to the shard.
//! Own creeps and spawns come from the objects of the first [`NAMES_MAX_ROOMS`] of those rooms
//! (`GET /api/game/room-objects`, [`NAMES_PARALLEL`] at a time): creeps in other rooms are not
//! in the list, and [`GameNames::is_complete`] tells this.
//!
//! # Pace
//!
//! On screeps.com every request of the token counts against its limit of 120 per minute. Each
//! request of a load first waits for the session's request budget
//! ([`Client::wait_for_budget`](overseer_net::Client::wait_for_budget), priority
//! [`Priority::Background`]), shared with the world map and the badges. Servers without rate
//! limits have no budget: requests go at once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::future::join_all;
use overseer_net::Priority;
use overseer_net::model::world::GameRoomObject;

use super::Console;
use crate::Error;
use crate::cache::{Freshness, Load};

/// How long the names of [`Console::names`] stay in use. Use the same freshness for other
/// data a completion reads (such as all of `Memory`).
pub const COMPLETION_FRESHNESS: Freshness =
    Freshness::new(Duration::from_mins(5), Duration::from_secs(30));

/// Most own rooms whose objects one load of [`Console::names`] reads.
pub const NAMES_MAX_ROOMS: usize = 16;

/// Most room object requests of one load at a time.
pub const NAMES_PARALLEL: usize = 8;

/// The names of the signed-in user's own rooms, creeps and spawns on one shard, each list
/// sorted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GameNames {
    /// The keys of `Game.creeps` found in the rooms read.
    pub creeps: Vec<String>,
    /// The keys of `Game.spawns` found in the rooms read.
    pub spawns: Vec<String>,
    /// The keys of `Game.rooms` that the user owns: all own rooms of the shard.
    pub rooms: Vec<String>,
    /// How many of [`Self::rooms`] gave their objects: creeps and spawns come from these
    /// rooms only.
    pub rooms_read: usize,
}

impl GameNames {
    /// Whether [`Self::creeps`] and [`Self::spawns`] cover all own rooms: every room gave its
    /// objects. `false` when the user owns more than [`NAMES_MAX_ROOMS`] rooms or a room could
    /// not be read.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.rooms_read == self.rooms.len()
    }

    /// Whether the load must be tried again soon: no creeps and no spawns, or a room of the
    /// first [`NAMES_MAX_ROOMS`] could not be read.
    fn retry_soon(&self) -> bool {
        (self.creeps.is_empty() && self.spawns.is_empty())
            || self.rooms_read < self.rooms.len().min(NAMES_MAX_ROOMS)
    }
}

/// The names of one shard: the last loaded names and when they were loaded.
#[derive(Debug, Default)]
struct Entry {
    load: Load,
    names: Option<Arc<GameNames>>,
}

/// The names of one session, by resolved shard. The async lock of each shard makes concurrent
/// loads share one.
#[derive(Debug, Default)]
pub(crate) struct NamesCache {
    shards: Mutex<HashMap<Option<String>, Arc<tokio::sync::Mutex<Entry>>>>,
}

impl NamesCache {
    /// The entry of `shard`, created when missing.
    fn entry(&self, shard: Option<&str>) -> Arc<tokio::sync::Mutex<Entry>> {
        let mut shards = self.lock();
        Arc::clone(shards.entry(shard.map(str::to_owned)).or_default())
    }

    /// Locks the map; its critical sections cannot panic halfway through an update.
    fn lock(&self) -> MutexGuard<'_, HashMap<Option<String>, Arc<tokio::sync::Mutex<Entry>>>> {
        self.shards.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Console {
    /// The names of the signed-in user's own rooms, creeps and spawns on `shard` (`None`: the
    /// default shard; ignored by single-world servers), for completion.
    ///
    /// Returns the session's names while they are fresh ([`COMPLETION_FRESHNESS`]); otherwise
    /// loads them first (see the [module documentation](self)). A concurrent call waits for
    /// the running load and shares its result. When a load fails, this returns the names of
    /// the last successful load of the shard, and the next load starts only after
    /// [`Freshness::fresh_empty`]. `Ok(None)`: no load of the shard succeeded yet.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user and [`Error::Closed`] after
    /// [`Overseer::shutdown`](crate::Overseer::shutdown); a failed request is not an error.
    pub async fn names(&self, shard: Option<&str>) -> Result<Option<Arc<GameNames>>, Error> {
        self.overseer.ensure_open()?;
        let user = self.overseer.require_user()?.to_owned();
        let shard = self.overseer.resolve_shard(shard);
        let entry = self.overseer.names_cache().entry(shard.as_deref());
        let mut entry = entry.lock().await;
        if entry.load.stale(&COMPLETION_FRESHNESS) {
            let loaded = self.load_names(&user, shard.as_deref()).await;
            entry
                .load
                .finish(loaded.as_ref().is_none_or(GameNames::retry_soon));
            if let Some(names) = loaded {
                entry.names = Some(Arc::new(names));
            }
        }
        Ok(entry.names.clone())
    }

    /// The names of the last successful [`Self::names`] load on `shard`, fresh or not; sends
    /// nothing.
    #[must_use]
    pub fn cached_names(&self, shard: Option<&str>) -> Option<Arc<GameNames>> {
        let shard = self.overseer.resolve_shard(shard);
        let entry = self.overseer.names_cache().entry(shard.as_deref());
        // A running load holds the lock: it has no result yet.
        entry.try_lock().ok()?.names.clone()
    }

    /// Loads the names of `user` on the resolved `shard`; `None` when the room list cannot be
    /// read.
    async fn load_names(&self, user: &str, shard: Option<&str>) -> Option<GameNames> {
        let client = self.overseer.client();
        client.wait_for_budget(Priority::Background, 0).await;
        let mut rooms: Vec<String> = self
            .overseer
            .users()
            .rooms(user)
            .await
            .ok()?
            .into_iter()
            .filter(|room| room.shard.is_none() || room.shard.as_deref() == shard)
            .map(|room| room.room)
            .collect();
        let facade = self.overseer.rooms();
        let mut names = GameNames::default();
        for chunk in rooms[..rooms.len().min(NAMES_MAX_ROOMS)].chunks(NAMES_PARALLEL) {
            // Room for the whole chunk: its last request may go.
            client
                .wait_for_budget(Priority::Background, chunk.len() - 1)
                .await;
            let replies = join_all(chunk.iter().map(|room| facade.objects(room, shard))).await;
            // A room that cannot be read adds no names.
            for objects in replies.into_iter().flatten() {
                names.rooms_read += 1;
                for object in &objects.objects {
                    if object.user() != Some(user) {
                        continue;
                    }
                    match object {
                        GameRoomObject::Creep(creep) => names.creeps.push(creep.name.clone()),
                        GameRoomObject::Spawn(spawn) => names.spawns.extend(spawn.name.clone()),
                        _ => {}
                    }
                }
            }
        }
        rooms.sort();
        names.creeps.sort();
        names.spawns.sort();
        names.rooms = rooms;
        Some(names)
    }
}
