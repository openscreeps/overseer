//! Memory paths: the dot-separated keys from `Memory` down to a value.

use std::fmt;
use std::str::FromStr;

/// A dot-separated path from `Memory` down to a value (`creeps.Harvester1.role`); the empty
/// path ([`Self::root`]) is all of `Memory`.
///
/// The servers split a path at its dots and put each key in a quoted JavaScript string
/// (`Memory['creeps']['Harvester1']`). Thus a key cannot be empty, cannot hold a dot, and a key
/// with a backslash or a line break does not survive the quotes ([`Self::is_key`]). A
/// `MemoryPath` holds only such keys. A server can refuse more paths than this type does
/// (for example a `__proto__` key on `openscreeps/server`).
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryPath(String);

/// Why a text or a key cannot be a part of a [`MemoryPath`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MemoryPathError {
    /// A key is empty (`creeps..c1`, `creeps.`, `.creeps`).
    #[error("a Memory path has an empty key")]
    EmptyKey,
    /// A key holds a dot, a backslash or a line break.
    #[error("the Memory key {0:?} has a dot, a backslash or a line break")]
    Unaddressable(String),
}

impl MemoryPath {
    /// All of `Memory`: the empty path.
    #[must_use]
    pub const fn root() -> Self {
        Self(String::new())
    }

    /// Whether `key` can be a key of a path: not empty, and without a dot, a backslash or a
    /// line break.
    #[must_use]
    pub fn is_key(key: &str) -> bool {
        check_key(key).is_ok()
    }

    /// The path `text` (`creeps.c1`); the empty text is [`Self::root`]. The text is used as it
    /// is: spaces are parts of keys.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryPathError`] when a key of `text` cannot be a key of a path.
    pub fn parse(text: &str) -> Result<Self, MemoryPathError> {
        if text.is_empty() {
            return Ok(Self::root());
        }
        for key in text.split('.') {
            check_key(key)?;
        }
        Ok(Self(text.to_owned()))
    }

    /// The path of the value at `keys` under this path.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryPathError`] when a key cannot be a key of a path.
    pub fn join<K: AsRef<str>>(
        &self,
        keys: impl IntoIterator<Item = K>,
    ) -> Result<Self, MemoryPathError> {
        let mut path = self.0.clone();
        for key in keys {
            let key = key.as_ref();
            check_key(key)?;
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(key);
        }
        Ok(Self(path))
    }

    /// The path as the servers take it (empty for all of `Memory`).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is all of `Memory`.
    #[must_use]
    pub const fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The keys from `Memory` down to the value; none for all of `Memory`.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.split('.').filter(|key| !key.is_empty())
    }

    /// The keys from the value at `base` down to the value at this path; `None` when `base` is
    /// not this path or a path above it.
    #[must_use]
    pub fn keys_below(&self, base: &Self) -> Option<Vec<&str>> {
        let mut keys = self.keys();
        for key in base.keys() {
            if keys.next() != Some(key) {
                return None;
            }
        }
        Some(keys.collect())
    }
}

impl fmt::Display for MemoryPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for MemoryPath {
    type Err = MemoryPathError;

    fn from_str(text: &str) -> Result<Self, MemoryPathError> {
        Self::parse(text)
    }
}

impl AsRef<str> for MemoryPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Rejects a key that cannot be a key of a path.
fn check_key(key: &str) -> Result<(), MemoryPathError> {
    if key.is_empty() {
        Err(MemoryPathError::EmptyKey)
    } else if key.contains(['.', '\\', '\n', '\r']) {
        Err(MemoryPathError::Unaddressable(key.to_owned()))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryPath, MemoryPathError};

    fn path(text: &str) -> MemoryPath {
        MemoryPath::parse(text).unwrap()
    }

    #[test]
    fn parses_paths() {
        assert_eq!(path("creeps.c1").as_str(), "creeps.c1");
        assert_eq!(path(""), MemoryPath::root());
        assert!(MemoryPath::root().is_root());
        assert_eq!(path(" a b ").as_str(), " a b ");
        assert_eq!(
            MemoryPath::parse("creeps..c1"),
            Err(MemoryPathError::EmptyKey)
        );
        assert_eq!(MemoryPath::parse("creeps."), Err(MemoryPathError::EmptyKey));
        assert_eq!(MemoryPath::parse(".creeps"), Err(MemoryPathError::EmptyKey));
        assert_eq!(
            MemoryPath::parse("a\\b"),
            Err(MemoryPathError::Unaddressable("a\\b".to_owned()))
        );
        assert!(MemoryPath::parse("a\nb").is_err());
        assert_eq!("x.y".parse::<MemoryPath>(), Ok(path("x.y")));
    }

    #[test]
    fn keys_that_a_path_cannot_hold_make_no_path() {
        for key in ["a.b", "a\\b", "a\nb", "a\rb", ""] {
            assert!(!MemoryPath::is_key(key), "{key:?}");
            assert!(
                MemoryPath::root().join(["rooms", key, "x"]).is_err(),
                "{key:?}"
            );
        }
        assert_eq!(
            MemoryPath::root().join(["W1N1", "my-key", "é 1"]),
            Ok(path("W1N1.my-key.é 1"))
        );
    }

    #[test]
    fn joins_keys_into_paths() {
        assert_eq!(
            MemoryPath::root().join(["creeps", "c1"]),
            Ok(path("creeps.c1"))
        );
        assert_eq!(path("creeps").join(["c1", "0"]), Ok(path("creeps.c1.0")));
        assert_eq!(path("creeps").join([] as [&str; 0]), Ok(path("creeps")));
        assert_eq!(
            MemoryPath::root().join([] as [&str; 0]),
            Ok(MemoryPath::root())
        );
    }

    #[test]
    fn splits_paths_into_keys() {
        assert_eq!(MemoryPath::root().keys().count(), 0);
        assert_eq!(
            path("creeps.c1").keys().collect::<Vec<_>>(),
            ["creeps", "c1"]
        );
        let full = path("creeps.c1.memory");
        assert_eq!(
            full.keys_below(&MemoryPath::root()),
            Some(vec!["creeps", "c1", "memory"])
        );
        assert_eq!(full.keys_below(&path("creeps")), Some(vec!["c1", "memory"]));
        assert_eq!(full.keys_below(&full), Some(vec![]));
        assert_eq!(full.keys_below(&path("creeps.c2")), None);
        assert_eq!(path("creeps").keys_below(&full), None);
        // Keys compare whole: `cree` is not above `creeps`.
        assert_eq!(full.keys_below(&path("cree")), None);
    }
}
