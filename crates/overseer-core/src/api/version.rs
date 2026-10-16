//! Server capabilities of [`PlatformVersion`].

use super::types::PlatformVersion;

impl PlatformVersion {
    /// The shard names, empty for a single-world server, whose socket channels and room history
    /// carry no shard.
    ///
    /// A backend-local server (the private server behind screeps-launcher, which reports
    /// `useNativeAuth`) is single-world whatever it lists: screepsmod-admin-utils lists its
    /// `shardName` setting there, `[null]` until one is set, and that name is what
    /// `Game.shard.name` reports, while the socket knows rooms by bare name only.
    #[must_use]
    pub fn shards(&self) -> Vec<String> {
        if self.use_native_auth.is_some() {
            return Vec::new();
        }
        self.server_data
            .shards
            .iter()
            .flatten()
            .flatten()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::api::types::PlatformVersion;

    fn version(json: &str) -> PlatformVersion {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn shards_are_empty_on_single_world_servers() {
        let official = version(
            r#"{"ok":1,"protocol":14,"package":179,"users":1000,
                "serverData":{"customObjectTypes":{},"historyChunkSize":100,
                    "shards":["shard0","shard3"],"features":[],
                    "renderer":{"resources":{},"metadata":{}}}}"#,
        );
        assert_eq!(official.shards(), ["shard0", "shard3"]);
        assert_eq!(official.server_data.history_chunk_size, 100);
        let private = version(
            r#"{"ok":1,"protocol":14,"users":2,
                "serverData":{"customObjectTypes":{},"historyChunkSize":20,
                    "renderer":{"resources":{},"metadata":{}}}}"#,
        );
        assert_eq!(private.shards(), [] as [String; 0]);
        assert_eq!(private.package, None);
    }

    #[test]
    fn a_hosted_server_with_one_shard_is_sharded() {
        // screeps.com/season lists `shardSeason` alone and qualifies its channels with it.
        let season = version(
            r#"{"ok":1,"protocol":14,"package":247,"users":70,
                "serverData":{"customObjectTypes":{},"historyChunkSize":100,
                    "shards":["shardSeason"],"features":[],
                    "renderer":{"resources":{},"metadata":{}}}}"#,
        );
        assert_eq!(season.shards(), ["shardSeason"]);
    }

    #[test]
    fn backend_local_stays_single_world_whatever_it_lists() {
        // screeps-launcher with screepsmod-admin-utils, `shardName: shard0` set…
        let named = version(
            r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,
                "serverData":{"customObjectTypes":{},"historyChunkSize":20,
                    "shards":["shard0"],"features":[{"name":"auth","version":1}],
                    "renderer":{"resources":{},"metadata":{}}}}"#,
        );
        assert_eq!(named.shards(), [] as [String; 0]);
        // …and unset.
        let nameless = version(
            r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,
                "serverData":{"customObjectTypes":{},"historyChunkSize":20,
                    "shards":[null],"renderer":{"resources":{},"metadata":{}}}}"#,
        );
        assert_eq!(nameless.shards(), [] as [String; 0]);
    }
}
