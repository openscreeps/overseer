//! The data loop of one world: viewers say what they need, the world's
//! [`WorldMap`] fetches and subscribes, and its events come back to the viewers.
//!
//! [`Feed::start`] moves the `WorldMap` into a task that owns it and alternates between
//! [`WorldMap::next`] (cancel-safe) and commands from the UI. Each viewer subscribes
//! ([`Feed::subscribe`]) with a [`Reach`] and a callback, and on camera changes sends its
//! [`Interest`] ([`Subscription::set_interest`]); the task merges the interests of every
//! subscriber ([`merge`]) into the world's. A new subscriber first gets what the world already
//! has for its reach (terrain, overviews, minimaps, live rooms), then every event.
//!
//! The task ends when the feed and every subscription are dropped or the session closes;
//! dropping the `WorldMap` then ends its subscriptions.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;

use futures_util::future::{Either, select};
use overseer::core::api::Terrain;
use overseer::map::{Interest, RoomCoord, RoomRect, WorldEvent, WorldMap};
use tokio::sync::mpsc;

/// What a subscriber shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The whole world (the map).
    World,
    /// One room (a room control).
    Room(RoomCoord),
}

/// A subscriber's callback.
type Deliver = Rc<dyn Fn(&WorldEvent)>;

/// What the UI asks of the task.
enum Command {
    Subscribe(u64, Reach, Deliver),
    Interest(u64, Interest),
    Unsubscribe(u64),
}

/// A running world; cheap to clone.
#[derive(Clone)]
pub struct Feed {
    commands: mpsc::UnboundedSender<Command>,
    next_id: Rc<Cell<u64>>,
    bounds: RoomRect,
}

impl Feed {
    /// Runs `world` on the browser's event loop; `on_terrain` sees every newly fetched room
    /// terrain (to cache it).
    pub fn start(world: WorldMap, on_terrain: impl Fn(RoomCoord, &Arc<Terrain>) + 'static) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let bounds = world.bounds();
        wasm_bindgen_futures::spawn_local(run(world, receiver, on_terrain));
        Self {
            commands,
            next_id: Rc::new(Cell::new(0)),
            bounds,
        }
    }

    /// The rooms of the world.
    pub const fn bounds(&self) -> RoomRect {
        self.bounds
    }

    /// Delivers what the world has for `reach`, then every event, to `deliver` until the
    /// subscription is dropped.
    pub fn subscribe(&self, reach: Reach, deliver: impl Fn(&WorldEvent) + 'static) -> Subscription {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        // A send fails only once the task ended; the subscription is idle then.
        let _ = self
            .commands
            .send(Command::Subscribe(id, reach, Rc::new(deliver)));
        Subscription {
            id,
            commands: self.commands.clone(),
        }
    }
}

/// A subscriber of a [`Feed`]; unsubscribes when dropped.
pub struct Subscription {
    id: u64,
    commands: mpsc::UnboundedSender<Command>,
}

impl Subscription {
    /// What the subscriber needs now.
    pub fn set_interest(&self, interest: Interest) {
        // A send fails only once the task ended.
        let _ = self.commands.send(Command::Interest(self.id, interest));
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Unsubscribe(self.id));
    }
}

/// A subscriber in the task.
struct Sink {
    reach: Reach,
    deliver: Deliver,
    interest: Interest,
}

/// The task: owns `world`, serves commands and forwards events.
async fn run(
    mut world: WorldMap,
    mut commands: mpsc::UnboundedReceiver<Command>,
    on_terrain: impl Fn(RoomCoord, &Arc<Terrain>),
) {
    let mut sinks: BTreeMap<u64, Sink> = BTreeMap::new();
    loop {
        let step = {
            let event = pin!(world.next());
            let command = pin!(commands.recv());
            match select(event, command).await {
                Either::Left((event, _)) => Either::Left(event),
                Either::Right((command, _)) => Either::Right(command),
            }
        };
        match step {
            Either::Left(Ok(event)) => {
                if let WorldEvent::Terrain(room, terrain) = &event {
                    on_terrain(*room, terrain);
                }
                let delivers: Vec<Deliver> =
                    sinks.values().map(|sink| sink.deliver.clone()).collect();
                for deliver in delivers {
                    deliver(&event);
                }
            }
            // The session closed, or the UI dropped the feed and every subscription.
            Either::Left(Err(_)) | Either::Right(None) => break,
            Either::Right(Some(command)) => {
                match command {
                    Command::Subscribe(id, reach, deliver) => {
                        replay(&world, reach, &deliver);
                        sinks.insert(
                            id,
                            Sink {
                                reach,
                                deliver,
                                interest: Interest::default(),
                            },
                        );
                    }
                    Command::Interest(id, interest) => {
                        if let Some(sink) = sinks.get_mut(&id) {
                            sink.interest = interest;
                        }
                    }
                    Command::Unsubscribe(id) => {
                        sinks.remove(&id);
                    }
                }
                let interests: Vec<(Reach, &Interest)> = sinks
                    .values()
                    .map(|sink| (sink.reach, &sink.interest))
                    .collect();
                world.set_interest(&merge(&interests));
            }
        }
    }
}

/// Delivers what `world` has for `reach`.
fn replay(world: &WorldMap, reach: Reach, deliver: &Deliver) {
    let rect = match reach {
        Reach::World => world.bounds(),
        Reach::Room(room) => RoomRect {
            min: room,
            max: room,
        },
    };
    let mut stats = Vec::new();
    for room in rect.iter() {
        if let Some(terrain) = world.terrain(room) {
            deliver(&WorldEvent::Terrain(room, terrain));
        }
        if let Some(stat) = world.stat(room) {
            stats.push((room, stat));
        }
        if let Some(minimap) = world.minimap(room) {
            deliver(&WorldEvent::Minimap(room, minimap));
        }
        if let Some(detail) = world.detail(room) {
            deliver(&WorldEvent::Detail(room, detail.clone()));
        }
    }
    if !stats.is_empty() {
        deliver(&WorldEvent::Stats(stats));
    }
}

/// The world's interest from its subscribers', most important first:
///
/// - overviews: the world subscribers' rectangle (a room control's room is then normally in
///   it, and its overview cached); without one, the rectangle around the room controls'
///   rooms;
/// - terrain and minimaps: the world subscribers' rooms, then the room controls';
/// - live rooms: the world's first (the room under the map's centre), then the room
///   controls'. The map's other rooms stay minimaps: the server caps live rooms per player
///   across every connection (2 on screeps.com and `OpenScreeps`), and the rest of that budget
///   goes to popped-out windows.
///
/// Rooms appear once, at their first place.
pub fn merge(interests: &[(Reach, &Interest)]) -> Interest {
    let world = || {
        interests
            .iter()
            .filter(|(reach, _)| *reach == Reach::World)
            .map(|(_, interest)| *interest)
    };
    let rooms = || {
        interests
            .iter()
            .filter(|(reach, _)| *reach != Reach::World)
            .map(|(_, interest)| *interest)
    };
    let mut stats = world().filter_map(|interest| interest.stats).reduce(union);
    if stats.is_none() {
        stats = interests
            .iter()
            .filter_map(|(reach, interest)| match reach {
                Reach::Room(room) => Some(interest.stats.unwrap_or(RoomRect {
                    min: *room,
                    max: *room,
                })),
                Reach::World => None,
            })
            .reduce(union);
    }
    let mut merged = Interest {
        stats,
        ..Interest::default()
    };
    for interest in world().chain(rooms()) {
        extend(&mut merged.terrain, &interest.terrain);
        extend(&mut merged.minimaps, &interest.minimaps);
    }
    for interest in world() {
        extend(
            &mut merged.detail,
            interest.detail.get(..1).unwrap_or_default(),
        );
    }
    for interest in rooms() {
        extend(&mut merged.detail, &interest.detail);
    }
    merged
}

/// The smallest rectangle holding `a` and `b`.
fn union(a: RoomRect, b: RoomRect) -> RoomRect {
    RoomRect {
        min: RoomCoord {
            x: a.min.x.min(b.min.x),
            y: a.min.y.min(b.min.y),
        },
        max: RoomCoord {
            x: a.max.x.max(b.max.x),
            y: a.max.y.max(b.max.y),
        },
    }
}

/// Appends the rooms of `more` that `rooms` lacks.
fn extend(rooms: &mut Vec<RoomCoord>, more: &[RoomCoord]) {
    for room in more {
        if !rooms.contains(room) {
            rooms.push(*room);
        }
    }
}

#[cfg(test)]
mod tests {
    use overseer::map::{Interest, RoomCoord, RoomRect};

    use super::{Reach, merge};

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    fn rooms(names: &[&str]) -> Vec<RoomCoord> {
        names.iter().map(|name| room(name)).collect()
    }

    fn rect(min: &str, max: &str) -> RoomRect {
        RoomRect {
            min: room(min),
            max: room(max),
        }
    }

    #[test]
    fn live_rooms_are_the_map_centre_then_room_controls() {
        let map = Interest {
            stats: Some(rect("W5N5", "W1N1")),
            terrain: rooms(&["W3N3", "W2N3"]),
            minimaps: rooms(&["W3N3"]),
            detail: rooms(&["W3N3", "W2N3", "W4N3"]),
        };
        let control = Interest {
            stats: None,
            terrain: rooms(&["W9N9"]),
            minimaps: rooms(&["W9N9"]),
            detail: rooms(&["W9N9"]),
        };
        let same = Interest {
            detail: rooms(&["W3N3"]),
            ..Interest::default()
        };
        let merged = merge(&[
            (Reach::Room(room("W9N9")), &control),
            (Reach::World, &map),
            (Reach::Room(room("W3N3")), &same),
        ]);
        assert_eq!(merged.stats, Some(rect("W5N5", "W1N1")));
        assert_eq!(merged.terrain, rooms(&["W3N3", "W2N3", "W9N9"]));
        assert_eq!(merged.minimaps, rooms(&["W3N3", "W9N9"]));
        assert_eq!(merged.detail, rooms(&["W3N3", "W9N9"]));
    }

    #[test]
    fn room_controls_alone_ask_for_their_overviews() {
        let a = Interest {
            detail: rooms(&["W1N1"]),
            ..Interest::default()
        };
        let b = Interest {
            detail: rooms(&["E1S1"]),
            ..Interest::default()
        };
        let merged = merge(&[
            (Reach::Room(room("W1N1")), &a),
            (Reach::Room(room("E1S1")), &b),
        ]);
        assert_eq!(merged.stats, Some(rect("W1N1", "E1S1")));
        assert_eq!(merged.detail, rooms(&["W1N1", "E1S1"]));
        assert_eq!(merge(&[]), Interest::default());
    }
}
