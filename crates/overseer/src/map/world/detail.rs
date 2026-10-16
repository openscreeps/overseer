//! Rooms watched live in full detail, in priority order, within the server's per-user limit.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::StreamExt as _;

use super::terrain::Terrains;
use super::{DetailState, WorldEvent};
use crate::map::RoomCoord;
use crate::{Closed, Error, RoomSnapshot, Rooms, Watch};

/// The subscription of one room.
enum Live {
    /// Not subscribed (refused).
    Off,
    /// Waiting for the room's terrain from the world map's terrain requests.
    Terrain,
    /// Subscribed.
    Watching(Watch<RoomSnapshot>),
}

/// One room wanted in detail.
struct Detail {
    state: DetailState,
    live: Live,
}

/// The rooms wanted in detail and their subscriptions.
#[derive(Default)]
pub(super) struct Details {
    /// The wanted rooms, most important first.
    order: Vec<RoomCoord>,
    rooms: HashMap<RoomCoord, Detail>,
    /// Subscribes the rooms; `None` without a signed-in user.
    access: Option<Rooms>,
    shard: Option<String>,
    /// The [barrier](Rooms::barrier) behind the latest subscription or release, until it
    /// yields; `None` once it did: refusals arriving from then on reflect the current
    /// subscriptions (see [`Self::poll`]).
    barrier: Option<Watch<()>>,
}

impl Details {
    /// The state of `room`, when wanted.
    pub(super) fn state(&self, room: RoomCoord) -> Option<&DetailState> {
        self.rooms.get(&room).map(|detail| &detail.state)
    }

    /// Makes `order` (unique, most important first) the wanted rooms: releases the rooms no
    /// longer wanted ([`WorldEvent::DetailGone`]) and subscribes the new ones
    /// ([`DetailState::Pending`]) once `terrain` knows their terrain (the caller wants it
    /// there), so subscribing makes no request of its own. When the list differs from the
    /// previous one, refused rooms are subscribed again. `rooms` is `None` without a signed-in
    /// user: every room is refused.
    pub(super) fn set(
        &mut self,
        order: Vec<RoomCoord>,
        rooms: Option<&Rooms>,
        shard: Option<&str>,
        terrain: &Terrains,
        events: &mut VecDeque<WorldEvent>,
    ) {
        if order == self.order {
            return;
        }
        self.access = rooms.cloned();
        self.shard = shard.map(str::to_owned);
        let mut released = false;
        for room in &self.order {
            if !order.contains(room) {
                if let Some(detail) = self.rooms.remove(room) {
                    released |= matches!(detail.live, Live::Watching(_));
                }
                events.push_back(WorldEvent::DetailGone(*room));
            }
        }
        for &room in &order {
            let retry = match self.rooms.get(&room) {
                None => true,
                Some(detail) => matches!(detail.live, Live::Off),
            };
            if !retry {
                continue;
            }
            let detail = if self.access.is_some() {
                Detail {
                    state: DetailState::Pending,
                    live: Live::Terrain,
                }
            } else {
                Detail {
                    state: DetailState::Refused(Error::Anonymous.to_string()),
                    live: Live::Off,
                }
            };
            events.push_back(WorldEvent::Detail(room, detail.state.clone()));
            self.rooms.insert(room, detail);
        }
        self.order = order;
        self.start(terrain, events, released);
    }

    /// Subscribes the rooms waiting for a terrain `terrain` now knows, and refuses those the
    /// server does not know; then, when it subscribed a room or `released` one before, puts a
    /// new barrier behind those changes. Returns whether anything changed.
    fn start(
        &mut self,
        terrain: &Terrains,
        events: &mut VecDeque<WorldEvent>,
        released: bool,
    ) -> bool {
        let Some(rooms) = &self.access else {
            return false;
        };
        let mut changed = false;
        let mut subscribed = false;
        for &room in &self.order {
            let Some(detail) = self.rooms.get_mut(&room) else {
                continue;
            };
            if !matches!(detail.live, Live::Terrain) {
                continue;
            }
            let name = room.to_string();
            let watch = if let Some(known) = terrain.known.get(&room) {
                rooms
                    .watch_terrain(self.shard.as_deref(), &name, Arc::clone(known))
                    .map_err(|error| error.to_string())
            } else if terrain.is_absent(room) {
                Err(format!("the server has no room {name}"))
            } else {
                continue;
            };
            changed = true;
            match watch {
                Ok(watch) => {
                    detail.live = Live::Watching(watch);
                    subscribed = true;
                }
                Err(error) => {
                    detail.live = Live::Off;
                    detail.state = DetailState::Refused(error);
                    events.push_back(WorldEvent::Detail(room, detail.state.clone()));
                }
            }
        }
        if subscribed || released {
            self.renew_barrier();
        }
        changed
    }

    /// Subscribes the rooms whose terrain arrived and delivers what arrived:
    /// [`DetailState::Live`] per update (also added to `live`, for what it shows of the room's
    /// overview and portals), [`DetailState::Refused`] when a subscription fails or the server
    /// refuses one. Returns whether anything changed.
    ///
    /// The server refuses (`err@room`) every subscription of the user beyond its limit, chosen
    /// at random, on each update until it processes a release, and answers a subscription with
    /// a refusal counted at its latest update. A refusal therefore releases a room only when it
    /// reflects the current subscriptions: when it arrived after the barrier behind the latest
    /// subscription or release yielded, so the server sent it after processing them (refusals
    /// found when the barrier yields may predate it and are dropped too; the server repeats
    /// those that still hold on its next update). It then releases the least important
    /// subscribed room, so the most important ones stay live, and puts a new barrier behind
    /// that release: refusals arriving meanwhile, of other rooms or of a room subscribed again,
    /// release nothing.
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        terrain: &Terrains,
        events: &mut VecDeque<WorldEvent>,
        live: &mut Vec<(RoomCoord, Arc<RoomSnapshot>)>,
    ) -> Result<bool, Closed> {
        let mut changed = self.start(terrain, events, false);
        loop {
            let current = self.settle(cx)?;
            let (updated, refusal) = self.drain(cx, current, events, live)?;
            changed |= updated;
            let Some(error) = refusal else {
                return Ok(changed);
            };
            self.release_least_important(error, events);
            changed = true;
        }
    }

    /// Polls the barrier; whether it had yielded before this call (refusals reflect the
    /// current subscriptions). Once it yields it is released.
    fn settle(&mut self, cx: &mut Context<'_>) -> Result<bool, Closed> {
        let Some(barrier) = &mut self.barrier else {
            return Ok(true);
        };
        match barrier.poll_next_unpin(cx) {
            Poll::Pending => {}
            Poll::Ready(Some(_)) => self.barrier = None,
            Poll::Ready(None) => return Err(Closed),
        }
        Ok(false)
    }

    /// Takes what arrived for every subscribed room: each update becomes
    /// [`DetailState::Live`] (and is added to `live`); refusals are dropped, but the first one
    /// is returned when they are `current`. Returns whether a room was updated and that
    /// refusal.
    fn drain(
        &mut self,
        cx: &mut Context<'_>,
        current: bool,
        events: &mut VecDeque<WorldEvent>,
        live: &mut Vec<(RoomCoord, Arc<RoomSnapshot>)>,
    ) -> Result<(bool, Option<String>), Closed> {
        let mut updated = false;
        let mut refusal = None;
        for &room in &self.order {
            let Some(Detail {
                state,
                live: Live::Watching(watch),
            }) = self.rooms.get_mut(&room)
            else {
                continue;
            };
            loop {
                match watch.poll_next_unpin(cx) {
                    Poll::Pending => break,
                    Poll::Ready(None) => return Err(Closed),
                    Poll::Ready(Some(snapshot)) => {
                        if let Some(error) = &snapshot.subscription_error {
                            if current && refusal.is_none() {
                                refusal = Some(error.clone());
                            }
                        } else {
                            updated = true;
                            *state = DetailState::Live(Arc::clone(&snapshot));
                            live.push((room, Arc::clone(&snapshot)));
                            events.push_back(WorldEvent::Detail(room, DetailState::Live(snapshot)));
                        }
                    }
                }
            }
        }
        Ok((updated, refusal))
    }

    /// Releases the least important subscribed room, reports it refused with `error` and puts
    /// a new barrier behind the release.
    fn release_least_important(&mut self, error: String, events: &mut VecDeque<WorldEvent>) {
        let subscribed = |room: &RoomCoord| {
            matches!(
                self.rooms.get(room),
                Some(Detail {
                    live: Live::Watching(_),
                    ..
                })
            )
        };
        let Some(victim) = self.order.iter().rev().copied().find(subscribed) else {
            return;
        };
        if let Some(detail) = self.rooms.get_mut(&victim) {
            detail.live = Live::Off;
            detail.state = DetailState::Refused(error);
            events.push_back(WorldEvent::Detail(victim, detail.state.clone()));
        }
        self.renew_barrier();
    }

    /// Replaces the barrier with one behind the subscriptions and releases sent so far.
    fn renew_barrier(&mut self) {
        // Released first, so the frames read in order: the old barrier's release, the new one.
        self.barrier = None;
        self.barrier = self
            .access
            .as_ref()
            .map(|rooms| rooms.barrier(self.shard.as_deref()));
    }
}
