//! The app's routes: the URL hash, shareable and restored on start.
//!
//! | Hash | Route |
//! |---|---|
//! | `#/` (or no hash) | [`Route::Servers`]: the server list |
//! | `#/<server>/` | [`Route::World`] of the server's default (or last) shard |
//! | `#/<server>/<shard>/` | [`Route::World`] of a shard |
//! | `#/<server>/<shard>/<room>` | [`Route::World`] focused on a room |
//! | `#/<server>/<shard>/<room>/window` | [`Route::Window`]: the room control alone (pop-out) |
//! | `#/<server>/@profile` | [`Route::Profile`]: the signed-in account |
//!
//! `<server>` is the stable slug of a saved server ([`crate::profiles::Profiles::slug`]),
//! `<shard>` a shard name or `-` on single-world servers ([`Shard::Single`]), `<room>` a world
//! room name (any case; written upper-case). `@` starts the server's own pages, so it never
//! begins a shard name.

use std::fmt;

use overseer::map::RoomCoord;

/// The shard segment of a route.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Shard {
    /// A single-world server (`-`).
    Single,
    /// A named shard.
    Named(String),
}

impl Shard {
    /// The segment for a server's shard (`None` on single-world servers).
    pub fn of(shard: Option<&str>) -> Self {
        shard.map_or(Self::Single, |name| Self::Named(name.to_owned()))
    }

    /// The shard name the library takes (`None` on single-world servers).
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Single => None,
            Self::Named(name) => Some(name),
        }
    }

    /// Parses a shard segment: `-`, or a name of ASCII letters, digits, `_` and `-`.
    fn parse(segment: &str) -> Option<Self> {
        if segment == "-" {
            return Some(Self::Single);
        }
        let valid = !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
        valid.then(|| Self::Named(segment.to_owned()))
    }
}

impl fmt::Display for Shard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name().unwrap_or("-"))
    }
}

/// Where the app is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The server list.
    Servers,
    /// The signed-in account of a server.
    Profile {
        /// Server slug.
        server: String,
    },
    /// The world map of a server.
    World {
        /// Server slug.
        server: String,
        /// The shard; `None`: the last or default shard (resolved once connected).
        shard: Option<Shard>,
        /// The room in focus; only with a shard.
        room: Option<RoomCoord>,
    },
    /// One room alone (a popped-out room control).
    Window {
        /// Server slug.
        server: String,
        /// The shard.
        shard: Shard,
        /// The room.
        room: RoomCoord,
    },
}

impl Route {
    /// Parses a URL hash (with or without the leading `#`); `None` for anything that is not a
    /// route.
    pub fn parse(hash: &str) -> Option<Self> {
        let hash = hash.strip_prefix('#').unwrap_or(hash);
        if hash.is_empty() || hash == "/" {
            return Some(Self::Servers);
        }
        let path = hash.strip_prefix('/')?;
        let segments: Vec<&str> = path.strip_suffix('/').unwrap_or(path).split('/').collect();
        let server = (*segments.first()?).to_owned();
        if !is_slug(&server) {
            return None;
        }
        match segments[1..] {
            [] => Some(Self::World {
                server,
                shard: None,
                room: None,
            }),
            ["@profile"] => Some(Self::Profile { server }),
            [shard] => Some(Self::World {
                server,
                shard: Some(Shard::parse(shard)?),
                room: None,
            }),
            [shard, room] => Some(Self::World {
                server,
                shard: Some(Shard::parse(shard)?),
                room: Some(parse_room(room)?),
            }),
            [shard, room, "window"] => Some(Self::Window {
                server,
                shard: Shard::parse(shard)?,
                room: parse_room(room)?,
            }),
            _ => None,
        }
    }

    /// The URL hash, `#/…`.
    pub fn hash(&self) -> String {
        self.to_string()
    }

    /// The server slug, if the route is about a server.
    pub fn server(&self) -> Option<&str> {
        match self {
            Self::Servers => None,
            Self::Profile { server } | Self::World { server, .. } | Self::Window { server, .. } => {
                Some(server)
            }
        }
    }
}

impl fmt::Display for Route {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Servers => f.write_str("#/"),
            Self::Profile { server } => write!(f, "#/{server}/@profile"),
            Self::World {
                server,
                shard: None,
                ..
            } => write!(f, "#/{server}/"),
            Self::World {
                server,
                shard: Some(shard),
                room: None,
            } => write!(f, "#/{server}/{shard}/"),
            Self::World {
                server,
                shard: Some(shard),
                room: Some(room),
            } => write!(f, "#/{server}/{shard}/{room}"),
            Self::Window {
                server,
                shard,
                room,
            } => write!(f, "#/{server}/{shard}/{room}/window"),
        }
    }
}

/// A server slug: ASCII lower-case letters, digits, `.` and `-`, not starting with `@`.
fn is_slug(segment: &str) -> bool {
    !segment.is_empty()
        && segment.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'-'
        })
}

/// A world room name in any case.
pub fn parse_room(name: &str) -> Option<RoomCoord> {
    name.trim().to_ascii_uppercase().parse().ok()
}

#[cfg(test)]
mod tests {
    use overseer::map::RoomCoord;

    use super::{Route, Shard};

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    #[test]
    fn round_trips() {
        let routes = [
            ("#/", Route::Servers),
            (
                "#/screeps.com/@profile",
                Route::Profile {
                    server: "screeps.com".to_owned(),
                },
            ),
            (
                "#/screeps.com/",
                Route::World {
                    server: "screeps.com".to_owned(),
                    shard: None,
                    room: None,
                },
            ),
            (
                "#/screeps.com-season/shard0/",
                Route::World {
                    server: "screeps.com-season".to_owned(),
                    shard: Some(Shard::Named("shard0".to_owned())),
                    room: None,
                },
            ),
            (
                "#/localhost-21025/-/",
                Route::World {
                    server: "localhost-21025".to_owned(),
                    shard: Some(Shard::Single),
                    room: None,
                },
            ),
            (
                "#/localhost-21025/-/W8N3",
                Route::World {
                    server: "localhost-21025".to_owned(),
                    shard: Some(Shard::Single),
                    room: Some(room("W8N3")),
                },
            ),
            (
                "#/screeps.com/shard3/E12S0/window",
                Route::Window {
                    server: "screeps.com".to_owned(),
                    shard: Shard::Named("shard3".to_owned()),
                    room: room("E12S0"),
                },
            ),
            (
                "#/127.0.0.1-21025/-/W0N0/window",
                Route::Window {
                    server: "127.0.0.1-21025".to_owned(),
                    shard: Shard::Single,
                    room: room("W0N0"),
                },
            ),
        ];
        for (hash, route) in routes {
            assert_eq!(Route::parse(hash).as_ref(), Some(&route), "{hash}");
            assert_eq!(route.hash(), hash);
        }
    }

    #[test]
    fn parses_lenient_forms() {
        assert_eq!(Route::parse(""), Some(Route::Servers));
        assert_eq!(Route::parse("#"), Some(Route::Servers));
        assert_eq!(
            Route::parse("#/a"),
            Route::parse("#/a/"),
            "the trailing slash is optional"
        );
        assert_eq!(
            Route::parse("/a/shard0/w8n3"),
            Route::parse("#/a/shard0/W8N3"),
            "room names in any case, `#` optional"
        );
        assert_eq!(
            Route::parse("#/a/-/W8N3/"),
            Route::parse("#/a/-/W8N3"),
            "a trailing slash after the room"
        );
    }

    #[test]
    fn rejects_other_hashes() {
        for hash in [
            "#a",
            "#//",
            "#/A/",
            "#/a b/",
            "#/a/@other",
            "#/a/sh ard/",
            "#/a/-/sim",
            "#/a/-/W8N3/other",
            "#/a/-/W8N3/window/x",
        ] {
            assert_eq!(Route::parse(hash), None, "{hash}");
        }
    }

    #[test]
    fn shard_names() {
        assert_eq!(Shard::of(None), Shard::Single);
        assert_eq!(Shard::of(Some("shard1")).name(), Some("shard1"));
        assert_eq!(Shard::Single.to_string(), "-");
    }
}
