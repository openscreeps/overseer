//! Room-history chunks and their replay into per-tick room states.

use serde_json::{Map, Value};

use overseer_core::model::socket::RoomEvent;
use overseer_core::model::world::PlatformHistory;
use overseer_core::socket::RoomState;

use crate::Error;

/// One room-history chunk: the room's objects over consecutive ticks, fetched by
/// [`WorldApi::room_history`](crate::api::WorldApi::room_history).
///
/// The first tick of a chunk holds every object; later ticks hold diffs in the format of the
/// socket's `room:` events. [`Self::replay`] applies them exactly as live updates are applied
/// ([`RoomState::apply`]). Ticks without changes may be absent.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomHistory {
    room: String,
    base: u64,
    timestamp: f64,
    /// Object diffs by tick, ascending.
    ticks: Vec<(u64, Map<String, Value>)>,
}

impl RoomHistory {
    /// The chunk of `room` (used when the chunk does not name its room), ticks ordered.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedReply`] when a tick key is not a non-negative integer or the
    /// base tick is negative.
    pub fn new(chunk: PlatformHistory, room: String) -> Result<Self, Error> {
        let base = u64::try_from(chunk.base)
            .map_err(|_| Error::UnexpectedReply("negative room-history base tick"))?;
        let mut ticks = chunk
            .ticks
            .into_iter()
            .map(|(tick, objects)| {
                let tick = tick
                    .parse()
                    .map_err(|_| Error::UnexpectedReply("room-history tick is not a number"))?;
                let diff = objects
                    .into_iter()
                    .map(|(id, object)| {
                        let change = object.map_or(Value::Null, |object| {
                            Value::Object(object.into_iter().collect())
                        });
                        (id, change)
                    })
                    .collect();
                Ok((tick, diff))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        ticks.sort_unstable_by_key(|(tick, _)| *tick);
        Ok(Self {
            room: chunk.room.unwrap_or(room),
            base,
            timestamp: chunk.timestamp,
            ticks,
        })
    }

    /// The room.
    #[must_use]
    pub fn room(&self) -> &str {
        &self.room
    }

    /// The first tick the chunk covers.
    #[must_use]
    pub const fn base(&self) -> u64 {
        self.base
    }

    /// When the chunk was captured, as the server reports it (`timestamp`).
    #[must_use]
    pub const fn timestamp(&self) -> f64 {
        self.timestamp
    }

    /// The ticks with data, ascending.
    pub fn ticks(&self) -> impl Iterator<Item = u64> + '_ {
        self.ticks.iter().map(|(tick, _)| *tick)
    }

    /// Replays the chunk from an empty room: each step applies one tick's diff.
    #[must_use]
    pub fn replay(self) -> HistoryReplay {
        HistoryReplay {
            ticks: self.ticks.into_iter(),
            state: RoomState::default(),
        }
    }
}

/// A [`RoomHistory`] being replayed.
///
/// Step with [`Self::advance`] and read [`Self::state`] to render tick by tick without copies,
/// or iterate to get a copy of the state after every tick.
#[derive(Debug)]
pub struct HistoryReplay {
    ticks: std::vec::IntoIter<(u64, Map<String, Value>)>,
    state: RoomState,
}

impl HistoryReplay {
    /// Applies the next tick's diff; returns the tick, or `None` at the end of the chunk.
    pub fn advance(&mut self) -> Option<u64> {
        let (tick, objects) = self.ticks.next()?;
        self.state.apply(RoomEvent {
            objects,
            game_time: Some(tick),
            ..RoomEvent::default()
        });
        Some(tick)
    }

    /// The room after the last applied tick ([`RoomState::game_time`] is that tick).
    #[must_use]
    pub const fn state(&self) -> &RoomState {
        &self.state
    }
}

impl Iterator for HistoryReplay {
    type Item = (u64, RoomState);

    fn next(&mut self) -> Option<Self::Item> {
        let tick = self.advance()?;
        Some((tick, self.state.clone()))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.ticks.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::RoomHistory;

    fn history(json: &str) -> Result<RoomHistory, crate::Error> {
        RoomHistory::new(serde_json::from_str(json).unwrap(), "W5N5".to_owned())
    }

    #[test]
    fn ticks_replay_in_numeric_order_from_an_empty_room() {
        let history = history(
            r#"{"base":90,"timestamp":5,"ticks":{
                "100":{"a":{"hits":7}},
                "99":{"a":{"_id":"a","type":"road","x":1,"y":1,"room":"W5N5","hits":9}},
                "101":{"a":null}}}"#,
        )
        .unwrap();
        assert_eq!(history.room(), "W5N5");
        assert_eq!(history.ticks().collect::<Vec<_>>(), [99, 100, 101]);
        let mut replay = history.replay();
        assert_eq!(replay.state().objects().count(), 0);
        assert_eq!(replay.advance(), Some(99));
        assert_eq!(replay.state().object("a").unwrap()["hits"], 9);
        assert_eq!(replay.advance(), Some(100));
        assert_eq!(replay.state().object("a").unwrap()["hits"], 7);
        assert_eq!(replay.state().object("a").unwrap()["type"], "road");
        assert_eq!(replay.state().game_time(), Some(100));
        assert_eq!(replay.advance(), Some(101));
        assert!(replay.state().object("a").is_none());
        assert_eq!(replay.advance(), None);
    }

    #[test]
    fn malformed_ticks_are_rejected() {
        let error = history(r#"{"base":0,"timestamp":0,"ticks":{"x":{}}}"#).unwrap_err();
        assert!(
            matches!(error, crate::Error::UnexpectedReply(_)),
            "{error:?}"
        );
        let error = history(r#"{"base":-1,"timestamp":0,"ticks":{}}"#).unwrap_err();
        assert!(
            matches!(error, crate::Error::UnexpectedReply(_)),
            "{error:?}"
        );
    }
}
