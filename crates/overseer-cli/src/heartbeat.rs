//! The heartbeat that the bot writes to `Memory.__overseer` every tick. The
//! heartbeat counts the footholds that the bot still has on its shard.
//!
//! A reset of a `lost` account needs a fresh heartbeat without footholds on
//! each lost shard. All other results keep the account as it is: no
//! heartbeat, an old heartbeat, or a heartbeat that does not parse.
//!
//! ```json
//! {"version": 1, "tick": 123, "shard": "shard3", "rooms": 0, "creeps": 0,
//!  "spawns": 0, "sites": 0, "powerCreeps": 0}
//! ```

use overseer::model::memory::MemoryValue;
use serde::Deserialize;

/// The Memory path of the heartbeat.
pub(crate) const PATH: &str = "__overseer";
/// The version of the heartbeat that this CLI reads.
const VERSION: u64 = 1;
/// The maximum number of ticks that a heartbeat can be behind the game time.
pub(crate) const MAX_AGE: u64 = 3;

/// A heartbeat of the bot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Heartbeat {
    /// The version of the format.
    pub(crate) version: u64,
    /// The game time when the bot wrote it.
    pub(crate) tick: u64,
    /// The shard of the bot.
    pub(crate) shard: String,
    /// The number of rooms whose controller the bot owns.
    pub(crate) rooms: u64,
    /// The number of creeps of the bot.
    pub(crate) creeps: u64,
    /// The number of spawns of the bot.
    pub(crate) spawns: u64,
    /// The number of construction sites of the bot.
    pub(crate) sites: u64,
    /// The number of power creeps of the bot in the world.
    pub(crate) power_creeps: u64,
}

impl Heartbeat {
    /// Decodes the heartbeat from a Memory value that the `overseer` Memory
    /// API returns.
    ///
    /// # Errors
    ///
    /// Returns `Unsafe::Missing` when there is no heartbeat. Returns
    /// `Unsafe::Malformed` when the heartbeat does not parse.
    pub(crate) fn decode(data: MemoryValue) -> Result<Self, Unsafe> {
        match data {
            MemoryValue::Undefined
            | MemoryValue::IncorrectPath
            | MemoryValue::Json(serde_json::Value::Null) => Err(Unsafe::Missing),
            MemoryValue::Json(serde_json::Value::String(text)) => match text.trim() {
                "" | "undefined" | "null" => Err(Unsafe::Missing),
                text => {
                    serde_json::from_str(text).map_err(|error| Unsafe::Malformed(error.to_string()))
                }
            },
            MemoryValue::Json(value) => {
                serde_json::from_value(value).map_err(|error| Unsafe::Malformed(error.to_string()))
            }
        }
    }

    /// Checks that the heartbeat shows no foothold of the bot on `shard` at
    /// the game time `time`. When `shard` is `None`, the heartbeat can be of
    /// any shard.
    ///
    /// # Errors
    ///
    /// Returns an error when the heartbeat has a different version or shard,
    /// when it is not fresh, or when it counts a foothold.
    pub(crate) fn check(&self, shard: Option<&str>, time: u64) -> Result<(), Unsafe> {
        if self.version != VERSION {
            return Err(Unsafe::Version(self.version));
        }
        if let Some(shard) = shard
            && shard != self.shard
        {
            return Err(Unsafe::Shard(self.shard.clone()));
        }
        let Some(age) = time.checked_sub(self.tick) else {
            return Err(Unsafe::Future {
                tick: self.tick,
                time,
            });
        };
        if age > MAX_AGE {
            return Err(Unsafe::Stale(age));
        }
        let footholds = [
            (self.rooms, "rooms"),
            (self.creeps, "creeps"),
            (self.spawns, "spawns"),
            (self.sites, "construction sites"),
            (self.power_creeps, "power creeps"),
        ];
        let held: Vec<String> = footholds
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, what)| format!("{count} {what}"))
            .collect();
        if held.is_empty() {
            Ok(())
        } else {
            Err(Unsafe::Footholds(held.join(", ")))
        }
    }
}

/// The reason that a heartbeat does not permit a reset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Unsafe {
    /// Memory has no heartbeat.
    #[error("no heartbeat in Memory.{PATH}")]
    Missing,
    /// The heartbeat does not parse.
    #[error("the heartbeat does not parse: {0}")]
    Malformed(String),
    /// The heartbeat has a different version.
    #[error("heartbeat version {0}, not {VERSION}")]
    Version(u64),
    /// The heartbeat is of a different shard.
    #[error("the heartbeat is of shard {0}")]
    Shard(String),
    /// The heartbeat is ahead of the game time.
    #[error("the heartbeat of tick {tick} is ahead of the game time {time}")]
    Future {
        /// The tick of the heartbeat.
        tick: u64,
        /// The game time.
        time: u64,
    },
    /// The heartbeat is old: the bot does not run.
    #[error("the heartbeat is {0} ticks old (at most {MAX_AGE}): the bot does not run")]
    Stale(u64),
    /// The bot still has footholds.
    #[error("the bot still has {0}")]
    Footholds(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Makes a decoded Memory value. The `overseer` crate handles the
    /// compression.
    fn memory(text: &str) -> MemoryValue {
        MemoryValue::Json(serde_json::from_str(text).expect("JSON fixture"))
    }

    /// A heartbeat without footholds at tick 100 on shard3.
    const EMPTY: &str = r#"{"version":1,"tick":100,"shard":"shard3","rooms":0,"creeps":0,"spawns":0,"sites":0,"powerCreeps":0}"#;

    /// Missing data and malformed fields never permit a reset.
    #[test]
    fn decodes() {
        let heartbeat = Heartbeat::decode(memory(EMPTY)).expect("a heartbeat");
        assert_eq!(heartbeat.tick, 100);
        assert_eq!(
            Heartbeat::decode(MemoryValue::Undefined),
            Err(Unsafe::Missing)
        );
        assert_eq!(Heartbeat::decode(memory("null")), Err(Unsafe::Missing));
        for broken in [
            r#"{"version":1}"#.to_owned(),
            EMPTY.replace("\"creeps\":0", "\"creeps\":-1"),
            EMPTY.replace("\"rooms\":0", "\"rooms\":0.5"),
        ] {
            assert!(
                matches!(
                    Heartbeat::decode(memory(&broken)),
                    Err(Unsafe::Malformed(_))
                ),
                "{broken}"
            );
        }
    }

    /// A reset needs version 1, the correct shard, an age of at most 3
    /// ticks, and no foothold.
    #[test]
    fn checks() {
        let heartbeat = Heartbeat::decode(memory(EMPTY)).expect("a heartbeat");
        assert_eq!(heartbeat.check(Some("shard3"), 100), Ok(()));
        assert_eq!(heartbeat.check(Some("shard3"), 103), Ok(()));
        assert_eq!(heartbeat.check(None, 101), Ok(()));
        assert_eq!(heartbeat.check(Some("shard3"), 104), Err(Unsafe::Stale(4)));
        assert!(matches!(
            heartbeat.check(Some("shard3"), 99),
            Err(Unsafe::Future { .. })
        ));
        assert_eq!(
            heartbeat.check(Some("shard0"), 100),
            Err(Unsafe::Shard("shard3".to_owned()))
        );
        let alive = Heartbeat {
            creeps: 2,
            sites: 1,
            ..heartbeat.clone()
        };
        assert_eq!(
            alive.check(Some("shard3"), 100),
            Err(Unsafe::Footholds(
                "2 creeps, 1 construction sites".to_owned()
            ))
        );
        let future = Heartbeat {
            version: 2,
            ..heartbeat
        };
        assert_eq!(future.check(Some("shard3"), 100), Err(Unsafe::Version(2)));
    }
}
