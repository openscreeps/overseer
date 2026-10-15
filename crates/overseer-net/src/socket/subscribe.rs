//! Subscription helpers that name channels for the socket's [`World`](crate::World).

use overseer_core::socket::{Channel, RoomState};

use super::Socket;
use crate::Error;

impl Socket {
    /// Subscribes to `channel` (sent by the next [`Self::next`] once the connection is ready, and
    /// again after every reconnect) and returns it.
    pub fn subscribe(&mut self, channel: Channel) -> Channel {
        self.session.subscribe(channel.clone());
        channel
    }

    /// Unsubscribes from `channel` and drops its room state (and its terrain when watched).
    pub fn unsubscribe(&mut self, channel: &Channel) {
        self.session.unsubscribe(channel);
        self.terrain.remove(channel);
    }

    /// Server announcements (`server-message`), the only channel open to anonymous connections.
    pub fn subscribe_server_messages(&mut self) -> Channel {
        self.subscribe(Channel::ServerMessage)
    }

    /// Detailed updates of `room` on the default shard; read them with [`Self::room`].
    pub fn subscribe_room(&mut self, room: &str) -> Channel {
        self.subscribe(self.room_channel(None, room))
    }

    /// Detailed updates of `room` on `shard`; read them with [`Self::room_on`].
    pub fn subscribe_room_on(&mut self, shard: &str, room: &str) -> Channel {
        self.subscribe(self.room_channel(Some(shard), room))
    }

    /// The world-map view of `room` on the default shard.
    pub fn subscribe_room_map(&mut self, room: &str) -> Channel {
        let shard = self.world.resolve_shard(None);
        self.subscribe(Channel::RoomMap {
            shard,
            room: room.to_owned(),
        })
    }

    /// The world-map view of `room` on `shard`.
    pub fn subscribe_room_map_on(&mut self, shard: &str, room: &str) -> Channel {
        let shard = self.world.resolve_shard(Some(shard));
        self.subscribe(Channel::RoomMap {
            shard,
            room: room.to_owned(),
        })
    }

    /// The signed-in user's map visuals on the default shard.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_map_visual(&mut self) -> Result<Channel, Error> {
        self.subscribe_map_visual_shard(None)
    }

    /// The signed-in user's map visuals on `shard`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_map_visual_on(&mut self, shard: &str) -> Result<Channel, Error> {
        self.subscribe_map_visual_shard(Some(shard))
    }

    /// The signed-in user's CPU and Memory usage per run.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_cpu(&mut self) -> Result<Channel, Error> {
        let user = self.user()?;
        Ok(self.subscribe(Channel::Cpu { user }))
    }

    /// The signed-in user's console output and run errors.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_console(&mut self) -> Result<Channel, Error> {
        let user = self.user()?;
        Ok(self.subscribe(Channel::Console { user }))
    }

    /// The string form of the Memory value at the dot-separated `path` on the default shard.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_memory(&mut self, path: &str) -> Result<Channel, Error> {
        self.subscribe_memory_shard(None, path)
    }

    /// The string form of the Memory value at the dot-separated `path` on `shard`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_memory_on(&mut self, shard: &str, path: &str) -> Result<Channel, Error> {
        self.subscribe_memory_shard(Some(shard), path)
    }

    /// The signed-in user's resources and credits.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_resources(&mut self) -> Result<Channel, Error> {
        let user = self.user()?;
        Ok(self.subscribe(Channel::Resources { user }))
    }

    /// The signed-in user's saved code branches.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_code(&mut self) -> Result<Channel, Error> {
        let user = self.user()?;
        Ok(self.subscribe(Channel::Code { user }))
    }

    /// Changes of the signed-in user's active world or simulation branch.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_active_branch(&mut self) -> Result<Channel, Error> {
        let user = self.user()?;
        Ok(self.subscribe(Channel::ActiveBranch { user }))
    }

    /// The signed-in user's incoming private messages.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_new_messages(&mut self) -> Result<Channel, Error> {
        let user = self.user()?;
        Ok(self.subscribe(Channel::NewMessage { user }))
    }

    /// Updates of the signed-in user's conversation with the user id `respondent`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] without a signed-in user.
    pub fn subscribe_conversation(&mut self, respondent: &str) -> Result<Channel, Error> {
        let user = self.user()?;
        Ok(self.subscribe(Channel::Conversation {
            user,
            respondent: respondent.to_owned(),
        }))
    }

    /// State of the subscribed `room` on the default shard, once its first update arrived on the
    /// current connection.
    #[must_use]
    pub fn room(&self, room: &str) -> Option<&RoomState> {
        self.session.room(&self.room_channel(None, room))
    }

    /// State of the subscribed `room` on `shard`, once its first update arrived on the current
    /// connection.
    #[must_use]
    pub fn room_on(&self, shard: &str, room: &str) -> Option<&RoomState> {
        self.session.room(&self.room_channel(Some(shard), room))
    }

    pub(super) fn room_channel(&self, shard: Option<&str>, room: &str) -> Channel {
        Channel::Room {
            shard: self.world.resolve_shard(shard),
            room: room.to_owned(),
        }
    }

    fn subscribe_map_visual_shard(&mut self, shard: Option<&str>) -> Result<Channel, Error> {
        let user = self.user()?;
        let shard = self.world.resolve_shard(shard);
        Ok(self.subscribe(Channel::MapVisual { user, shard }))
    }

    fn subscribe_memory_shard(
        &mut self,
        shard: Option<&str>,
        path: &str,
    ) -> Result<Channel, Error> {
        let user = self.user()?;
        let shard = self.world.resolve_shard(shard);
        Ok(self.subscribe(Channel::Memory {
            user,
            shard,
            path: path.to_owned(),
        }))
    }

    fn user(&self) -> Result<String, Error> {
        self.world.user().map(str::to_owned).ok_or(Error::Anonymous)
    }
}
