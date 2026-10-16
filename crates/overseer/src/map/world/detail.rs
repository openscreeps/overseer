//! Rooms watched live in full detail, in priority order, within the server's per-user limit.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::StreamExt as _;
use overseer_net::rt::Instant;

use super::terrain::Terrains;
use super::{DetailState, WorldEvent};
use crate::map::RoomCoord;
use crate::{Closed, Error, RoomSnapshot, Rooms, Watch};

/// How long a room wanted in detail may wait for its first update (or a refusal) while it
/// holds back portal scans (see [`WorldMap`](super::WorldMap)): the server answers the room
/// before the scans' subscriptions, which would otherwise queue ahead of it.
pub const DETAIL_FIRST: Duration = Duration::from_secs(5);

/// The subscription of one room.
enum Live {
    /// Not subscribed (refused).
    Off,
    /// Subscribed; `terrain` tells whether the subscription has the room's terrain (its
    /// updates become snapshots once it has).
    Watching {
        watch: Watch<RoomSnapshot>,
        terrain: bool,
    },
}

/// One room wanted in detail.
struct Detail {
    state: DetailState,
    live: Live,
    /// Until then, while [`DetailState::Pending`], the room holds back portal scans.
    first_by: Instant,
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

    /// While a wanted room waits for its first update (or a refusal), at most
    /// [`DETAIL_FIRST`] after it was subscribed: when the first of those waits ends.
    pub(super) fn holding(&self, now: Instant) -> Option<Instant> {
        self.rooms
            .values()
            .filter(|detail| {
                matches!(detail.state, DetailState::Pending)
                    && matches!(detail.live, Live::Watching { .. })
                    && detail.first_by > now
            })
            .map(|detail| detail.first_by)
            .min()
    }

    /// Makes `order` (unique, most important first) the wanted rooms: releases the rooms no
    /// longer wanted ([`WorldEvent::DetailGone`]) and subscribes the new ones at once
    /// ([`DetailState::Pending`]), with no request of their own: their terrain comes from the
    /// world map's terrain requests (the caller wants it there) and is given to the
    /// subscription when `terrain` knows it; a room the server has no terrain for is refused.
    /// When the list differs from the previous one, refused rooms are subscribed again.
    /// `rooms` is `None` without a signed-in user: every room is refused.
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
        let mut changed = false;
        for room in &self.order {
            if !order.contains(room) {
                if let Some(detail) = self.rooms.remove(room) {
                    changed |= matches!(detail.live, Live::Watching { .. });
                }
                events.push_back(WorldEvent::DetailGone(*room));
            }
        }
        let now = Instant::now();
        for &room in &order {
            let retry = match self.rooms.get(&room) {
                None => true,
                Some(detail) => matches!(detail.live, Live::Off),
            };
            if !retry {
                continue;
            }
            let (state, live) = self.subscribe(room, terrain);
            changed |= matches!(live, Live::Watching { .. });
            events.push_back(WorldEvent::Detail(room, state.clone()));
            self.rooms.insert(
                room,
                Detail {
                    state,
                    live,
                    first_by: now + DETAIL_FIRST,
                },
            );
        }
        self.order = order;
        if changed {
            self.renew_barrier();
        }
    }

    /// Subscribes `room`, with its terrain when `terrain` knows it: its state and
    /// subscription.
    fn subscribe(&self, room: RoomCoord, terrain: &Terrains) -> (DetailState, Live) {
        let Some(rooms) = &self.access else {
            return (
                DetailState::Refused(Error::Anonymous.to_string()),
                Live::Off,
            );
        };
        if terrain.is_absent(room) {
            return (DetailState::Refused(no_room(room)), Live::Off);
        }
        let known = terrain.known.get(&room).cloned();
        let given = known.is_some();
        match rooms.watch_room(self.shard.as_deref(), &room.to_string(), known) {
            Ok(watch) => (
                DetailState::Pending,
                Live::Watching {
                    watch,
                    terrain: given,
                },
            ),
            Err(error) => (DetailState::Refused(error.to_string()), Live::Off),
        }
    }

    /// Gives the subscriptions waiting for their room's terrain the terrains `terrain` now
    /// knows, and releases (refused) the rooms the server has no terrain for; then, after a
    /// release, puts a new barrier behind it. Returns whether anything changed.
    fn follow_terrain(&mut self, terrain: &Terrains, events: &mut VecDeque<WorldEvent>) -> bool {
        let Some(rooms) = &self.access else {
            return false;
        };
        let mut changed = false;
        let mut released = false;
        for &room in &self.order {
            let Some(detail) = self.rooms.get_mut(&room) else {
                continue;
            };
            let Live::Watching { terrain: given, .. } = &mut detail.live else {
                continue;
            };
            if *given {
                continue;
            }
            if let Some(known) = terrain.known.get(&room) {
                rooms.set_terrain(self.shard.as_deref(), &room.to_string(), Arc::clone(known));
                *given = true;
                changed = true;
            } else if terrain.is_absent(room) {
                detail.live = Live::Off;
                detail.state = DetailState::Refused(no_room(room));
                events.push_back(WorldEvent::Detail(room, detail.state.clone()));
                changed = true;
                released = true;
            }
        }
        if released {
            self.renew_barrier();
        }
        changed
    }

    /// Gives the subscriptions the terrains that arrived and delivers what arrived:
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
        let mut changed = self.follow_terrain(terrain, events);
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
                live: Live::Watching { watch, .. },
                ..
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
                    live: Live::Watching { .. },
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

/// Why `room` cannot be watched: the server has no terrain for it.
fn no_room(room: RoomCoord) -> String {
    format!("the server has no room {room}")
}
