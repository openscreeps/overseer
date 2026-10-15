//! Room history playback.
//!
//! The server records each room's objects in chunks of
//! [`World::history_chunk_size`](overseer_net::World::history_chunk_size) ticks. A
//! [`HistoryPlayer`] moves through one room's history tick by tick: it loads the chunk holding
//! the tick, replays its diffs from the chunk's base and keeps recent chunks for stepping
//! back and forth.

mod player;

pub use player::{CACHED_CHUNKS, HistoryPlayer, Seek};

use crate::{Error, Overseer};

/// Room history playback.
///
/// Created by [`Overseer::history`]; cheap to clone.
#[derive(Clone, Debug)]
pub struct History {
    overseer: Overseer,
}

impl History {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// A player of the history of `room` on `shard` (`None`: the default shard; ignored by
    /// single-world servers), positioned nowhere until [`HistoryPlayer::seek`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown).
    pub fn player(&self, room: &str, shard: Option<&str>) -> Result<HistoryPlayer, Error> {
        self.overseer.ensure_open()?;
        let shard = self.overseer.resolve_shard(shard);
        Ok(HistoryPlayer::new(
            self.overseer.clone(),
            shard,
            room.to_owned(),
        ))
    }
}
