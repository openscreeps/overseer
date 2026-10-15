//! The subscribing user's flags in a room, as `room:` events send them.

use std::str::FromStr;

/// One flag of the flags string of a room ([`RoomState::flags`](super::RoomState::flags)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Flag {
    /// Flag name (unique per user).
    pub name: String,
    /// Primary colour (`COLOR_*`, 1 to 10).
    pub color: u8,
    /// Secondary colour (`COLOR_*`, 1 to 10).
    pub secondary_color: u8,
    /// Room x coordinate.
    pub x: u64,
    /// Room y coordinate.
    pub y: u64,
}

/// A flags string entry that is not `name~color~secondaryColor~x~y`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid flag entry {entry:?}: {problem}")]
pub struct FlagError {
    /// The entry as sent.
    pub entry: String,
    /// What is wrong with it.
    pub problem: &'static str,
}

impl Flag {
    /// Parses a flags string: entries joined by `|`, each `name~color~secondaryColor~x~y`, with
    /// `|` and `~` inside a name written as `$VLINE$` and `$TILDE$`. Empty entries are skipped,
    /// as the engine skips them.
    ///
    /// # Errors
    ///
    /// Returns the first entry that is not five `~`-separated fields with numeric colours and
    /// coordinates.
    pub fn parse_all(flags: &str) -> Result<Vec<Self>, FlagError> {
        flags
            .split('|')
            .filter(|entry| !entry.is_empty())
            .map(str::parse)
            .collect()
    }
}

impl FromStr for Flag {
    type Err = FlagError;

    /// Parses one `name~color~secondaryColor~x~y` entry.
    fn from_str(entry: &str) -> Result<Self, FlagError> {
        let error = |problem| FlagError {
            entry: entry.to_owned(),
            problem,
        };
        let fields: Vec<&str> = entry.split('~').collect();
        let [name, color, secondary_color, x, y] = fields.as_slice() else {
            return Err(error("expected 5 `~`-separated fields"));
        };
        Ok(Self {
            // The engine unescapes in this order.
            name: name.replace("$VLINE$", "|").replace("$TILDE$", "~"),
            color: color.parse().map_err(|_| error("invalid color"))?,
            secondary_color: secondary_color
                .parse()
                .map_err(|_| error("invalid secondary color"))?,
            x: x.parse().map_err(|_| error("invalid x"))?,
            y: y.parse().map_err(|_| error("invalid y"))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Flag;

    fn flag(name: &str, color: u8, secondary_color: u8, x: u64, y: u64) -> Flag {
        Flag {
            name: name.to_owned(),
            color,
            secondary_color,
            x,
            y,
        }
    }

    #[test]
    fn entries_split_on_bars_and_names_unescape() {
        let flags = Flag::parse_all("Flag1~1~2~10~20|a$VLINE$b$TILDE$c~10~10~0~49||").unwrap();
        assert_eq!(
            flags,
            [flag("Flag1", 1, 2, 10, 20), flag("a|b~c", 10, 10, 0, 49)]
        );
        assert_eq!(Flag::parse_all("").unwrap(), []);
        // `$VLINE$` is unescaped first, as in the engine.
        assert_eq!(
            "$TILDE$VLINE$~1~1~1~1".parse::<Flag>().unwrap().name,
            "$TILDE|"
        );
    }

    #[test]
    fn malformed_entries_are_errors() {
        let error = Flag::parse_all("ok~1~1~1~1|bad~1~1~1").unwrap_err();
        assert_eq!(error.entry, "bad~1~1~1");
        assert_eq!(
            error.to_string(),
            "invalid flag entry \"bad~1~1~1\": expected 5 `~`-separated fields"
        );
        assert_eq!(
            "f~x~1~1~1".parse::<Flag>().unwrap_err().problem,
            "invalid color"
        );
        assert_eq!(
            "f~1~1~1~-2".parse::<Flag>().unwrap_err().problem,
            "invalid y"
        );
    }
}
