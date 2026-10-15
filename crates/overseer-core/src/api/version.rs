//! Server capabilities of [`PlatformVersion`].

use super::types::PlatformVersion;

impl PlatformVersion {
    /// The shard names; empty for a single-world server, whose socket channels carry no shard.
    #[must_use]
    pub fn shards(&self) -> &[String] {
        self.server_data.shards.as_deref().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use crate::api::types::PlatformVersion;

    #[test]
    fn shards_are_empty_on_single_world_servers() {
        let official: PlatformVersion = serde_json::from_str(
            r#"{"ok":1,"protocol":14,"package":179,"users":1000,
                "serverData":{"customObjectTypes":{},"historyChunkSize":100,
                    "shards":["shard0","shard3"],"features":[],
                    "renderer":{"resources":{},"metadata":{}}}}"#,
        )
        .unwrap();
        assert_eq!(official.shards(), ["shard0", "shard3"]);
        assert_eq!(official.server_data.history_chunk_size, 100);
        let private: PlatformVersion = serde_json::from_str(
            r#"{"ok":1,"protocol":14,"users":2,
                "serverData":{"customObjectTypes":{},"historyChunkSize":20,
                    "renderer":{"resources":{},"metadata":{}}}}"#,
        )
        .unwrap();
        assert_eq!(private.shards(), [] as [String; 0]);
        assert_eq!(private.package, None);
    }
}
