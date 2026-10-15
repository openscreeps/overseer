//! What the driver knows about the server's world and the signed-in user.

use overseer_core::api::types::PlatformVersion;

/// The server's shards and capabilities and the signed-in user, as discovered by
/// [`Client::world`].
///
/// Official servers run several shards and qualify room and Memory channels, as well as most
/// requests, with one; private servers run a single world (no shards listed) and use shardless
/// channel names.
///
/// [`Client::world`]: crate::Client::world
#[derive(Clone, Debug, PartialEq)]
pub struct World {
    version: PlatformVersion,
    shard: Option<String>,
    user: Option<String>,
}

impl World {
    /// A world described by `GET /api/version` (no shards listed for a single-world server) and
    /// the user id from `GET /api/auth/me`. The default shard is the first one listed.
    #[must_use]
    pub fn new(version: PlatformVersion, user: Option<String>) -> Self {
        let shard = version.shards().first().cloned();
        Self {
            version,
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

    /// The shards, in the order the server lists them; empty on single-world servers.
    #[must_use]
    pub fn shards(&self) -> &[String] {
        self.version.shards()
    }

    /// Whether channels and requests name a shard.
    #[must_use]
    pub fn is_sharded(&self) -> bool {
        !self.shards().is_empty()
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
    use overseer_core::api::types::PlatformVersion;

    use super::World;

    fn version(shards: &str, chunk: i64) -> PlatformVersion {
        serde_json::from_str(&format!(
            r#"{{"ok":1,"protocol":14,"users":1,"serverData":{{"customObjectTypes":{{}},
                "historyChunkSize":{chunk},"renderer":{{"resources":{{}},"metadata":{{}}}}{shards}}}}}"#
        ))
        .unwrap()
    }

    #[test]
    fn sharded_world_defaults_to_the_first_shard_and_honours_overrides() {
        let mut world = World::new(version(r#","shards":["shard0","shard3"]"#, 100), None);
        assert_eq!(world.shard(), Some("shard0"));
        assert_eq!(world.resolve_shard(None).as_deref(), Some("shard0"));
        assert_eq!(
            world.resolve_shard(Some("shard1")).as_deref(),
            Some("shard1")
        );
        world.set_shard("shard3");
        assert_eq!(world.resolve_shard(None).as_deref(), Some("shard3"));
        assert_eq!(world.history_chunk_size(), 100);
    }

    #[test]
    fn single_world_never_names_a_shard() {
        let mut world = World::new(version("", 20), Some("5a1b".into()));
        world.set_shard("shard0");
        assert!(!world.is_sharded());
        assert_eq!(world.shard(), None);
        assert_eq!(world.resolve_shard(Some("shard0")), None);
        assert_eq!(world.user(), Some("5a1b"));
        assert_eq!(world.history_chunk_size(), 20);
    }

    #[test]
    fn history_chunk_size_is_at_least_one() {
        assert_eq!(World::new(version("", 0), None).history_chunk_size(), 1);
        assert_eq!(World::new(version("", -5), None).history_chunk_size(), 1);
    }
}
