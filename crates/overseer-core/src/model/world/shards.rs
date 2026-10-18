//! The shards of a server (`GET /api/game/shards/info`): screeps.com and its Season server
//! list every shard with its CPU limit, screepsmod-admin-utils lists the one world of a
//! community server with no CPU limit.

use crate::api::official::types as official;
use crate::api::plugins::screepsmod_admin_utils::types as admin_utils;

/// One shard of the server (`GET /api/game/shards/info`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShardInfo {
    /// The shard name.
    pub name: String,
    /// The most CPU per tick a player can use on the shard, when the shard caps it. `None`
    /// when the server reports no cap (absent, or 0 or less: screepsmod-admin-utils always
    /// reports 0).
    pub cpu_limit: Option<f64>,
    /// The average tick duration in milliseconds (`tick`), when reported.
    pub tick: Option<f64>,
}

/// A reported CPU limit as a cap: `None` when absent, 0 or less (or not a number).
fn cpu_cap(reported: Option<f64>) -> Option<f64> {
    reported.filter(|limit| *limit > 0.0)
}

impl From<official::GameShardInfo> for ShardInfo {
    fn from(shard: official::GameShardInfo) -> Self {
        Self {
            name: shard.name,
            cpu_limit: cpu_cap(shard.cpu_limit),
            tick: Some(shard.tick),
        }
    }
}

impl ShardInfo {
    /// The shard of a screepsmod-admin-utils entry. `None` for an entry without a name: the
    /// plugin sends `null` until the server's `shardName` setting is set, and requests name no
    /// shard on a community server.
    #[must_use]
    pub fn from_admin_utils(shard: admin_utils::GameShardInfo) -> Option<Self> {
        Some(Self {
            name: shard.name?,
            cpu_limit: cpu_cap(Some(shard.cpu_limit)),
            tick: shard.tick,
        })
    }
}

/// Every shard of the official reply, in the server's order.
impl From<official::GameShardsInfoResponse> for Vec<ShardInfo> {
    fn from(reply: official::GameShardsInfoResponse) -> Self {
        reply.shards.into_iter().map(ShardInfo::from).collect()
    }
}

/// The named shards of the screepsmod-admin-utils reply, in the server's order; entries
/// without a name are skipped ([`ShardInfo::from_admin_utils`]).
impl From<admin_utils::GameShardsInfoResponse> for Vec<ShardInfo> {
    fn from(reply: admin_utils::GameShardsInfoResponse) -> Self {
        reply
            .shards
            .into_iter()
            .filter_map(ShardInfo::from_admin_utils)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::ShardInfo;
    use crate::api::official::types as official;
    use crate::api::plugins::screepsmod_admin_utils::types as admin_utils;

    /// The shard of `name` with `cpu_limit` and `tick`.
    fn shard(name: &str, cpu_limit: Option<f64>, tick: Option<f64>) -> ShardInfo {
        ShardInfo {
            name: name.to_owned(),
            cpu_limit,
            tick,
        }
    }

    #[test]
    fn official_reply_keeps_caps_and_drops_zero_or_absent_limits() {
        let reply: official::GameShardsInfoResponse = serde_json::from_str(
            r#"{"ok":1,"shards":[
                {"name":"shard3","lastTicks":[3100,3200],"cpuLimit":20,"rooms":4000,"users":900,"tick":3150.5},
                {"name":"shard0","cpuLimit":0,"rooms":30000,"users":1200,"tick":4000},
                {"name":"shard1","rooms":10000,"users":800,"tick":3500}]}"#,
        )
        .unwrap();
        assert_eq!(
            Vec::<ShardInfo>::from(reply),
            [
                shard("shard3", Some(20.0), Some(3150.5)),
                shard("shard0", None, Some(4000.0)),
                shard("shard1", None, Some(3500.0)),
            ]
        );
    }

    #[test]
    fn admin_utils_reply_has_no_cap_and_skips_unnamed_entries() {
        let reply: admin_utils::GameShardsInfoResponse = serde_json::from_str(
            r#"{"ok":1,"shards":[
                {"name":"shardS","cpuLimit":0,"lastTicks":[1000,1200],"rooms":169,"tick":1100},
                {"name":"shardT","cpuLimit":-1,"lastTicks":[],"rooms":169,"tick":null},
                {"name":null,"cpuLimit":0,"lastTicks":[],"rooms":169,"tick":null}]}"#,
        )
        .unwrap();
        assert_eq!(
            Vec::<ShardInfo>::from(reply),
            [
                shard("shardS", None, Some(1100.0)),
                shard("shardT", None, None),
            ]
        );
    }

    #[test]
    fn admin_utils_entry_keeps_a_positive_limit() {
        let entry = admin_utils::GameShardInfo {
            cpu_limit: 50.0,
            name: Some("shard0".to_owned()),
            tick: Some(500.0),
            ..admin_utils::GameShardInfo::default()
        };
        assert_eq!(
            ShardInfo::from_admin_utils(entry),
            Some(shard("shard0", Some(50.0), Some(500.0)))
        );
    }
}
