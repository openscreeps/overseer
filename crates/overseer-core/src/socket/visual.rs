//! Visual strings: one JSON item per line (room visuals of `room:` events, `mapVisual:` events).

use serde::de::DeserializeOwned;

/// The items of a visual string, skipping empty lines; a line that is not a `T` is an error.
pub(super) fn items<T: DeserializeOwned>(
    text: &str,
) -> impl Iterator<Item = Result<T, serde_json::Error>> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(serde_json::from_str)
}
