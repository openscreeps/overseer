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
//! The task also hands what the world fetched (terrain, rooms without terrain, overviews,
//! scanned portals) to the app's cache and reports the world's loading progress
//! ([`progress_label`] for the status bar, including when the rate limit holds overviews
//! back); after the user lifted the limit, [`Feed::recheck_rate_limit`] resumes them, and
//! [`Feed::set_rate_limit_exempt_until`] lets them load regardless of the budget while lifted.
//!
//! The task ends when the feed and every subscription are dropped or the session closes;
//! dropping the `WorldMap` then ends its subscriptions.

use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::pin::pin;
use std::rc::Rc;

use futures_util::future::{Either, select};
use overseer::map::{Interest, RoomCoord, RoomRect, WorldEvent, WorldMap, WorldProgress};
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
    RecheckRateLimit,
    RateLimitExemptUntil(Option<u64>),
}

/// A running world; cheap to clone.
#[derive(Clone)]
pub struct Feed {
    commands: mpsc::UnboundedSender<Command>,
    next_id: Rc<Cell<u64>>,
    bounds: RoomRect,
}

impl Feed {
    /// Runs `world` on the browser's event loop; `on_event` sees every event first (to cache
    /// what was fetched), `on_progress` every change of the world's loading progress.
    pub fn start(
        world: WorldMap,
        on_event: impl Fn(&WorldEvent) + 'static,
        on_progress: impl Fn(WorldProgress) + 'static,
    ) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let bounds = world.bounds();
        wasm_bindgen_futures::spawn_local(run(world, receiver, on_event, on_progress));
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

    /// After the user lifted the rate limit: resumes overview loading paused by it
    /// ([`WorldMap::recheck_rate_limit`]); nothing happens while it is not paused.
    pub fn recheck_rate_limit(&self) {
        // A send fails only once the task ended.
        let _ = self.commands.send(Command::RecheckRateLimit);
    }

    /// The token's rate limits are lifted until `until` (milliseconds since the Unix epoch),
    /// or not (`None`): see [`WorldMap::set_rate_limit_exempt_until`].
    pub fn set_rate_limit_exempt_until(&self, until: Option<u64>) {
        // A send fails only once the task ended.
        let _ = self.commands.send(Command::RateLimitExemptUntil(until));
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
    on_event: impl Fn(&WorldEvent),
    on_progress: impl Fn(WorldProgress),
) {
    let mut sinks: BTreeMap<u64, Sink> = BTreeMap::new();
    let mut progress = world.progress();
    on_progress(progress);
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
                on_event(&event);
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
                    Command::RecheckRateLimit => {
                        world.recheck_rate_limit();
                    }
                    Command::RateLimitExemptUntil(until) => {
                        world.set_rate_limit_exempt_until(until);
                    }
                }
                let interests: Vec<(Reach, &Interest)> = sinks
                    .values()
                    .map(|sink| (sink.reach, &sink.interest))
                    .collect();
                world.set_interest(&merge(&interests));
            }
        }
        let now = world.progress();
        if now != progress {
            progress = now;
            on_progress(progress);
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
        } else if world.has_no_terrain(room) {
            deliver(&WorldEvent::NoTerrain(room));
        }
        if let Some(stat) = world.stat(room) {
            stats.push((room, stat));
        }
        if let Some(portals) = world.portals(room) {
            deliver(&WorldEvent::Portals(room, portals));
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
///   rooms; portals likewise;
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
    let rect = |pick: fn(&Interest) -> Option<RoomRect>| {
        world().filter_map(pick).reduce(union).or_else(|| {
            interests
                .iter()
                .filter_map(|(reach, interest)| match reach {
                    Reach::Room(room) => Some(pick(interest).unwrap_or(RoomRect {
                        min: *room,
                        max: *room,
                    })),
                    Reach::World => None,
                })
                .reduce(union)
        })
    };
    let mut merged = Interest {
        stats: rect(|interest| interest.stats),
        portals: rect(|interest| interest.portals),
        ..Interest::default()
    };
    let (mut terrain, mut minimaps, mut detail) =
        (Unique::default(), Unique::default(), Unique::default());
    for interest in world().chain(rooms()) {
        terrain.extend(&mut merged.terrain, &interest.terrain);
        minimaps.extend(&mut merged.minimaps, &interest.minimaps);
    }
    for interest in world() {
        detail.extend(
            &mut merged.detail,
            interest.detail.get(..1).unwrap_or_default(),
        );
    }
    for interest in rooms() {
        detail.extend(&mut merged.detail, &interest.detail);
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

/// The rooms already in a list: [`Interest::terrain`] may list every room of a shard.
#[derive(Default)]
struct Unique(HashSet<RoomCoord>);

impl Unique {
    /// Appends the rooms of `more` that `rooms` lacks.
    fn extend(&mut self, rooms: &mut Vec<RoomCoord>, more: &[RoomCoord]) {
        rooms.extend(more.iter().filter(|room| self.0.insert(**room)));
    }
}

/// The status bar's loading progress at `now` (milliseconds since the Unix epoch), such as
/// `Terrain 12 345 / 25 000 · Owners 2 500 / 25 000 · Portals 40 / 331`, with
/// `waiting for the rate limit, resumes in 12 min` while the rate limit holds overviews back;
/// `None` once everything wanted is known.
pub fn progress_label(progress: &WorldProgress, now: u64) -> Option<String> {
    if progress.is_complete() {
        return None;
    }
    let terrain = (progress.terrain_wanted > 0).then(|| {
        let done = progress.terrain_known + progress.terrain_absent;
        format!(
            "Terrain {} / {}",
            grouped(done.min(progress.terrain_wanted)),
            grouped(progress.terrain_wanted)
        )
    });
    let owners = (progress.stats_wanted > 0).then(|| {
        format!(
            "Owners {} / {}",
            grouped(progress.stats_known.min(progress.stats_wanted)),
            grouped(progress.stats_wanted)
        )
    });
    let portals = (progress.portals_wanted > 0).then(|| {
        format!(
            "Portals {} / {}",
            grouped(progress.portals_scanned.min(progress.portals_wanted)),
            grouped(progress.portals_wanted)
        )
    });
    let waiting = progress
        .stats_paused_until
        .map(|until| crate::rate_limits::waiting_label(until, now));
    Some(
        [terrain, owners, portals, waiting]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

/// `count` with its thousands separated by narrow no-break spaces (`12 345`).
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() * 2);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('\u{202f}');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use overseer::map::{Interest, RoomCoord, RoomRect, WorldProgress};

    use super::{Reach, merge, progress_label};

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
            portals: Some(rect("W5N5", "W1N1")),
        };
        let control = Interest {
            stats: None,
            terrain: rooms(&["W9N9"]),
            minimaps: rooms(&["W9N9"]),
            detail: rooms(&["W9N9"]),
            portals: Some(rect("W9N9", "W9N9")),
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
        assert_eq!(merged.portals, Some(rect("W5N5", "W1N1")));
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
        // Portals likewise: around the room controls' rooms when the map wants none.
        assert_eq!(merged.portals, Some(rect("W1N1", "E1S1")));
        assert_eq!(merged.detail, rooms(&["W1N1", "E1S1"]));
        assert_eq!(merge(&[]), Interest::default());
    }

    #[test]
    fn whole_shards_merge_once_per_room() {
        let shard = rect("W90N90", "E90S90");
        let all = shard.iter().collect::<Vec<_>>();
        let map = Interest {
            stats: Some(shard),
            terrain: all.clone(),
            ..Interest::default()
        };
        let control = Interest {
            terrain: rooms(&["W1N1", "W200N1"]),
            ..Interest::default()
        };
        let merged = merge(&[(Reach::World, &map), (Reach::Room(room("W1N1")), &control)]);
        assert_eq!(merged.terrain.len(), all.len() + 1);
        assert_eq!(merged.terrain[..all.len()], all[..]);
        assert_eq!(merged.terrain[all.len()], room("W200N1"));
    }

    #[test]
    fn progress_shows_until_everything_is_known() {
        let now = 1_700_000_000_000;
        let loading = WorldProgress {
            terrain_known: 12_000,
            terrain_absent: 345,
            terrain_wanted: 25_000,
            stats_known: 2_500,
            stats_wanted: 25_000,
            stats_paused_until: None,
            portals_scanned: 0,
            portals_wanted: 0,
        };
        assert_eq!(
            progress_label(&loading, now).as_deref(),
            Some("Terrain 12\u{202f}345 / 25\u{202f}000 · Owners 2\u{202f}500 / 25\u{202f}000")
        );
        let terrain_only = WorldProgress {
            stats_known: 0,
            stats_wanted: 0,
            ..loading
        };
        assert_eq!(
            progress_label(&terrain_only, now).as_deref(),
            Some("Terrain 12\u{202f}345 / 25\u{202f}000")
        );
        let owners_left = WorldProgress {
            terrain_known: 24_655,
            ..loading
        };
        assert_eq!(
            progress_label(&owners_left, now).as_deref(),
            Some("Terrain 25\u{202f}000 / 25\u{202f}000 · Owners 2\u{202f}500 / 25\u{202f}000")
        );
        let done = WorldProgress {
            stats_known: 25_000,
            ..owners_left
        };
        assert_eq!(progress_label(&done, now), None);
        let portals = WorldProgress {
            portals_scanned: 40,
            portals_wanted: 331,
            ..done
        };
        assert_eq!(
            progress_label(&portals, now).as_deref(),
            Some(
                "Terrain 25\u{202f}000 / 25\u{202f}000 · Owners 25\u{202f}000 / 25\u{202f}000 · Portals 40 / 331"
            )
        );
        assert_eq!(progress_label(&WorldProgress::default(), now), None);
        let small = WorldProgress {
            terrain_known: 7,
            terrain_wanted: 999,
            ..WorldProgress::default()
        };
        assert_eq!(
            progress_label(&small, now).as_deref(),
            Some("Terrain 7 / 999")
        );
    }

    #[test]
    fn paused_overviews_say_when_they_resume_as_time_passes() {
        let now = 1_700_000_000_000;
        let paused = WorldProgress {
            stats_known: 2_500,
            stats_wanted: 33_124,
            stats_paused_until: Some(now + 42 * 60_000),
            ..WorldProgress::default()
        };
        assert_eq!(
            progress_label(&paused, now).as_deref(),
            Some(
                "Owners 2\u{202f}500 / 33\u{202f}124 · waiting for the rate limit, resumes in 42 min"
            )
        );
        assert_eq!(
            progress_label(&paused, now + 41 * 60_000 + 30_000).as_deref(),
            Some(
                "Owners 2\u{202f}500 / 33\u{202f}124 · waiting for the rate limit, resumes in 30 s"
            )
        );
        assert_eq!(
            progress_label(&paused, now + 42 * 60_000).as_deref(),
            Some("Owners 2\u{202f}500 / 33\u{202f}124 · waiting for the rate limit, resuming")
        );
        // Everything known: nothing to wait for.
        let known = WorldProgress {
            stats_known: 33_124,
            ..paused
        };
        assert_eq!(progress_label(&known, now), None);
    }
}
