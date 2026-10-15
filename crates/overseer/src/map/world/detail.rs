//! Rooms watched live in full detail, in priority order, within the server's per-user limit.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_util::StreamExt as _;
use overseer_net::rt::BoxFuture;

use super::{DetailState, WorldEvent};
use crate::map::RoomCoord;
use crate::{Closed, Error, RoomSnapshot, Rooms, Watch};

/// The subscription of one room.
enum Live {
    /// Not subscribed (refused).
    Off,
    /// Fetching the terrain before subscribing.
    Starting(BoxFuture<'static, Result<Watch<RoomSnapshot>, Error>>),
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
}

impl Details {
    /// The state of `room`, when wanted.
    pub(super) fn state(&self, room: RoomCoord) -> Option<&DetailState> {
        self.rooms.get(&room).map(|detail| &detail.state)
    }

    /// Makes `order` (unique, most important first) the wanted rooms: releases the rooms no
    /// longer wanted ([`WorldEvent::DetailGone`]) and subscribes the new ones
    /// ([`DetailState::Pending`]). When the list differs from the previous one, refused rooms
    /// are subscribed again. `rooms` is `None` without a signed-in user: every room is refused.
    pub(super) fn set(
        &mut self,
        order: Vec<RoomCoord>,
        rooms: Option<&Rooms>,
        shard: Option<&str>,
        events: &mut VecDeque<WorldEvent>,
    ) {
        if order == self.order {
            return;
        }
        for room in &self.order {
            if !order.contains(room) {
                self.rooms.remove(room);
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
            let detail = match rooms {
                Some(rooms) => Detail {
                    state: DetailState::Pending,
                    live: Live::Starting(subscribe(rooms, shard, room)),
                },
                None => Detail {
                    state: DetailState::Refused(Error::Anonymous.to_string()),
                    live: Live::Off,
                },
            };
            events.push_back(WorldEvent::Detail(room, detail.state.clone()));
            self.rooms.insert(room, detail);
        }
        self.order = order;
    }

    /// Delivers what arrived: [`DetailState::Live`] per update, [`DetailState::Refused`] when a
    /// subscription fails or the server refuses one. A refusal (`err@room`, sent each update
    /// for every subscription of the user beyond the limit, chosen at random by the server)
    /// releases the least important subscribed room, so the most important ones stay live.
    /// Returns whether anything changed.
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        events: &mut VecDeque<WorldEvent>,
    ) -> Result<bool, Closed> {
        let mut changed = false;
        let mut refusals = Vec::new();
        for &room in &self.order {
            let Some(detail) = self.rooms.get_mut(&room) else {
                continue;
            };
            loop {
                match &mut detail.live {
                    Live::Off => break,
                    Live::Starting(start) => match start.as_mut().poll(cx) {
                        Poll::Pending => break,
                        Poll::Ready(Ok(watch)) => detail.live = Live::Watching(watch),
                        Poll::Ready(Err(Error::Closed)) => return Err(Closed),
                        Poll::Ready(Err(error)) => {
                            changed = true;
                            detail.live = Live::Off;
                            detail.state = DetailState::Refused(error.to_string());
                            events.push_back(WorldEvent::Detail(room, detail.state.clone()));
                            break;
                        }
                    },
                    Live::Watching(watch) => match watch.poll_next_unpin(cx) {
                        Poll::Pending => break,
                        Poll::Ready(None) => return Err(Closed),
                        Poll::Ready(Some(snapshot)) => {
                            changed = true;
                            if let Some(error) = &snapshot.subscription_error {
                                refusals.push((room, error.clone()));
                            } else {
                                detail.state = DetailState::Live(Arc::clone(&snapshot));
                                events.push_back(WorldEvent::Detail(
                                    room,
                                    DetailState::Live(snapshot),
                                ));
                            }
                        }
                    },
                }
            }
        }
        for (room, error) in refusals {
            self.refuse(room, error, events);
        }
        Ok(changed)
    }

    /// Handles the refusal of `room`'s subscription: releases the least important subscribed
    /// room (possibly `room` itself) and reports it refused. Ignored when `room` was released
    /// meanwhile.
    fn refuse(&mut self, room: RoomCoord, error: String, events: &mut VecDeque<WorldEvent>) {
        let subscribed = |room: &RoomCoord| {
            matches!(
                self.rooms.get(room),
                Some(Detail {
                    live: Live::Watching(_),
                    ..
                })
            )
        };
        if !subscribed(&room) {
            return;
        }
        let Some(victim) = self.order.iter().rev().copied().find(subscribed) else {
            return;
        };
        if let Some(detail) = self.rooms.get_mut(&victim) {
            detail.live = Live::Off;
            detail.state = DetailState::Refused(error);
            events.push_back(WorldEvent::Detail(victim, detail.state.clone()));
        }
    }
}

/// Watches `room` (fetching its terrain first).
fn subscribe(
    rooms: &Rooms,
    shard: Option<&str>,
    room: RoomCoord,
) -> BoxFuture<'static, Result<Watch<RoomSnapshot>, Error>> {
    let (rooms, shard, room) = (rooms.clone(), shard.map(str::to_owned), room.to_string());
    Box::pin(async move {
        match shard {
            Some(shard) => rooms.watch_on(&shard, &room).await,
            None => rooms.watch(&room).await,
        }
    })
}
