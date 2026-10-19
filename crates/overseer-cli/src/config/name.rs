//! The names of the configuration: server, profile and shard names. A name
//! is not empty and has only letters, digits, `-`, `_` and `.`. The server of
//! a profile ([`ServerRef`]) can also be the name or the URL of a saved
//! server.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;

/// Defines a name type. A name is not empty and has only letters, digits,
/// `-`, `_` and `.`.
macro_rules! name {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
        #[serde(try_from = "String")]
        pub(crate) struct $name(String);

        impl TryFrom<String> for $name {
            type Error = InvalidName;

            fn try_from(name: String) -> Result<Self, InvalidName> {
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
                {
                    Ok(Self(name))
                } else {
                    Err(InvalidName { what: $what, name })
                }
            }
        }

        impl FromStr for $name {
            type Err = InvalidName;

            fn from_str(name: &str) -> Result<Self, InvalidName> {
                Self::try_from(name.to_owned())
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.pad(&self.0)
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

name!(
    /// The name of a server of the configuration.
    ServerName,
    "server"
);
name!(
    /// The name of a profile of the configuration.
    ProfileName,
    "profile"
);
name!(
    /// The name of a shard of a server (`shard3`).
    ShardName,
    "shard"
);

impl ShardName {
    /// The name as the API takes it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The server of a profile: the name of a `[servers.NAME]` table of the
/// configuration, or the name or the URL of a saved server. It is not empty.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct ServerRef(String);

impl ServerRef {
    /// The text of the configuration.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ServerRef {
    type Error = EmptyServer;

    fn try_from(server: String) -> Result<Self, EmptyServer> {
        if server.trim().is_empty() {
            Err(EmptyServer)
        } else {
            Ok(Self(server))
        }
    }
}

impl fmt::Display for ServerRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

/// The server of a profile is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("server: the server of a profile is not empty")]
pub(crate) struct EmptyServer;

/// A name that is empty or has a character that is not a letter, a digit,
/// `-`, `_` or `.`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{what} {name:?}: a name has letters, digits, `-`, `_`, and `.`")]
pub(crate) struct InvalidName {
    what: &'static str,
    name: String,
}
