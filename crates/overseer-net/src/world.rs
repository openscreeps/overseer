//! What the driver knows about the server's world and the signed-in user.

use overseer_core::model::platform::{ConnectionInfo, ServerInfo, Subsystem, Subsystems};

/// The server's subsystems, shards and connection info and the signed-in user, as discovered
/// by [`Client::world`].
///
/// Decide behaviour by [`Self::subsystems`] (what the server offers) and [`Self::is_sharded`]
/// (whether requests name a shard), never by [`Self::connection`]'s kind or plugins, which are
/// for display and diagnostics.
///
/// Official servers run several shards and qualify room and Memory channels, room history and
/// most requests with one. Every other server runs a single world and names no shard anywhere,
/// whatever its `GET /api/version` lists ([`ServerInfo::shards`]): the official web client never
/// qualifies their channels either, and the Screeps server's socket only knows bare room names.
///
/// [`Client::world`]: crate::Client::world
#[derive(Clone, Debug, PartialEq)]
pub struct World {
    connection: ConnectionInfo,
    shard: Option<String>,
    user: Option<String>,
}

impl World {
    /// The world of the server `connection` describes, and the user id from
    /// `GET /api/auth/me`. The default shard is the first one listed ([`ServerInfo::shards`]:
    /// official servers only).
    #[must_use]
    pub fn new(connection: ConnectionInfo, user: Option<String>) -> Self {
        let shard = connection.server.shards.first().cloned();
        Self {
            connection,
            shard,
            user,
        }
    }

    /// The server's kind, plugins and `GET /api/version`, for display and diagnostics only:
    /// decide behaviour by [`Self::subsystems`].
    #[must_use]
    pub const fn connection(&self) -> &ConnectionInfo {
        &self.connection
    }

    /// The subsystems the server offers ([`ConnectionInfo::subsystems`]).
    #[must_use]
    pub const fn subsystems(&self) -> Subsystems {
        self.connection.subsystems()
    }

    /// Whether the server offers `subsystem`.
    #[must_use]
    pub const fn supports(&self, subsystem: Subsystem) -> bool {
        self.subsystems().contains(subsystem)
    }

    /// What the server reports in `GET /api/version`: protocol, package, history chunk size,
    /// socket update throttle, welcome text, renderer metadata, shards and features.
    #[must_use]
    pub const fn version(&self) -> &ServerInfo {
        &self.connection.server
    }

    /// The shards channels and requests can name, in the order the server lists them; empty on
    /// single-world servers, whatever they list.
    #[must_use]
    pub fn shards(&self) -> &[String] {
        &self.version().shards
    }

    /// Whether channels and requests name a shard: on official servers.
    #[must_use]
    pub fn is_sharded(&self) -> bool {
        !self.version().shards.is_empty()
    }

    /// The shard used when none is named; `None` on single-world servers.
    #[must_use]
    pub fn shard(&self) -> Option<&str> {
        self.shard.as_deref()
    }

    /// Replaces the default shard. Single-world servers have no shards and keep `None`.
    pub fn set_shard(&mut self, shard: impl Into<String>) {
        if self.is_sharded() {
            self.shard = Some(shard.into());
        }
    }

    /// The signed-in user's id (`_id`), the key of user channels; `None` when anonymous.
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Ticks per room-history chunk ([`ServerInfo::history_chunk_size`]); chunks start at
    /// multiples of it. At least 1.
    #[must_use]
    pub const fn history_chunk_size(&self) -> u64 {
        self.version().history_chunk_size
    }

    /// The shard a channel or request names: `shard`, else the default shard, and `None` on
    /// single-world servers whatever was asked.
    pub(crate) fn resolve_shard(&self, shard: Option<&str>) -> Option<String> {
        if self.is_sharded() {
            shard.or(self.shard()).map(str::to_owned)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use overseer_core::model::platform::{
        ConnectionInfo, Plugins, ServerInfo, ServerKind, Subsystem, Subsystems,
    };

    use super::World;

    fn connection(
        kind: ServerKind,
        plugins: Plugins,
        shards: &[&str],
        history_chunk_size: u64,
    ) -> ConnectionInfo {
        ConnectionInfo {
            kind,
            plugins,
            forced: false,
            server: ServerInfo {
                protocol: 14,
                history_chunk_size,
                shards: shards.iter().map(|&shard| shard.to_owned()).collect(),
                ..ServerInfo::default()
            },
        }
    }

    #[test]
    fn official_world_defaults_to_the_first_shard_and_honours_overrides() {
        let official = connection(
            ServerKind::Official,
            Plugins::NONE,
            &["shard0", "shard3"],
            100,
        );
        let mut world = World::new(official.clone(), None);
        assert_eq!(world.connection(), &official);
        assert_eq!(world.shards(), ["shard0", "shard3"]);
        assert_eq!(world.shard(), Some("shard0"));
        assert_eq!(world.resolve_shard(None).as_deref(), Some("shard0"));
        assert_eq!(
            world.resolve_shard(Some("shard1")).as_deref(),
            Some("shard1")
        );
        world.set_shard("shard3");
        assert_eq!(world.resolve_shard(None).as_deref(), Some("shard3"));
        assert_eq!(world.history_chunk_size(), 100);
        assert!(world.supports(Subsystem::RoomHistory));
    }

    #[test]
    fn single_world_never_names_a_shard() {
        let plugins = Plugins {
            screepsmod_auth: true,
            ..Plugins::NONE
        };
        let mut world = World::new(
            connection(ServerKind::Community, plugins, &[], 20),
            Some("5a1b".into()),
        );
        world.set_shard("shard0");
        assert!(!world.is_sharded());
        assert_eq!(world.shards(), [] as [String; 0]);
        assert_eq!(world.shard(), None);
        assert_eq!(world.resolve_shard(Some("shard0")), None);
        assert_eq!(world.user(), Some("5a1b"));
        assert_eq!(world.history_chunk_size(), 20);
        assert_eq!(world.connection().plugins, plugins);
        assert_eq!(
            world.subsystems(),
            Subsystems::of(ServerKind::Community, plugins)
        );
        assert!(world.supports(Subsystem::PasswordSignIn));
        assert!(!world.supports(Subsystem::RoomObjects));
    }
}
