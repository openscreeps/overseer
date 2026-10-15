//! Subscription channels and their wire names.

use std::fmt;

/// A socket channel to subscribe to.
///
/// [`Display`](fmt::Display) writes the wire name sent with `subscribe`/`unsubscribe` and carried
/// by the channel's events. A `shard` of `None` selects the shardless form used by standalone
/// (private) servers, which run one world: pass `Some` exactly when `GET /api/version` lists
/// shards. User channels take the user id (`_id` of `GET /api/auth/me`), not the username.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Server announcements: `server-message`. The only channel open to anonymous connections.
    ServerMessage,
    /// Detailed room updates: `room:<shard>/<room>` or `room:<room>`.
    Room {
        /// Shard of the room; `None` on standalone servers.
        shard: Option<String>,
        /// Room name, for example `W8N3`.
        room: String,
    },
    /// Low-detail room view for the world map: `roomMap2:<shard>/<room>` or `roomMap2:<room>`.
    RoomMap {
        /// Shard of the room; `None` on standalone servers.
        shard: Option<String>,
        /// Room name, for example `W8N3`.
        room: String,
    },
    /// The user's map visuals: `mapVisual:<user>/<shard>` or `mapVisual:<user>`.
    MapVisual {
        /// User id.
        user: String,
        /// Shard; `None` on standalone servers.
        shard: Option<String>,
    },
    /// CPU and Memory usage of each run: `user:<user>/cpu`.
    Cpu {
        /// User id.
        user: String,
    },
    /// Console output and run errors: `user:<user>/console`.
    Console {
        /// User id.
        user: String,
    },
    /// The string form of one Memory path: `user:<user>/memory/<shard>/<path>` or
    /// `user:<user>/memory/<path>`.
    Memory {
        /// User id.
        user: String,
        /// Shard; `None` on standalone servers.
        shard: Option<String>,
        /// Dot-separated Memory path, for example `stats.tick`.
        path: String,
    },
    /// Account resources and credits: `user:<user>/resources`.
    Resources {
        /// User id.
        user: String,
    },
    /// Saved code branches: `user:<user>/code`.
    Code {
        /// User id.
        user: String,
    },
    /// Active world or simulation branch changes: `user:<user>/set-active-branch`.
    ActiveBranch {
        /// User id.
        user: String,
    },
    /// Incoming private messages: `user:<user>/newMessage`.
    NewMessage {
        /// User id.
        user: String,
    },
    /// Updates of one conversation: `user:<user>/message:<respondent>`.
    Conversation {
        /// User id.
        user: String,
        /// User id of the other participant.
        respondent: String,
    },
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ServerMessage => f.write_str("server-message"),
            Self::Room { shard, room } => sharded_room(f, "room", shard.as_deref(), room),
            Self::RoomMap { shard, room } => sharded_room(f, "roomMap2", shard.as_deref(), room),
            Self::MapVisual {
                user,
                shard: Some(shard),
            } => write!(f, "mapVisual:{user}/{shard}"),
            Self::MapVisual { user, shard: None } => write!(f, "mapVisual:{user}"),
            Self::Cpu { user } => write!(f, "user:{user}/cpu"),
            Self::Console { user } => write!(f, "user:{user}/console"),
            Self::Memory {
                user,
                shard: Some(shard),
                path,
            } => {
                write!(f, "user:{user}/memory/{shard}/{path}")
            }
            Self::Memory {
                user,
                shard: None,
                path,
            } => write!(f, "user:{user}/memory/{path}"),
            Self::Resources { user } => write!(f, "user:{user}/resources"),
            Self::Code { user } => write!(f, "user:{user}/code"),
            Self::ActiveBranch { user } => write!(f, "user:{user}/set-active-branch"),
            Self::NewMessage { user } => write!(f, "user:{user}/newMessage"),
            Self::Conversation { user, respondent } => {
                write!(f, "user:{user}/message:{respondent}")
            }
        }
    }
}

fn sharded_room(
    f: &mut fmt::Formatter<'_>,
    prefix: &str,
    shard: Option<&str>,
    room: &str,
) -> fmt::Result {
    match shard {
        Some(shard) => write!(f, "{prefix}:{shard}/{room}"),
        None => write!(f, "{prefix}:{room}"),
    }
}

#[cfg(test)]
mod tests {
    use super::Channel;

    fn user(user: &str) -> String {
        user.to_owned()
    }

    #[test]
    fn wire_names_of_official_and_shardless_channels() {
        let shard = || Some("shard3".to_owned());
        let u = user("5a1b");
        let cases = [
            (Channel::ServerMessage, "server-message"),
            (
                Channel::Room {
                    shard: shard(),
                    room: "W8N3".into(),
                },
                "room:shard3/W8N3",
            ),
            (
                Channel::Room {
                    shard: None,
                    room: "W8N3".into(),
                },
                "room:W8N3",
            ),
            (
                Channel::RoomMap {
                    shard: shard(),
                    room: "W8N3".into(),
                },
                "roomMap2:shard3/W8N3",
            ),
            (
                Channel::RoomMap {
                    shard: None,
                    room: "W8N3".into(),
                },
                "roomMap2:W8N3",
            ),
            (
                Channel::MapVisual {
                    user: u.clone(),
                    shard: shard(),
                },
                "mapVisual:5a1b/shard3",
            ),
            (
                Channel::MapVisual {
                    user: u.clone(),
                    shard: None,
                },
                "mapVisual:5a1b",
            ),
            (Channel::Cpu { user: u.clone() }, "user:5a1b/cpu"),
            (Channel::Console { user: u.clone() }, "user:5a1b/console"),
            (
                Channel::Memory {
                    user: u.clone(),
                    shard: shard(),
                    path: "stats.tick".into(),
                },
                "user:5a1b/memory/shard3/stats.tick",
            ),
            (
                Channel::Memory {
                    user: u.clone(),
                    shard: None,
                    path: "stats.tick".into(),
                },
                "user:5a1b/memory/stats.tick",
            ),
            (
                Channel::Resources { user: u.clone() },
                "user:5a1b/resources",
            ),
            (Channel::Code { user: u.clone() }, "user:5a1b/code"),
            (
                Channel::ActiveBranch { user: u.clone() },
                "user:5a1b/set-active-branch",
            ),
            (
                Channel::NewMessage { user: u.clone() },
                "user:5a1b/newMessage",
            ),
            (
                Channel::Conversation {
                    user: u,
                    respondent: user("6c7d"),
                },
                "user:5a1b/message:6c7d",
            ),
        ];
        for (channel, wire) in cases {
            assert_eq!(channel.to_string(), wire);
        }
    }
}
