//! What a [`WorldMap`](super::WorldMap) knows of each room, readable from anywhere.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use tokio::sync::watch;

use super::{Portal, RoomCoord, RoomRect, RoomStat};

/// What a [`WorldMap`](super::WorldMap) knows of each room: its overview and its portals,
/// fetched, seeded from the app's cache or derived from a live room. The world map is its only
/// writer; this is the read-only handle, from [`WorldMap::knowledge`](super::WorldMap::knowledge).
///
/// Cheap to clone and readable outside the task that owns the world map (every clone sees the
/// same rooms). Every change bumps [`Self::revision`] once (a request's whole batch of overviews
/// is one change); [`Self::watch`] awaits the next one. The same changes are reported as
/// [`WorldEvent::Stats`](super::WorldEvent::Stats) and
/// [`WorldEvent::Portals`](super::WorldEvent::Portals), which the app persists.
#[derive(Clone)]
pub struct WorldKnowledge {
    shared: Arc<Shared>,
}

struct Shared {
    rooms: RwLock<Rooms>,
    revision: watch::Sender<u64>,
}

#[derive(Default)]
struct Rooms {
    stats: HashMap<RoomCoord, Arc<RoomStat>>,
    portals: HashMap<RoomCoord, Arc<[Portal]>>,
}

impl WorldKnowledge {
    /// Nothing known yet; revision 0.
    pub(crate) fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                rooms: RwLock::new(Rooms::default()),
                revision: watch::Sender::new(0),
            }),
        }
    }

    /// The overview of `room`, when known.
    #[must_use]
    pub fn stat(&self, room: RoomCoord) -> Option<Arc<RoomStat>> {
        self.read().stats.get(&room).cloned()
    }

    /// The portals of `room` (empty: it has none), when known.
    #[must_use]
    pub fn portals(&self, room: RoomCoord) -> Option<Arc<[Portal]>> {
        self.read().portals.get(&room).cloned()
    }

    /// The rooms of `rect` with an overview, row by row from the north-west corner.
    #[must_use]
    pub fn stats_in(&self, rect: RoomRect) -> Vec<(RoomCoord, Arc<RoomStat>)> {
        let rooms = self.read();
        rect.iter()
            .filter_map(|room| Some((room, Arc::clone(rooms.stats.get(&room)?))))
            .collect()
    }

    /// The rooms of `rect` whose portals are known (empty: none), row by row from the
    /// north-west corner.
    #[must_use]
    pub fn portals_in(&self, rect: RoomRect) -> Vec<(RoomCoord, Arc<[Portal]>)> {
        let rooms = self.read();
        rect.iter()
            .filter_map(|room| Some((room, Arc::clone(rooms.portals.get(&room)?))))
            .collect()
    }

    /// Bumped by every change of what this holds; 0 while nothing is known.
    #[must_use]
    pub fn revision(&self) -> u64 {
        *self.shared.revision.borrow()
    }

    /// The revision, to await changes: `changed().await` returns at the next one (or at once
    /// when one happened since the receiver last looked). Its value is [`Self::revision`].
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.shared.revision.subscribe()
    }

    /// Replaces the overviews of `stats` as one change.
    pub(crate) fn set_stats<'a>(
        &self,
        stats: impl IntoIterator<Item = &'a (RoomCoord, Arc<RoomStat>)>,
    ) {
        let mut rooms = self.write();
        let mut changed = false;
        for (room, stat) in stats {
            rooms.stats.insert(*room, Arc::clone(stat));
            changed = true;
        }
        drop(rooms);
        if changed {
            self.bump();
        }
    }

    /// Replaces the portals of `room` as one change.
    pub(crate) fn set_portals(&self, room: RoomCoord, portals: Arc<[Portal]>) {
        self.write().portals.insert(room, portals);
        self.bump();
    }

    fn bump(&self) {
        self.shared
            .revision
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    fn read(&self) -> RwLockReadGuard<'_, Rooms> {
        // Critical sections only move values in and out of maps.
        self.shared
            .rooms
            .read()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Rooms> {
        self.shared
            .rooms
            .write()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for WorldKnowledge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rooms = self.read();
        f.debug_struct("WorldKnowledge")
            .field("revision", &self.revision())
            .field("stats", &rooms.stats.len())
            .field("portals", &rooms.portals.len())
            .finish()
    }
}
