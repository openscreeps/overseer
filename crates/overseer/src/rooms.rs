//! Live rooms: typed snapshots of watched rooms, room information over HTTP and the actions of
//! the room screen (flags, construction sites, intents, the first spawn).

mod actions;
mod info;

pub use actions::{FlagColor, GlobalIntent, ObjectIntent, RoomIntent};
pub use info::StatInterval;

use std::collections::BTreeMap;
use std::sync::Arc;

use overseer_net::core::api::Terrain;
use overseer_net::core::api::types::GameRoomObject;
use overseer_net::core::socket::types::{RoomUser, RoomVisualItem};
use overseer_net::core::socket::{Channel, Flag, FlagError, Payload, RoomState};
use tokio::sync::watch;

use crate::live::{Sink, Watch};
use crate::{Error, Overseer};

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

    async fn watch_shard(
        &self,
        shard: Option<&str>,
        room: &str,
    ) -> Result<Watch<RoomSnapshot>, Error> {
        self.overseer.ensure_open()?;
        self.overseer.require_user()?;
        let shard = self.overseer.resolve_shard(shard);
        let terrain = self
            .overseer
            .client()
            .terrain(shard.as_deref(), room)
            .await?;
        let channel = Channel::Room {
            shard,
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
                entry.sinks.push(Box::new(RoomSink { sender, terrain }));
                receiver
            }
        });
        let receiver = receiver.ok_or(Error::Closed)?;
        Ok(Watch::new(receiver, registration))
    }
}

/// The room channel's one sink: the snapshot sender of every watch and the room's terrain.
pub(crate) struct RoomSink {
    sender: watch::Sender<Option<Arc<RoomSnapshot>>>,
    terrain: Arc<Terrain>,
}

impl RoomSink {
    /// The terrain snapshots are built with.
    pub(crate) fn terrain(&self) -> Arc<Terrain> {
        Arc::clone(&self.terrain)
    }

    /// Publishes the snapshot of an update.
    pub(crate) fn publish(&self, snapshot: RoomSnapshot) {
        self.sender.send_replace(Some(Arc::new(snapshot)));
    }

    /// Records a refused subscription (`err@room`) in the latest snapshot, or in an empty one
    /// before the first update.
    pub(crate) fn refuse(&self, channel: &Channel, error: String) {
        let Channel::Room { shard, room } = channel else {
            return;
        };
        let latest = self.sender.borrow().clone();
        let snapshot = match latest {
            Some(latest) => RoomSnapshot {
                subscription_error: Some(error),
                ..RoomSnapshot::clone(&latest)
            },
            None => RoomSnapshot::refused(room, shard.as_deref(), self.terrain(), error),
        };
        self.publish(snapshot);
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
