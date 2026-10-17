//! Server capabilities of [`PlatformVersion`].

use super::types::PlatformVersion;

impl PlatformVersion {
    /// The shard names the server lists, without unset (`null`) entries.
    ///
    /// Listing a shard does not make channels and requests name it: a private server with
    /// screepsmod-admin-utils lists its `shardName` setting (what `Game.shard.name` reports)
    /// although nothing it serves names a shard. Only official servers do
    /// ([`ServerUrl::is_official`](crate::ServerUrl::is_official)).
    pub fn shards(&self) -> impl Iterator<Item = &str> {
        self.server_data
            .shards
            .iter()
            .flatten()
            .flatten()
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use crate::api::types::PlatformVersion;

    fn shards(version: &PlatformVersion) -> Vec<&str> {
        version.shards().collect()
    }

    #[test]
    fn official_and_stock_private_replies_list_their_shards() {
        let official: PlatformVersion = serde_json::from_str(
            r#"{"ok":1,"protocol":14,"package":179,"users":1000,
                "serverData":{"customObjectTypes":{},"historyChunkSize":100,
                    "shards":["shard0","shard3"],"features":[{"name":"auth","version":1}],
                    "renderer":{"resources":{},"metadata":{}}}}"#,
        )
        .unwrap();
        assert_eq!(shards(&official), ["shard0", "shard3"]);
        assert_eq!(official.server_data.history_chunk_size, 100);
        let private: PlatformVersion = serde_json::from_str(
            r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":2,
                "serverData":{"customObjectTypes":{},"historyChunkSize":20,
                    "renderer":{"resources":{},"metadata":{}}}}"#,
        )
        .unwrap();
        assert_eq!(shards(&private), [] as [&str; 0]);
        assert_eq!(private.package, None);
    }

    #[test]
    fn community_mod_replies_decode_with_their_feature_versions_and_shard_name() {
        // screepsmod-admin-utils' `/api/version` with screepsmod-auth: mod features carry their
        // package version or none, and `shardName` is `null` until it is set.
        let reply = |shard: &str| {
            format!(
                r#"{{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,
                "serverData":{{"shards":[{shard}],"customObjectTypes":{{}},"historyChunkSize":20,
                    "socketUpdateThrottle":1000,"renderer":{{"resources":{{}},"metadata":{{}}}},
                    "features":[{{"name":"auth","version":1}},{{"name":"auth-tokens"}},
                        {{"name":"screepsmod-auth","version":"2.10.1","authTypes":["password"]}},
                        {{"name":"screepsmod-admin-utils","version":"1.37.1"}}]}}}}"#
            )
        };
        let unset: PlatformVersion = serde_json::from_str(&reply("null")).unwrap();
        assert_eq!(shards(&unset), [] as [&str; 0]);
        let named: PlatformVersion = serde_json::from_str(&reply(r#""shard0""#)).unwrap();
        assert_eq!(shards(&named), ["shard0"]);
        assert_eq!(
            named.server_data.features.map(|features| features.len()),
            Some(4)
        );
    }
}
