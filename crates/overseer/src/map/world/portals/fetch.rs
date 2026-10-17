//! Destinations of rooms with portals: the room-objects requests, their order and their
//! rate limit. [`Portals::fetch`] starts the requests; [`Portals::poll`] completes them.

use std::cmp::Reverse;

use overseer_net::Client;
use overseer_net::api::WorldOperation;
use overseer_net::rt::{self, Instant};

use super::super::focus::{Budget, Focus, Tier, distance};
use super::super::terrain::reset_at;
use super::{
    Due, Fetch, PORTAL_BACKGROUND_IN_FLIGHT, PORTAL_FETCH_TIMEOUT, PORTAL_IN_FLIGHT, Portals, Tile,
};
use crate::map::{Map, RoomCoord, may_have_portals};

impl Portals {
    /// Requests the destinations of rooms with portals, up to [`PORTAL_IN_FLIGHT`] at once,
    /// as far as the request `budget` allows: first the viewed rooms and the rooms whose
    /// destinations are wanted now ([`Self::set_wanted`]), in their order, at the viewed
    /// priority; then the others at the background priority, up to
    /// [`PORTAL_BACKGROUND_IN_FLIGHT`] in flight, those whose fetch tells the most
    /// portal tiles not known otherwise first, nearest the centre first. The others wait
    /// while scans of rooms where portals usually stand are in flight, at most
    /// [`PORTAL_SCAN_SETTLE`](super::PORTAL_SCAN_SETTLE), and while a room with the same shape
    /// of portal tiles is being fetched (they may be partners, told by that fetch). A room
    /// whose fetch failed waits for its retry.
    pub(super) fn fetch(
        &mut self,
        map: &Map,
        shard: Option<&str>,
        (now, millis, focus): (Instant, u64, &Focus),
        budget: &mut Budget<'_>,
        due: &mut Due,
    ) {
        let mut waiting = Due::default();
        let available = |portals: &Self, room: RoomCoord, waiting: &mut Due| {
            if !portals.found.contains(&room)
                || portals.fetches.iter().any(|fetch| fetch.room == room)
            {
                return false;
            }
            match portals.failed.get(&room) {
                Some(failure) if failure.until > now => {
                    waiting.at(failure.until);
                    false
                }
                _ => true,
            }
        };
        let mut urgent = Vec::new();
        for &room in focus.viewed().iter().chain(&self.wanted) {
            if !urgent.contains(&room) && available(self, room, &mut waiting) {
                urgent.push(room);
            }
        }
        for &room in &urgent {
            if self.fetches.len() >= PORTAL_IN_FLIGHT {
                return;
            }
            if let Err(at) = budget.take(Tier::Viewed) {
                due.at(at);
                return;
            }
            self.start(map, shard, room, millis);
        }
        if self.fetches.len() >= PORTAL_IN_FLIGHT {
            return;
        }
        if let Some(settled) = self
            .scans
            .iter()
            .filter(|scan| scan.settled > now && may_have_portals(scan.room))
            .map(|scan| scan.settled)
            .min()
        {
            due.at(settled);
            return;
        }
        let centre = focus.centre();
        if self.fetches.len() >= PORTAL_BACKGROUND_IN_FLIGHT || self.found.is_empty() {
            return;
        }
        // Planning runs after every scan: the rooms waiting are ordered only when one of them
        // may go out.
        if let Some(at) = budget.ready_at(Tier::Background) {
            due.at(at);
            return;
        }
        let mut rest = self
            .found
            .iter()
            .copied()
            .filter(|room| !urgent.contains(room))
            .collect::<Vec<_>>();
        rest.retain(|&room| available(self, room, &mut waiting));
        let mut rest = rest
            .into_iter()
            .map(|room| {
                let key = (
                    Reverse(self.untold(room)),
                    distance(room, centre),
                    room.y,
                    room.x,
                );
                (key, room)
            })
            .collect::<Vec<_>>();
        rest.sort_unstable();
        let mut flying = self
            .fetches
            .iter()
            .map(|fetch| self.shape(fetch.room))
            .collect::<Vec<_>>();
        for (_, room) in rest {
            if self.fetches.len() >= PORTAL_BACKGROUND_IN_FLIGHT {
                return;
            }
            let shape = self.shape(room);
            if flying.contains(&shape) {
                continue;
            }
            if let Err(at) = budget.take(Tier::Background) {
                due.at(at);
                return;
            }
            self.start(map, shard, room, millis);
            flying.push(shape);
        }
        if let Some(at) = waiting.0 {
            due.at(at);
        }
    }

    /// Requests the destinations of `room` now.
    fn start(&mut self, map: &Map, shard: Option<&str>, room: RoomCoord, millis: u64) {
        let (map, shard, name) = (map.clone(), shard.map(str::to_owned), room.to_string());
        let request = Box::pin(async move {
            rt::timeout(PORTAL_FETCH_TIMEOUT, map.portals(&name, shard.as_deref()))
                .await
                .ok()
        });
        self.fetches.push(Fetch {
            room,
            sent_at: millis,
            request,
        });
    }

    /// The shape of the portal tiles of `room`'s map view: the tiles moved to the origin, in
    /// order (paired clusters have the same shape).
    fn shape(&self, room: RoomCoord) -> Vec<Tile> {
        let tiles = self.tiles.get(&room).map_or(&[][..], Vec::as_slice);
        let x = tiles.iter().map(|tile| tile.0).min().unwrap_or(0);
        let y = tiles.iter().map(|tile| tile.1).min().unwrap_or(0);
        let mut shape = tiles
            .iter()
            .map(|tile| (tile.0 - x, tile.1 - y))
            .collect::<Vec<_>>();
        shape.sort_unstable();
        shape
    }

    /// The portal tiles of `room`'s map view that no known portal leads to.
    fn untold(&self, room: RoomCoord) -> usize {
        let tiles = self.tiles.get(&room).map_or(&[][..], Vec::as_slice);
        let incoming = self.incoming.get(&room);
        tiles
            .iter()
            .filter(|tile| {
                incoming
                    .and_then(|incoming| incoming.get(*tile))
                    .is_none_or(|sources| sources.len() != 1)
            })
            .count()
    }

    /// When a room-objects request may go out: after a refusal and, when the endpoint's
    /// latest limit has no request left, after its reset.
    pub(super) fn ready_at(&self, client: &Client, now: Instant) -> Instant {
        let mut ready = self.not_before.unwrap_or(now);
        if let Some(limit) = WorldOperation::RoomObjects.rate_limit(client)
            && limit.remaining == 0
        {
            ready = ready.max(reset_at(limit.reset, now));
        }
        ready
    }
}
