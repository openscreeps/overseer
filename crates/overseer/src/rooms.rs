//! Live rooms: typed snapshots of watched rooms, room information over HTTP and the actions of
//! the room screen (flags, construction sites, intents, the first spawn).

mod actions;
mod info;

pub use actions::{GlobalIntent, ObjectIntent, RoomIntent};
pub use info::StatInterval;
pub use overseer_net::model::world::FlagColor;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use overseer_net::core::socket::{Channel, Flag, FlagError, Payload, RoomState};
use overseer_net::model::world::{GameRoomObject, RoomUser, RoomVisualItem, Terrain};
use tokio::sync::watch;

use crate::live::{Command, Sink, Watch};
use crate::{Error, Overseer};

/// Prefix of the [`Rooms::barrier`] channels' room names, which no room has.
const BARRIER_PREFIX: &str = "overseer-barrier-";

/// Serial number of the next [`Rooms::barrier`], unique in the process: a barrier never shares
/// a channel (or its latest value) with another.
static NEXT_BARRIER: AtomicU64 = AtomicU64::new(0);

/// A watched room as a renderer reads it, built once per update and shared by every
/// [`Watch`] of the room.
#[derive(Clone, Debug)]
pub struct RoomSnapshot {
    /// The room name.
    pub room: String,
    /// The room's shard; `None` on single-world servers.
    pub shard: Option<String>,
    /// Game tick of the latest update; `None` before the first one (a snapshot that only
    /// reports a [`Self::subscription_error`]).
    pub game_time: Option<u64>,
    /// The decoded terrain.
    pub terrain: Arc<Terrain>,
    /// Every object matching the reference, typed by its `type`, in no particular order.
    pub objects: Vec<GameRoomObject>,
    /// `(object id, decoding error)` of the objects that do not match the reference.
    pub undecoded: Vec<(String, String)>,
    /// The users referenced by the room's objects, by user id.
    pub users: BTreeMap<String, RoomUser>,
    /// The signed-in user's flags in the room (empty when the flag list does not decode; that
    /// is reported as [`Notice::Undecodable`](crate::Notice::Undecodable)).
    pub flags: Vec<Flag>,
    /// The signed-in user's room visuals of the previous tick; lines that do not match the
    /// reference are skipped.
    pub visuals: Vec<RoomVisualItem>,
    /// Why the server refused the subscription (`err@room`, for example
    /// `subscribe limit reached`); cleared by the next update.
    pub subscription_error: Option<String>,
}

impl RoomSnapshot {
    /// A snapshot of the live `state`, and the error of a flag list that does not decode.
    pub(crate) fn build(
        room: &str,
        shard: Option<&str>,
        terrain: Arc<Terrain>,
        state: &RoomState,
    ) -> (Self, Option<FlagError>) {
        let mut objects = Vec::new();
        let mut undecoded = Vec::new();
        for (id, object) in state.typed_objects() {
            match object {
                Ok(object) => objects.push(object),
                Err(error) => undecoded.push((id.to_owned(), error.to_string())),
            }
        }
        let (flags, flag_error) = match state.flags() {
            Ok(flags) => (flags, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let snapshot = Self {
            room: room.to_owned(),
            shard: shard.map(str::to_owned),
            game_time: state.game_time(),
            terrain,
            objects,
            undecoded,
            users: state
                .users()
                .map(|(id, user)| (id.to_owned(), user.clone()))
                .collect(),
            flags,
            visuals: state.visuals().filter_map(Result::ok).collect(),
            subscription_error: None,
        };
        (snapshot, flag_error)
    }

    /// A snapshot without live state that reports a refused subscription.
    fn refused(room: &str, shard: Option<&str>, terrain: Arc<Terrain>, error: String) -> Self {
        Self {
            room: room.to_owned(),
            shard: shard.map(str::to_owned),
            game_time: None,
            terrain,
            objects: Vec::new(),
            undecoded: Vec::new(),
            users: BTreeMap::new(),
            flags: Vec::new(),
            visuals: Vec::new(),
            subscription_error: Some(error),
        }
    }
}

/// Live rooms; created by [`Overseer::rooms`].
#[derive(Clone, Debug)]
pub struct Rooms {
    overseer: Overseer,
}

impl Rooms {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// Watches `room` on the default shard: fetches its terrain (once per session, cached by
    /// the client) and subscribes to its updates. Every [`Watch`] of a room shares one
    /// subscription and one snapshot per update; a new watch of a room already watched starts
    /// with the latest snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user (the server only lets signed-in
    /// connections subscribe to rooms), [`Error::Closed`] after
    /// [`Overseer::shutdown`](crate::Overseer::shutdown) and [`Error::Net`] when the terrain
    /// cannot be fetched.
    pub async fn watch(&self, room: &str) -> Result<Watch<RoomSnapshot>, Error> {
        self.watch_shard(None, room).await
    }

    /// Watches `room` on `shard` (ignored by single-world servers); see [`Self::watch`].
    ///
    /// # Errors
    ///
    /// As [`Self::watch`].
    pub async fn watch_on(&self, shard: &str, room: &str) -> Result<Watch<RoomSnapshot>, Error> {
        self.watch_shard(Some(shard), room).await
    }

    /// Subscribes first, then fetches the terrain: the server starts sending the room while
    /// the terrain is on its way.
    async fn watch_shard(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> Result<Watch<RoomSnapshot>, Error> {
        let shard = self.overseer.resolve_shard(shard);
        let watch = self.watch_room(shard.as_deref(), room, None)?;
        let terrain = self
            .overseer
            .client()
            .terrain(shard.as_deref(), room)
            .await?;
        self.set_terrain(shard.as_deref(), room, terrain);
        Ok(watch)
    }

    /// Watches `room` on `shard` (`None`: the default shard): subscribes without any request.
    /// Its updates become snapshots once the room has a terrain: `terrain` when the caller
    /// has it, else the first given to [`Self::set_terrain`]; until then the server's state is
    /// kept and nothing is published. A room already watched keeps the terrain it has.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user and [`Error::Closed`] after
    /// [`Overseer::shutdown`](crate::Overseer::shutdown).
    pub(crate) fn watch_room(
        &self,
        shard: Option<&str>,
        room: &str,
        terrain: Option<Arc<Terrain>>,
    ) -> Result<Watch<RoomSnapshot>, Error> {
        self.overseer.ensure_open()?;
        self.overseer.require_user()?;
        let channel = Channel::Room {
            shard: self.overseer.resolve_shard(shard),
            room: room.to_owned(),
        };
        let (registration, receiver) = self.overseer.registry().acquire(channel, |entry| {
            if let Some(sink) = entry.sink::<RoomSink>() {
                let mut receiver = sink.sender.subscribe();
                if receiver.borrow().is_some() {
                    receiver.mark_changed();
                }
                receiver
            } else {
                let (sender, receiver) = watch::channel(None);
                entry.sinks.push(Box::new(RoomSink {
                    sender,
                    terrain: None,
                    refused: None,
                }));
                receiver
            }
        });
        let receiver = receiver.ok_or(Error::Closed)?;
        let watch = Watch::new(receiver, registration);
        if let Some(terrain) = terrain {
            self.set_terrain(shard, room, terrain);
        }
        Ok(watch)
    }

    /// Gives the terrain of `room` on `shard` (`None`: the default shard) to its
    /// subscription, when watched without one: the state and refusal the server sent
    /// meanwhile are published at once.
    pub(crate) fn set_terrain(&self, shard: Option<&str>, room: &str, terrain: Arc<Terrain>) {
        let channel = Channel::Room {
            shard: self.overseer.resolve_shard(shard),
            room: room.to_owned(),
        };
        let given = self
            .overseer
            .registry()
            .with_entry(&channel, |entry| {
                entry.sink::<RoomSink>().is_some_and(|sink| {
                    sink.terrain.is_none() && {
                        sink.terrain = Some(terrain);
                        true
                    }
                })
            })
            .unwrap_or(false);
        if given {
            self.overseer
                .registry()
                .command(Command::Republish(channel));
        }
    }

    /// A barrier behind every subscription and release sent so far on `shard` (`None`: the
    /// default shard): yields once the server has processed them. Dropping it unsubscribes.
    ///
    /// It subscribes to the `roomMap2` view of a room that does not exist, under a name unique
    /// in the process. The server handles a connection's commands in order and answers a new
    /// `roomMap2` subscription at once, with `{}` for such a room (checked on screeps.com and
    /// `OpenScreeps`), and writes its replies and updates in order: its first value comes after
    /// the server processed everything sent before it, and whatever arrives after that value
    /// was sent after that. The server keeps sending the view each update; only the first
    /// value matters.
    pub(crate) fn barrier(&self, shard: Option<&str>) -> Watch<()> {
        let serial = NEXT_BARRIER.fetch_add(1, Ordering::Relaxed);
        let channel = Channel::RoomMap {
            shard: self.overseer.resolve_shard(shard),
            room: format!("{BARRIER_PREFIX}{serial}"),
        };
        self.overseer.watch(channel, |payload| {
            matches!(payload, Payload::RoomMap(_)).then_some(())
        })
    }
}

/// The room channel's one sink: the snapshot sender of every watch and the room's terrain.
pub(crate) struct RoomSink {
    sender: watch::Sender<Option<Arc<RoomSnapshot>>>,
    /// The terrain snapshots are built with; `None` until [`Rooms::set_terrain`] gives it.
    terrain: Option<Arc<Terrain>>,
    /// A refusal that arrived before the terrain, reported once it is there.
    refused: Option<String>,
}

impl RoomSink {
    /// The terrain snapshots are built with, once there.
    pub(crate) fn terrain(&self) -> Option<Arc<Terrain>> {
        self.terrain.clone()
    }

    /// Publishes the snapshot of an update.
    pub(crate) fn publish(&self, snapshot: RoomSnapshot) {
        self.sender.send_replace(Some(Arc::new(snapshot)));
    }

    /// Records a refused subscription (`err@room`) in the latest snapshot, or in an empty one
    /// before the first update; before the terrain is there, holds it until then.
    pub(crate) fn refuse(&mut self, channel: &Channel, error: String) {
        let Channel::Room { shard, room } = channel else {
            return;
        };
        let Some(terrain) = self.terrain() else {
            self.refused = Some(error);
            return;
        };
        let latest = self.sender.borrow().clone();
        let snapshot = match latest {
            Some(latest) => RoomSnapshot {
                subscription_error: Some(error),
                ..RoomSnapshot::clone(&latest)
            },
            None => RoomSnapshot::refused(room, shard.as_deref(), terrain, error),
        };
        self.publish(snapshot);
    }

    /// Reports the refusal held until the terrain was there, if any.
    pub(crate) fn refuse_held(&mut self, channel: &Channel) {
        if let Some(error) = self.refused.take() {
            self.refuse(channel, error);
        }
    }
}

impl Sink for RoomSink {
    /// Room events carry no data: the background task builds snapshots from the socket's room
    /// state and [publishes](RoomSink::publish) them.
    fn deliver(&mut self, _payload: &Payload) {}

    fn is_unused(&self) -> bool {
        self.sender.receiver_count() == 0
    }
}
