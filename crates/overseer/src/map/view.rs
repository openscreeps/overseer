//! A changing set of room map views.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures_util::future::select_all;
use overseer_net::core::socket::{Channel, Payload};
use overseer_net::model::world::RoomMap;

use crate::{Closed, Overseer, Watch};

/// The rooms a [`MapView::set_rooms`] call subscribed and released.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewChange {
    /// Rooms that entered the view, in name order.
    pub added: Vec<String>,
    /// Rooms that left the view, in name order.
    pub removed: Vec<String>,
}

/// The map views (`roomMap2`) of the rooms visible on a map screen.
///
/// Created by [`Map::view`](crate::Map::view). Holds one [`Watch<RoomMap>`] per room in view:
/// [`Self::set_rooms`] watches the rooms that enter the view and drops the watches of those
/// that leave, so scrolling only subscribes and releases the rooms at the edges. Watches are
/// reference-counted with every other handle of the session: a room another view also shows
/// stays subscribed until both release it.
#[derive(Debug)]
pub struct MapView {
    overseer: Overseer,
    shard: Option<String>,
    rooms: BTreeMap<String, Watch<RoomMap>>,
}

impl MapView {
    pub(crate) fn new(overseer: Overseer, shard: Option<String>) -> Self {
        Self {
            overseer,
            shard,
            rooms: BTreeMap::new(),
        }
    }

    /// The view's shard; `None` on single-world servers.
    #[must_use]
    pub fn shard(&self) -> Option<&str> {
        self.shard.as_deref()
    }

    /// The rooms in view, in name order.
    pub fn rooms(&self) -> impl Iterator<Item = &str> {
        self.rooms.keys().map(String::as_str)
    }

    /// Makes `rooms` the rooms in view: watches the new ones and releases those no longer
    /// listed. Returns what changed.
    pub fn set_rooms<R: Into<String>>(&mut self, rooms: impl IntoIterator<Item = R>) -> ViewChange {
        let wanted = rooms
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<String>>();
        let mut change = ViewChange::default();
        self.rooms.retain(|room, _| {
            let keep = wanted.contains(room);
            if !keep {
                change.removed.push(room.clone());
            }
            keep
        });
        for room in wanted {
            if self.rooms.contains_key(&room) {
                continue;
            }
            let channel = Channel::RoomMap {
                shard: self.shard.clone(),
                room: room.clone(),
            };
            let watch = self.overseer.watch(channel, |payload| match payload {
                Payload::RoomMap(view) => Some(view),
                _ => None,
            });
            change.added.push(room.clone());
            self.rooms.insert(room, watch);
        }
        change
    }

    /// The latest map view of `room`; `None` when the room is not in view or no view arrived
    /// yet.
    #[must_use]
    pub fn latest(&self, room: &str) -> Option<Arc<RoomMap>> {
        self.rooms.get(room).and_then(Watch::latest)
    }

    /// The latest map view of every room in view that has one, by room name.
    #[must_use]
    pub fn views(&self) -> BTreeMap<String, Arc<RoomMap>> {
        self.rooms
            .iter()
            .filter_map(|(room, watch)| Some((room.clone(), watch.latest()?)))
            .collect()
    }

    /// Waits until a room in view has a map view this handle has not returned yet and returns
    /// the room and its view. Never completes while no room is in view. Cancel-safe.
    ///
    /// # Errors
    ///
    /// Returns [`Closed`] once the session is shut down.
    pub async fn changed(&mut self) -> Result<(String, Arc<RoomMap>), Closed> {
        if self.rooms.is_empty() {
            return std::future::pending().await;
        }
        let waits = self
            .rooms
            .iter_mut()
            .map(|(room, watch)| Box::pin(async move { (room.as_str(), watch.changed().await) }));
        let ((room, view), _, _) = select_all(waits).await;
        Ok((room.to_owned(), view?))
    }
}
