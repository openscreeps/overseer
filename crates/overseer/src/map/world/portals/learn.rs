//! Learning the portals of a room: from its map view (with pending destinations), from a
//! room-objects request or a live room, or inferred from the paired portals that lead to it.
//! Each learnt room goes to the [`WorldKnowledge`](crate::map::WorldKnowledge) and to the
//! events.

use std::collections::VecDeque;
use std::sync::Arc;

use overseer_net::model::world::RoomMap;
use overseer_net::rt::Instant;

use super::super::WorldEvent;
use super::super::stats::epoch_millis;
use super::{Known, Portals, lands, stale_at};
use crate::map::{Portal, PortalDestination, RoomCoord};

impl Portals {
    /// A room's map view arrived. Without portals, the room has none. With portals, their
    /// tiles are known at once and reported with pending destinations (the map draws them),
    /// unless the known portals stand on the same tiles (their destinations stay shown until
    /// fetched again); the destinations are wanted (unless its paired portals tell them).
    pub(super) fn scanned(
        &mut self,
        room: RoomCoord,
        view: &RoomMap,
        events: &mut VecDeque<WorldEvent>,
    ) {
        let mut tiles = view
            .p
            .iter()
            .flatten()
            .filter_map(|&[x, y]| Some((u8::try_from(x).ok()?, u8::try_from(y).ok()?)))
            .collect::<Vec<_>>();
        if tiles.is_empty() {
            self.tiles.remove(&room);
            self.record(room, Vec::new(), None, events);
            return;
        }
        tiles.sort_unstable_by_key(|&(x, y)| (y, x));
        tiles.dedup();
        let unchanged = self.knowledge.portals(room).is_some_and(|known| {
            known
                .iter()
                .map(|portal| (portal.x, portal.y))
                .eq(tiles.iter().copied())
        });
        self.tiles.insert(room, tiles.clone());
        self.found.insert(room);
        if !self.infer(room, events) && !unchanged {
            let pending = tiles
                .into_iter()
                .map(|(x, y)| Portal::pending(room, x, y))
                .collect();
            self.record(room, pending, None, events);
        }
    }

    /// Infers the portals of `room`, waiting for its destinations, when every portal tile of
    /// its map view is the destination of a known portal of the shard: each leads back to the
    /// portal that leads to it, unstable and decaying alike (screeps.com places portals in
    /// such pairs). Returns whether it did.
    fn infer(&mut self, room: RoomCoord, events: &mut VecDeque<WorldEvent>) -> bool {
        if !self.found.contains(&room) {
            return false;
        }
        let (Some(tiles), Some(incoming)) = (self.tiles.get(&room), self.incoming.get(&room))
        else {
            return false;
        };
        let Some(mut portals) = tiles
            .iter()
            .map(|tile| {
                // Exactly one known portal lands on the tile: two would not be a pair.
                let [source] = incoming.get(tile)?.as_slice() else {
                    return None;
                };
                Some(Portal {
                    room,
                    x: tile.0,
                    y: tile.1,
                    destination: Some(PortalDestination {
                        room: source.room,
                        shard: None,
                        x: Some(source.x),
                        y: Some(source.y),
                    }),
                    expires_at: source.expires_at,
                    decay_time: source.decay_time,
                })
            })
            .collect::<Option<Vec<_>>>()
        else {
            return false;
        };
        portals.sort_unstable_by_key(|portal| (portal.y, portal.x));
        self.found.remove(&room);
        self.failed.remove(&room);
        self.store(room, portals, None, true, events);
        true
    }

    /// Stores the portals of `room` fetched or scanned now (shown by a live room at
    /// `live_at`) and queues them; then infers the portals of the rooms they lead to.
    pub(super) fn record(
        &mut self,
        room: RoomCoord,
        portals: Vec<Portal>,
        live_at: Option<u64>,
        events: &mut VecDeque<WorldEvent>,
    ) {
        if let Some(old) = self.knowledge.portals(room) {
            for portal in old.iter() {
                if let Some((to, tile)) = lands(portal)
                    && let Some(sources) = self
                        .incoming
                        .get_mut(&to)
                        .and_then(|incoming| incoming.get_mut(&tile))
                {
                    sources.retain(|from| from.room != room);
                }
            }
        }
        let mut targets = Vec::new();
        for portal in &portals {
            if let Some((to, tile)) = lands(portal) {
                self.incoming
                    .entry(to)
                    .or_default()
                    .entry(tile)
                    .or_default()
                    .push(portal.clone());
                if !targets.contains(&to) {
                    targets.push(to);
                }
            }
        }
        self.store(room, portals, live_at, false, events);
        for target in targets {
            self.infer(target, events);
        }
    }

    /// Stores the portals of `room` learnt now and queues them.
    fn store(
        &mut self,
        room: RoomCoord,
        portals: Vec<Portal>,
        live_at: Option<u64>,
        inferred: bool,
        events: &mut VecDeque<WorldEvent>,
    ) {
        let scanned_at = epoch_millis();
        let portals = Arc::<[Portal]>::from(portals);
        let stale_at = stale_at(room, &portals, scanned_at, Instant::now(), scanned_at);
        let some = !portals.is_empty() || self.tiles.contains_key(&room);
        let learnt = self
            .known
            .insert(
                room,
                Known {
                    // A room whose map view shows portals though none came (a portal its
                    // objects do not list) counts as having some, so its live minimap does not
                    // ask again.
                    some,
                    inferred,
                    scanned_at,
                    live_at,
                    stale_at,
                },
            )
            .is_none();
        self.check_at(stale_at);
        if learnt {
            self.learn(room);
        }
        self.knowledge.set_portals(room, Arc::clone(&portals));
        events.push_back(WorldEvent::Portals(room, portals));
    }
}
