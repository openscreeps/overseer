//! World-map statistics (`POST /api/game/map-stats`). The contracts differ in the ranking of
//! the requested statistic: screeps.com sends one `{user, value}` entry per room, `OpenScreeps`
//! a list sorted by value.

use std::collections::BTreeMap;

use super::{GameControllerSign, GameMapStatEntry, GamePublicUsers};
use crate::api::{official, openscreeps};

/// Map statistics of some rooms (`POST /api/game/map-stats`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapStats {
    /// The current game tick.
    pub game_time: i64,
    /// Statistics by room name; requested rooms the server does not know are absent.
    pub stats: BTreeMap<String, MapStatsRoom>,
    /// The largest ranked value by statistic name (for example `energyHarvested8`), which the
    /// official world map scales its circles by; empty for `owner0`.
    pub stats_max: BTreeMap<String, f64>,
    /// The users the statistics reference, by user id.
    pub users: GamePublicUsers,
}

/// The map statistics of one room.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapStatsRoom {
    /// The room status, for example `normal` or `out of borders`.
    pub status: Option<String>,
    /// Until when the room is a novice area, in milliseconds since the Unix epoch.
    pub novice: Option<f64>,
    /// Until when the room is a respawn area, in milliseconds since the Unix epoch.
    pub respawn_area: Option<f64>,
    /// When the room opens, in milliseconds since the Unix epoch.
    pub open_time: Option<f64>,
    /// The controller's owner (level 1 to 8), its reserver (level 0) or an invader core.
    pub own: Option<MapStatsOwner>,
    /// Whether the controller's safe mode is active (`true` when present).
    pub safe_mode: Option<bool>,
    /// The controller's sign.
    pub sign: Option<GameControllerSign>,
    /// The system sign (novice and respawn areas).
    pub hard_sign: Option<GameControllerSign>,
    /// Whether the controller has power enabled (screeps.com).
    pub is_power_enabled: Option<bool>,
    /// The room's mineral.
    pub mineral: Option<MapStatsMineral>,
    /// The ranking of the requested statistic by its name (for example `energyHarvested8`),
    /// highest value first: one entry on screeps.com, every user on `OpenScreeps`.
    pub ranking: BTreeMap<String, Vec<GameMapStatEntry>>,
}

/// The owner of a room on the world map: the controller's owner or reserver, or an invader
/// core.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapStatsOwner {
    /// The user id; `None` when the server sent none.
    pub user: Option<String>,
    /// The controller level (RCL), 0 for a reservation, or the invader core's level.
    pub level: i64,
}

/// The mineral of a room on the world map.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapStatsMineral {
    /// The mineral's resource type, such as `H`.
    pub kind: Option<String>,
    /// The mineral's density (1 to 4).
    pub density: Option<i64>,
}

/// `From` the map-statistics reply of each contract; `$ranking` turns a room's ranking value
/// into its entries. The community contract shares the official reply.
macro_rules! map_stats_from {
    ($($contract:ident => $ranking:expr),* $(,)?) => {
        $(
            impl From<$contract::types::GameMapStatsResponse> for MapStats {
                fn from(reply: $contract::types::GameMapStatsResponse) -> Self {
                    Self {
                        game_time: reply.game_time,
                        stats: reply
                            .stats
                            .into_iter()
                            .map(|(room, stats)| (room, MapStatsRoom::from(stats)))
                            .collect(),
                        stats_max: reply.stats_max.unwrap_or_default(),
                        users: reply.users,
                    }
                }
            }

            impl From<$contract::types::GameMapStatsRoom> for MapStatsRoom {
                fn from(room: $contract::types::GameMapStatsRoom) -> Self {
                    Self {
                        status: room.status,
                        novice: room.novice,
                        respawn_area: room.respawn_area,
                        open_time: room.open_time,
                        own: room.own.map(|own| MapStatsOwner {
                            user: own.user,
                            level: own.level,
                        }),
                        safe_mode: room.safe_mode,
                        sign: room.sign,
                        hard_sign: room.hard_sign,
                        is_power_enabled: room.is_power_enabled,
                        mineral: room.minerals0.map(|mineral| MapStatsMineral {
                            kind: mineral.r#type,
                            density: mineral.density,
                        }),
                        ranking: room
                            .other
                            .into_iter()
                            .map(|(name, ranking)| (name, $ranking(ranking)))
                            .collect(),
                    }
                }
            }
        )*
    };
}

map_stats_from!(
    official => |entry: GameMapStatEntry| vec![entry],
    openscreeps => |entries: Vec<GameMapStatEntry>| entries,
);

#[cfg(test)]
mod tests {
    use super::{MapStats, MapStatsMineral, MapStatsOwner};
    use crate::api::{community, official, openscreeps};

    /// screeps.com's reply to `energyHarvested8`: one ranking entry per room.
    const OFFICIAL: &str = r##"{"ok":1,"gameTime":123,"stats":{
        "W1N1":{"status":"normal","novice":null,"openTime":1500000000000,
            "own":{"user":"u1","level":8},"safeMode":true,"isPowerEnabled":true,
            "minerals0":{"type":"H","density":3},
            "sign":{"user":"u1","text":"hi","time":5,"datetime":1600000000000},
            "energyHarvested8":{"user":"u1","value":250}}},
        "statsMax":{"energyHarvested8":250},
        "users":{"u1":{"_id":"u1","username":"Alice","badge":{"type":1,"color1":"#000000","color2":"#111111","color3":"#222222","param":0,"flip":false}}}}"##;

    /// `OpenScreeps`' reply to `energyHarvested8`: every user's value, highest first.
    const OPENSCREEPS: &str = r#"{"ok":1,"gameTime":123,"stats":{
        "W1N1":{"status":"normal","openTime":1500000000000,"own":{"user":"u1","level":8},
            "safeMode":true,"minerals0":{"type":"H","density":3},
            "energyHarvested8":[{"user":"u1","value":250},{"user":"u2","value":10}]}},
        "statsMax":{"energyHarvested8":250},
        "users":{"u1":{"_id":"u1","username":"Alice"},"u2":{"_id":"u2","username":"Bob"}}}"#;

    #[test]
    fn official_rankings_become_one_entry() {
        let reply: official::types::GameMapStatsResponse = serde_json::from_str(OFFICIAL).unwrap();
        let stats = MapStats::from(reply);
        assert_eq!(stats.game_time, 123);
        assert_eq!(stats.stats_max["energyHarvested8"], 250.0);
        let room = &stats.stats["W1N1"];
        assert_eq!(room.status.as_deref(), Some("normal"));
        assert_eq!(room.open_time, Some(1_500_000_000_000.0));
        assert_eq!(
            room.own,
            Some(MapStatsOwner {
                user: Some("u1".to_owned()),
                level: 8
            })
        );
        assert_eq!(
            room.mineral,
            Some(MapStatsMineral {
                kind: Some("H".to_owned()),
                density: Some(3)
            })
        );
        assert_eq!(
            (room.safe_mode, room.is_power_enabled),
            (Some(true), Some(true))
        );
        assert_eq!(
            room.sign.as_ref().map(|sign| sign.text.as_str()),
            Some("hi")
        );
        let ranking = &room.ranking["energyHarvested8"];
        assert_eq!(ranking.len(), 1);
        assert_eq!((ranking[0].user.as_str(), ranking[0].value), ("u1", 250.0));
        assert_eq!(stats.users["u1"].username.as_deref(), Some("Alice"));
        // The community contract shares the official reply.
        let community: community::types::GameMapStatsResponse =
            serde_json::from_str(OFFICIAL).unwrap();
        assert_eq!(MapStats::from(community), stats);
    }

    #[test]
    fn openscreeps_rankings_keep_every_entry() {
        let reply: openscreeps::types::GameMapStatsResponse =
            serde_json::from_str(OPENSCREEPS).unwrap();
        let stats = MapStats::from(reply);
        let room = &stats.stats["W1N1"];
        assert_eq!(room.own.as_ref().map(|own| own.level), Some(8));
        assert_eq!(room.is_power_enabled, None);
        let ranking = &room.ranking["energyHarvested8"];
        let users: Vec<_> = ranking.iter().map(|entry| entry.user.as_str()).collect();
        assert_eq!(users, ["u1", "u2"]);
        assert_eq!(stats.users.len(), 2);
    }

    #[test]
    fn owner_requests_have_no_ranking() {
        let reply: openscreeps::types::GameMapStatsResponse = serde_json::from_str(
            r#"{"ok":1,"gameTime":1,"stats":{"W2N2":{"status":"out of borders"}},"users":{}}"#,
        )
        .unwrap();
        let stats = MapStats::from(reply);
        assert!(stats.stats_max.is_empty());
        assert!(stats.stats["W2N2"].ranking.is_empty());
        assert!(stats.stats["W2N2"].own.is_none());
    }
}
