//! Watched rooms: decoded terrain together with the live room state, for rendering.

use std::sync::Arc;

use overseer_core::model::world::{GameRoomObject, RoomUser, RoomVisualItem, Terrain, Tile};
use overseer_core::socket::{Channel, Flag, FlagError, RoomState};

use super::Socket;
use crate::Error;

impl Socket {
    /// Subscribes to `room` on the default shard and fetches its terrain (once per client, see
    /// [`Client::terrain`](crate::Client::terrain)); read both with [`Self::room_view`].
    ///
    /// The socket is not polled while the terrain is fetched; frames wait in the connection.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::terrain`](crate::Client::terrain); the room is not
    /// subscribed then.
    pub async fn watch_room(&mut self, room: &str) -> Result<Channel, Error> {
        self.watch(None, room).await
    }

    /// Subscribes to `room` on `shard` and fetches its terrain; read both with
    /// [`Self::room_view_on`].
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::terrain`](crate::Client::terrain); the room is not
    /// subscribed then.
    pub async fn watch_room_on(&mut self, shard: &str, room: &str) -> Result<Channel, Error> {
        self.watch(Some(shard), room).await
    }

    /// The watched `room` on the default shard: its terrain and, once the first update arrived
    /// on the current connection, its live state. `None` unless the room was opened with
    /// [`Self::watch_room`].
    #[must_use]
    pub fn room_view(&self, room: &str) -> Option<RoomView<'_>> {
        self.view(&self.room_channel(None, room))
    }

    /// The watched `room` on `shard`; see [`Self::room_view`].
    #[must_use]
    pub fn room_view_on(&self, shard: &str, room: &str) -> Option<RoomView<'_>> {
        self.view(&self.room_channel(Some(shard), room))
    }

    async fn watch(&mut self, shard: Option<&str>, room: &str) -> Result<Channel, Error> {
        let shard = self.world.resolve_shard(shard);
        let terrain = self.client.terrain(shard.as_deref(), room).await?;
        let channel = Channel::Room {
            shard,
            room: room.to_owned(),
        };
        self.terrain.insert(channel.clone(), terrain);
        Ok(self.subscribe(channel))
    }

    fn view(&self, channel: &Channel) -> Option<RoomView<'_>> {
        let (channel, terrain) = self.terrain.get_key_value(channel)?;
        let Channel::Room { room, .. } = channel else {
            return None;
        };
        Some(RoomView {
            room,
            terrain,
            state: self.session.room(channel),
        })
    }
}

/// A watched room as a renderer reads it: terrain, typed objects, users, visuals and flags.
///
/// Borrowed from the [`Socket`] by [`Socket::room_view`]; take a new view after each event.
/// Before the room's first update arrives (and after a reconnect, until the next one) only the
/// terrain is known and the other readers are empty.
#[derive(Clone, Copy, Debug)]
pub struct RoomView<'a> {
    room: &'a str,
    terrain: &'a Arc<Terrain>,
    state: Option<&'a RoomState>,
}

impl<'a> RoomView<'a> {
    /// The room name.
    #[must_use]
    pub const fn room(&self) -> &'a str {
        self.room
    }

    /// The decoded terrain.
    #[must_use]
    pub fn terrain(&self) -> &'a Terrain {
        self.terrain
    }

    /// The terrain at `(x, y)`, or `None` outside `0..50`.
    #[must_use]
    pub fn tile(&self, x: u8, y: u8) -> Option<Tile> {
        self.terrain.get(x, y)
    }

    /// The live room state, once its first update arrived on the current connection.
    #[must_use]
    pub const fn state(&self) -> Option<&'a RoomState> {
        self.state
    }

    /// Game tick of the latest update.
    #[must_use]
    pub fn game_time(&self) -> Option<u64> {
        self.state.and_then(RoomState::game_time)
    }

    /// Every object as `(id, typed object)`, in no particular order (see
    /// [`RoomState::typed_object`]).
    pub fn objects(
        &self,
    ) -> impl Iterator<Item = (&'a str, Result<GameRoomObject, serde_json::Error>)> + 'a {
        self.state.into_iter().flat_map(RoomState::typed_objects)
    }

    /// The users referenced by the room's objects as `(user id, user)`.
    pub fn users(&self) -> impl Iterator<Item = (&'a str, &'a RoomUser)> + 'a {
        self.state.into_iter().flat_map(RoomState::users)
    }

    /// The subscribing user's room visuals of the previous tick, parsed item by item.
    pub fn visuals(&self) -> impl Iterator<Item = Result<RoomVisualItem, serde_json::Error>> + 'a {
        self.state.into_iter().flat_map(RoomState::visuals)
    }

    /// The subscribing user's flags in the room.
    ///
    /// # Errors
    ///
    /// Returns the [`FlagError`] of a malformed flag entry.
    pub fn flags(&self) -> Result<Vec<Flag>, FlagError> {
        self.state.map_or_else(|| Ok(Vec::new()), RoomState::flags)
    }
}
