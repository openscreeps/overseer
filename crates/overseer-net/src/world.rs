//! What the driver knows about the server's world and the signed-in user.

use overseer_core::ServerUrl;
use overseer_core::api::types::PlatformVersion;

/// The server's shards and capabilities and the signed-in user, as discovered by
/// [`Client::world`].
///
/// Official servers ([`ServerUrl::is_official`]) run several shards and qualify room and
/// Memory channels, room history and most requests with one. Private servers run a single
/// world and name no shard anywhere, whatever their `GET /api/version` lists
/// ([`PlatformVersion::shards`]): the official web client never qualifies their channels
/// either, and the Screeps server's socket only knows bare room names.
///
/// [`Client::world`]: crate::Client::world
#[derive(Clone, Debug, PartialEq)]
pub struct World {
    version: PlatformVersion,
    shards: Vec<String>,
    shard: Option<String>,
    user: Option<String>,
}

impl World {
    /// The world of `server` described by its `GET /api/version` reply and the user id from
    /// `GET /api/auth/me`. On an official server the default shard is the first one listed.
    #[must_use]
    pub fn new(server: &ServerUrl, version: PlatformVersion, user: Option<String>) -> Self {
        let shards: Vec<String> = if server.is_official() {
            version.shards().map(str::to_owned).collect()
        } else {
            Vec::new()
        };
        let shard = shards.first().cloned();
        Self {
            version,
            shards,
            shard,
            user,
        }
    }

    /// The server's `GET /api/version` reply: protocol, package and `serverData` (history chunk
    /// size, socket update throttle, welcome text, renderer metadata, …).
    #[must_use]
    pub const fn version(&self) -> &PlatformVersion {
        &self.version
    }

    /// The shards channels and requests can name, in the order the server lists them; empty on
    /// private servers, whatever they list.
    #[must_use]
    pub fn shards(&self) -> &[String] {
        &self.shards
    }

    /// Whether channels and requests name a shard: on official servers.
    #[must_use]
    pub fn is_sharded(&self) -> bool {
        !self.shards.is_empty()
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

    /// Ticks per room-history chunk (`serverData.historyChunkSize`); chunks start at multiples
    /// of it. At least 1, whatever the server reports.
    #[must_use]
    pub fn history_chunk_size(&self) -> u64 {
        u64::try_from(self.version.server_data.history_chunk_size)
            .unwrap_or(1)
            .max(1)
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
    use overseer_core::ServerUrl;
    use overseer_core::api::types::PlatformVersion;

    use super::World;

    fn version(shards: &str, chunk: i64) -> PlatformVersion {
        serde_json::from_str(&format!(
            r#"{{"ok":1,"protocol":14,"users":1,"serverData":{{"customObjectTypes":{{}},
                "historyChunkSize":{chunk},"renderer":{{"resources":{{}},"metadata":{{}}}}{shards}}}}}"#
        ))
        .unwrap()
    }

    fn private() -> ServerUrl {
        ServerUrl::parse("http://localhost:21025/").unwrap()
    }

    #[test]
    fn official_world_defaults_to_the_first_shard_and_honours_overrides() {
        let shards = r#","shards":["shard0","shard3"]"#;
        let mut world = World::new(&ServerUrl::official(), version(shards, 100), None);
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
        // Through a proxy in front of it.
        let proxy = ServerUrl::proxied("http://127.0.0.1:8080/", &ServerUrl::official()).unwrap();
        assert!(World::new(&proxy, version(shards, 100), None).is_sharded());
    }

    #[test]
    fn private_world_never_names_a_shard_whatever_it_lists() {
        // Nothing listed (OpenScreeps, the stock Screeps server), screepsmod-admin-utils'
        // unset `shardName`, and a set one.
        for shards in ["", r#","shards":[null]"#, r#","shards":["shard0"]"#] {
            let mut world = World::new(&private(), version(shards, 20), Some("5a1b".into()));
            world.set_shard("shard0");
            assert!(!world.is_sharded(), "{shards}");
            assert_eq!(world.shards(), [] as [String; 0]);
            assert_eq!(world.shard(), None);
            assert_eq!(world.resolve_shard(Some("shard0")), None);
            assert_eq!(world.user(), Some("5a1b"));
            assert_eq!(world.history_chunk_size(), 20);
        }
    }

    #[test]
    fn history_chunk_size_is_at_least_one() {
        for chunk in [0, -5] {
            let world = World::new(&private(), version("", chunk), None);
            assert_eq!(world.history_chunk_size(), 1);
        }
    }
}
