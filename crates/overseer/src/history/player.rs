//! Playback of one room's history.

use std::collections::VecDeque;
use std::sync::Arc;

use overseer_net::core::api::{ApiError, Terrain};
use overseer_net::core::socket::RoomState;
use overseer_net::{HistoryReplay, RoomHistory};

use crate::{Error, Overseer, RoomSnapshot};

/// Chunks a [`HistoryPlayer`] keeps for moving back and forth without fetching them again.
pub const CACHED_CHUNKS: usize = 4;

/// The outcome of moving a [`HistoryPlayer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seek {
    /// The player is at the requested tick.
    Moved,
    /// The server keeps no history for the requested tick (HTTP 404): the room was not active
    /// then, the chunk expired, or it is not written yet (the most recent ticks). The player
    /// did not move.
    NotKept,
}

/// Plays the history of one room; created by [`History::player`](crate::History::player).
///
/// [`Self::seek`] loads the chunk holding a tick (the last [`CACHED_CHUNKS`] chunks are kept)
/// and replays its diffs from the chunk's base up to the tick; [`Self::step`] applies the next
/// tick and [`Self::step_back`] replays the chunk again up to the previous one. Ticks without
/// changes keep the state of the last change.
#[derive(Debug)]
pub struct HistoryPlayer {
    overseer: Overseer,
    shard: Option<String>,
    room: String,
    /// `(requested base tick, chunk)`, least recently used first.
    chunks: VecDeque<(u64, RoomHistory)>,
    terrain: Option<Arc<Terrain>>,
    position: Option<Position>,
}

/// Where a player is: a chunk replayed up to a tick.
#[derive(Debug)]
struct Position {
    /// The requested base tick of the chunk.
    base: u64,
    /// The chunk's ticks with data, ascending.
    ticks: Vec<u64>,
    /// How many of `ticks` the replay applied.
    applied: usize,
    replay: HistoryReplay,
    tick: u64,
}

impl Position {
    fn start(base: u64, chunk: RoomHistory) -> Self {
        Self {
            base,
            ticks: chunk.ticks().collect(),
            applied: 0,
            replay: chunk.replay(),
            tick: base,
        }
    }

    /// Applies every diff up to and including `tick`.
    fn advance_to(&mut self, tick: u64) {
        while self
            .ticks
            .get(self.applied)
            .is_some_and(|&next| next <= tick)
        {
            self.replay.advance();
            self.applied += 1;
        }
        self.tick = tick;
    }
}

impl HistoryPlayer {
    pub(crate) fn new(overseer: Overseer, shard: Option<String>, room: String) -> Self {
        Self {
            overseer,
            shard,
            room,
            chunks: VecDeque::with_capacity(CACHED_CHUNKS),
            terrain: None,
            position: None,
        }
    }

    /// The room.
    #[must_use]
    pub fn room(&self) -> &str {
        &self.room
    }

    /// The room's shard; `None` on single-world servers.
    #[must_use]
    pub fn shard(&self) -> Option<&str> {
        self.shard.as_deref()
    }

    /// The current tick; `None` before the first successful [`Self::seek`].
    #[must_use]
    pub fn tick(&self) -> Option<u64> {
        self.position.as_ref().map(|position| position.tick)
    }

    /// The room at the current tick (raw objects; [`RoomState::game_time`] is the tick of the
    /// last change); `None` before the first successful [`Self::seek`].
    #[must_use]
    pub fn state(&self) -> Option<&RoomState> {
        self.position
            .as_ref()
            .map(|position| position.replay.state())
    }

    /// The room at the current tick in the shape of a live [`RoomSnapshot`]:
    /// [`RoomSnapshot::game_time`] is the current tick; objects, undecoded objects and terrain
    /// are filled; history records no users, flags or visuals, so those are empty.
    /// `None` before the first successful [`Self::seek`].
    #[must_use]
    pub fn snapshot(&self) -> Option<RoomSnapshot> {
        let position = self.position.as_ref()?;
        let terrain = Arc::clone(self.terrain.as_ref()?);
        let state = position.replay.state();
        let (mut snapshot, _no_flags) =
            RoomSnapshot::build(&self.room, self.shard.as_deref(), terrain, state);
        snapshot.game_time = Some(position.tick);
        Some(snapshot)
    }

    /// Moves to `tick`: loads the chunk holding it (from the cache, or
    /// `GET /room-history…`) and replays it up to `tick`, continuing the current replay when
    /// moving forward within a chunk. Also loads the room's terrain once, for
    /// [`Self::snapshot`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] after [`Overseer::shutdown`](crate::Overseer::shutdown) and
    /// [`Error::Net`] when a request fails (other than with HTTP 404, which is
    /// [`Seek::NotKept`]) or the chunk does not decode. The player does not move then.
    pub async fn seek(&mut self, tick: u64) -> Result<Seek, Error> {
        self.overseer.ensure_open()?;
        let size = self.overseer.world().history_chunk_size();
        let base = tick - tick % size;
        let resume = self
            .position
            .as_ref()
            .is_some_and(|position| position.base == base && position.tick <= tick);
        if !resume {
            let Some(chunk) = self.chunk(base).await? else {
                return Ok(Seek::NotKept);
            };
            if self.terrain.is_none() {
                let client = self.overseer.client();
                self.terrain = Some(client.terrain(self.shard.as_deref(), &self.room).await?);
            }
            self.position = Some(Position::start(base, chunk));
        }
        if let Some(position) = &mut self.position {
            position.advance_to(tick);
        }
        Ok(Seek::Moved)
    }

    /// Moves to the tick after the current one, loading the next chunk at a chunk's end.
    ///
    /// # Errors
    ///
    /// As [`Self::seek`]; [`Error::InvalidInput`] before the first successful seek.
    pub async fn step(&mut self) -> Result<Seek, Error> {
        let tick = self.current()?;
        self.seek(tick + 1).await
    }

    /// Moves to the tick before the current one, replaying its chunk from the base.
    ///
    /// # Errors
    ///
    /// As [`Self::seek`]; [`Error::InvalidInput`] before the first successful seek and at
    /// tick 0.
    pub async fn step_back(&mut self) -> Result<Seek, Error> {
        let tick = self
            .current()?
            .checked_sub(1)
            .ok_or_else(|| Error::InvalidInput("there is no tick before 0".to_owned()))?;
        self.seek(tick).await
    }

    fn current(&self) -> Result<u64, Error> {
        self.tick()
            .ok_or_else(|| Error::InvalidInput("the history player has no tick yet".to_owned()))
    }

    /// The chunk starting at `base`, from the cache or the server; `None` when not kept.
    async fn chunk(&mut self, base: u64) -> Result<Option<RoomHistory>, Error> {
        if let Some(index) = self.chunks.iter().position(|(cached, _)| *cached == base)
            && let Some(entry) = self.chunks.remove(index)
        {
            let chunk = entry.1.clone();
            self.chunks.push_back(entry);
            return Ok(Some(chunk));
        }
        let client = self.overseer.client();
        match client
            .room_history(self.shard.as_deref(), &self.room, base)
            .await
        {
            Ok(chunk) => {
                if self.chunks.len() == CACHED_CHUNKS {
                    self.chunks.pop_front();
                }
                self.chunks.push_back((base, chunk.clone()));
                Ok(Some(chunk))
            }
            Err(overseer_net::Error::Api(ApiError::Status { status, .. }))
                if status.as_u16() == 404 =>
            {
                Ok(None)
            }
            Err(error) => Err(error.into()),
        }
    }
}
