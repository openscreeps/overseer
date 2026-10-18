//! The text of a tick duration: short for the status bar ([`compact`]) and in a sentence with
//! its source ([`sentence`]). The session measures it ([`overseer::Map::tick_speed`]).

use overseer::map::{TickSource, TickSpeed};

/// The most characters of [`compact`] (`250 ms/tick`).
pub const COMPACT_CHARS: usize = 11;

/// A tick duration of `ms` milliseconds, short, for the status bar, with its unit:
/// `250 ms/tick` below a second, `3.1 s/tick` below 10 seconds, `12 s/tick` from there on; at
/// most [`COMPACT_CHARS`] characters for ticks below 1000 s.
pub fn compact(ms: f64) -> String {
    let seconds = ms / 1000.0;
    if ms < 999.5 {
        format!("{ms:.0} ms/tick")
    } else if seconds < 9.95 {
        format!("{seconds:.1} s/tick")
    } else {
        format!("{seconds:.0} s/tick")
    }
}

/// `speed` in a sentence: `4.00 s per tick (0.25 ticks per second), reported by the server` or
/// `… measured from game time`.
pub fn sentence(speed: &TickSpeed) -> String {
    let source = match speed.source {
        TickSource::Server => "reported by the server",
        TickSource::Measured => "measured from game time",
    };
    format!(
        "{:.2} s per tick ({:.2} ticks per second), {source}",
        speed.ms / 1000.0,
        1000.0 / speed.ms
    )
}

#[cfg(test)]
mod tests {
    use overseer::map::{TickSource, TickSpeed};

    use super::{COMPACT_CHARS, compact, sentence};

    #[test]
    fn durations_read_with_their_unit() {
        assert_eq!(compact(250.0), "250 ms/tick");
        assert_eq!(compact(999.0), "999 ms/tick");
        assert_eq!(compact(1000.0), "1.0 s/tick");
        assert_eq!(compact(3104.0), "3.1 s/tick");
        assert_eq!(compact(9960.0), "10 s/tick");
        assert_eq!(compact(123_400.0), "123 s/tick");
        for ms in [250.0, 999.0, 3104.0, 123_400.0] {
            assert!(compact(ms).chars().count() <= COMPACT_CHARS, "{ms}");
        }
        let server = TickSpeed {
            ms: 4000.0,
            source: TickSource::Server,
        };
        assert_eq!(compact(server.ms), "4.0 s/tick");
        assert_eq!(
            sentence(&server),
            "4.00 s per tick (0.25 ticks per second), reported by the server"
        );
        let measured = TickSpeed {
            ms: 250.0,
            source: TickSource::Measured,
        };
        assert_eq!(
            sentence(&measured),
            "0.25 s per tick (4.00 ticks per second), measured from game time"
        );
    }
}
