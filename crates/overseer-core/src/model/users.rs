//! Users: other players' public profiles, rooms and statistics, and the leaderboards. Badges
//! are [`UserBadge`]s. The game's non-player characters are [`Npc`]s; the levels of a profile's
//! points are [`LevelProgress`]es.

mod levels;
mod npc;

use std::collections::BTreeMap;

pub use levels::{GCL_MULTIPLY, GCL_POW, LevelProgress, POWER_LEVEL_MULTIPLY, POWER_LEVEL_POW};
pub use npc::Npc;

use crate::api::community::types as community;
use crate::api::official::types as official;
use crate::api::openscreeps::types as openscreeps;
use crate::model::account::UserBadge;

/// Every contract's leaderboard mode, ranking entry and season are these types (codegen
/// re-exports the official ones from the others).
pub use crate::api::official::types::{LeaderboardMode, LeaderboardResult, LeaderboardSeason};

/// A player's public profile (`GET /api/user/find`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PublicProfile {
    /// The user's id (`_id`).
    pub id: String,
    /// The username; `None` until the account has chosen one.
    pub username: Option<String>,
    /// The badge, once chosen.
    pub badge: Option<UserBadge>,
    /// Lifetime control points (GCL progress), when reported.
    pub gcl: Option<f64>,
    /// Lifetime power processed (GPL progress), when reported.
    pub power: Option<f64>,
    /// The linked Steam account's id, unless the player hides it.
    pub steam_id: Option<String>,
}

/// `From` the official public profile (`OpenScreeps` shares it).
impl From<official::UserPublicProfile> for PublicProfile {
    fn from(user: official::UserPublicProfile) -> Self {
        Self {
            id: user.id,
            username: user.username,
            badge: user.badge,
            gcl: user.gcl,
            power: user.power,
            steam_id: user.steam.map(|steam| steam.id),
        }
    }
}

/// `From` the community public profile, whose Steam account may lack an id.
impl From<community::UserPublicProfile> for PublicProfile {
    fn from(user: community::UserPublicProfile) -> Self {
        Self {
            id: user.id,
            username: user.username,
            badge: user.badge,
            gcl: user.gcl,
            power: user.power,
            steam_id: user.steam.and_then(|steam| steam.id),
        }
    }
}

/// A room whose controller a player owns (`GET /api/user/rooms`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserRoom {
    /// The room's shard; `None` on single-world servers.
    pub shard: Option<String>,
    /// The room name.
    pub room: String,
}

/// The rooms that a player holds, as the server reports them (`GET /api/user/rooms`): the
/// rooms whose controller the player owns and, on sharded servers, the rooms whose controller
/// the player reserves.
///
/// The maps keep the difference between an empty list and a missing key:
///
/// - A shard with an empty list: the server reports no room on the shard.
/// - A shard that is not in the map: the reply has no report for the shard.
///
/// The holdings are not proof that the player has no creeps or other objects on a shard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomHoldings {
    /// The reply of a sharded server (screeps.com).
    Sharded {
        /// The owned rooms, keyed by shard (`shards`).
        owned: BTreeMap<String, Vec<String>>,
        /// The reserved rooms, keyed by shard (`reservations`). `None` when the reply has no
        /// `reservations` map: then the reply has no report of reserved rooms. The web client
        /// asks for the map with the `reservation` flag.
        reserved: Option<BTreeMap<String, Vec<String>>>,
    },
    /// The reply of a single-world server (`OpenScreeps` and community servers). These
    /// servers do not report reserved rooms. The reply is no report about a named shard.
    SingleWorld {
        /// The owned rooms, highest controller level first.
        owned: Vec<String>,
    },
}

/// `From` the official rooms of a player, grouped by shard.
impl From<official::UserRoomsShardedResponse> for RoomHoldings {
    fn from(reply: official::UserRoomsShardedResponse) -> Self {
        Self::Sharded {
            owned: reply.shards,
            reserved: reply.reservations,
        }
    }
}

/// `From` the single-world rooms of a player on each private contract.
macro_rules! room_holdings_from {
    ($($reply:ty),+) => {$(
        impl From<$reply> for RoomHoldings {
            fn from(reply: $reply) -> Self {
                Self::SingleWorld { owned: reply.rooms }
            }
        }
    )+};
}

room_holdings_from!(
    openscreeps::UserRoomsFlatResponse,
    community::UserRoomsFlatResponse
);

/// `From` the holdings into the owned rooms, with their shard on sharded servers. The list
/// drops the reserved rooms, and it does not show the shards without owned rooms.
impl From<RoomHoldings> for Vec<UserRoom> {
    fn from(holdings: RoomHoldings) -> Self {
        match holdings {
            RoomHoldings::Sharded { owned, .. } => owned
                .into_iter()
                .flat_map(|(shard, rooms)| {
                    rooms.into_iter().map(move |room| UserRoom {
                        shard: Some(shard.clone()),
                        room,
                    })
                })
                .collect(),
            RoomHoldings::SingleWorld { owned } => owned
                .into_iter()
                .map(|room| UserRoom { shard: None, room })
                .collect(),
        }
    }
}

/// A page of a leaderboard (`GET /api/leaderboard/list`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeaderboardPage {
    /// How many players the season ranks.
    pub count: u64,
    /// The page's positions.
    pub list: Vec<LeaderboardResult>,
    /// The listed players, by id.
    pub users: BTreeMap<String, LeaderboardUser>,
}

/// A player listed on a [`LeaderboardPage`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeaderboardUser {
    /// The user's id (`_id`).
    pub id: String,
    /// The username, when set.
    pub username: Option<String>,
    /// The badge, once chosen.
    pub badge: Option<UserBadge>,
    /// Lifetime control points, when reported (`OpenScreeps`).
    pub gcl: Option<f64>,
}

/// `From` the official leaderboard page (the community contract shares it).
impl From<official::LeaderboardListResponse> for LeaderboardPage {
    fn from(page: official::LeaderboardListResponse) -> Self {
        Self {
            count: page.count,
            list: page.list,
            users: page
                .users
                .into_iter()
                .map(|(id, user)| {
                    let user = LeaderboardUser {
                        id: user.id,
                        username: user.username,
                        badge: user.badge,
                        gcl: None,
                    };
                    (id, user)
                })
                .collect(),
        }
    }
}

/// `From` the `OpenScreeps` leaderboard page, whose users carry their GCL.
impl From<openscreeps::LeaderboardListResponse> for LeaderboardPage {
    fn from(page: openscreeps::LeaderboardListResponse) -> Self {
        Self {
            count: page.count,
            list: page.list,
            users: page
                .users
                .into_iter()
                .map(|(id, user)| {
                    let user = LeaderboardUser {
                        id: user.id,
                        username: user.username,
                        badge: user.badge,
                        gcl: user.gcl,
                    };
                    (id, user)
                })
                .collect(),
        }
    }
}

/// `From` the `GET /api/leaderboard/find` reply every contract shares into the player's
/// rankings: the one of the asked season, or one per season.
impl From<official::LeaderboardFindResponse> for Vec<LeaderboardResult> {
    fn from(reply: official::LeaderboardFindResponse) -> Self {
        match reply {
            official::LeaderboardFindResponse::Season(rank) => vec![LeaderboardResult {
                id: rank.id,
                rank: rank.rank,
                score: rank.score,
                season: rank.season,
                user: rank.user,
            }],
            official::LeaderboardFindResponse::All(all) => all.list,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{LeaderboardPage, LeaderboardResult, PublicProfile, RoomHoldings, UserRoom};
    use crate::api::community::types as community;
    use crate::api::official::types as official;
    use crate::api::openscreeps::types as openscreeps;

    #[test]
    fn public_profiles_convert_from_both_shapes() {
        let official: official::UserPublicProfile = serde_json::from_str(
            r#"{"_id":"u2","username":"Bob","gcl":300,"steam":{"id":"7656"}}"#,
        )
        .unwrap();
        let profile = PublicProfile::from(official);
        assert_eq!(profile.id, "u2");
        assert_eq!(profile.username.as_deref(), Some("Bob"));
        assert_eq!(profile.gcl, Some(300.0));
        assert_eq!(profile.steam_id.as_deref(), Some("7656"));
        // A community user without a Steam account has `steam: {}`.
        let community: community::UserPublicProfile =
            serde_json::from_str(r#"{"_id":"u2","username":"Bob","steam":{}}"#).unwrap();
        let profile = PublicProfile::from(community);
        assert_eq!(profile.steam_id, None);
        assert_eq!(profile.username.as_deref(), Some("Bob"));
    }

    /// The shard map of `entries`.
    fn shards(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        entries
            .iter()
            .map(|(shard, rooms)| {
                let rooms = rooms.iter().map(|&room| room.to_owned()).collect();
                ((*shard).to_owned(), rooms)
            })
            .collect()
    }

    fn sharded(json: &str) -> RoomHoldings {
        let reply: official::UserRoomsShardedResponse = serde_json::from_str(json).unwrap();
        RoomHoldings::from(reply)
    }

    #[test]
    fn sharded_holdings_keep_empty_shards_apart_from_absent_ones() {
        // `shard1` is absent; `shard3` owns nothing, and `shard0` reserves nothing.
        let holdings = sharded(
            r#"{"ok":1,"shards":{"shard0":["W1N1"],"shard3":[]},
                "reservations":{"shard0":[],"shard2":["W5N5"]}}"#,
        );
        assert_eq!(
            holdings,
            RoomHoldings::Sharded {
                owned: shards(&[("shard0", &["W1N1"]), ("shard3", &[])]),
                reserved: Some(shards(&[("shard0", &[]), ("shard2", &["W5N5"])])),
            }
        );
        // The owned-room list keeps neither the reservations nor the empty shard.
        assert_eq!(
            Vec::<UserRoom>::from(holdings),
            [UserRoom {
                shard: Some("shard0".to_owned()),
                room: "W1N1".to_owned(),
            }]
        );
    }

    #[test]
    fn reservations_are_kept_without_owned_rooms_and_absent_without_the_map() {
        let holdings = sharded(r#"{"ok":1,"shards":{},"reservations":{"shard0":["W1N2"]}}"#);
        assert_eq!(
            holdings,
            RoomHoldings::Sharded {
                owned: BTreeMap::new(),
                reserved: Some(shards(&[("shard0", &["W1N2"])])),
            }
        );
        assert_eq!(Vec::<UserRoom>::from(holdings), Vec::new());
        // A reply without `reservations` has no report of reserved rooms, not an empty one.
        assert_eq!(
            sharded(r#"{"ok":1,"shards":{"shard0":[]}}"#),
            RoomHoldings::Sharded {
                owned: shards(&[("shard0", &[])]),
                reserved: None,
            }
        );
    }

    #[test]
    fn single_world_holdings_keep_the_order_and_have_no_shard() {
        let flat = r#"{"ok":1,"rooms":["W3N2","W2N2"]}"#;
        let expected = RoomHoldings::SingleWorld {
            owned: vec!["W3N2".to_owned(), "W2N2".to_owned()],
        };
        let openscreeps: openscreeps::UserRoomsFlatResponse = serde_json::from_str(flat).unwrap();
        assert_eq!(RoomHoldings::from(openscreeps), expected);
        let community: community::UserRoomsFlatResponse = serde_json::from_str(flat).unwrap();
        assert_eq!(RoomHoldings::from(community), expected);
        let rooms: Vec<_> = Vec::<UserRoom>::from(expected)
            .into_iter()
            .map(|room| (room.shard, room.room))
            .collect();
        assert_eq!(
            rooms,
            [(None, "W3N2".to_owned()), (None, "W2N2".to_owned())]
        );
    }

    #[test]
    fn leaderboards_convert_from_every_contract() {
        let page = r#"{"ok":1,"count":31,
            "list":[{"_id":"r1","rank":20,"score":5000,"season":"2026-10","user":"u2"}],
            "users":{"u2":{"_id":"u2","username":"Bob","gcl":1200}}}"#;
        let official: official::LeaderboardListResponse = serde_json::from_str(page).unwrap();
        let official = LeaderboardPage::from(official);
        assert_eq!(official.count, 31);
        assert_eq!(official.list[0].rank, 20);
        assert_eq!(official.users["u2"].username.as_deref(), Some("Bob"));
        assert_eq!(official.users["u2"].gcl, None);
        let openscreeps: openscreeps::LeaderboardListResponse = serde_json::from_str(page).unwrap();
        assert_eq!(
            LeaderboardPage::from(openscreeps).users["u2"].gcl,
            Some(1200.0)
        );

        let rank = r#"{"_id":"r1","rank":20,"score":5000,"season":"2026-10","user":"u2"}"#;
        let expected = LeaderboardResult {
            id: "r1".to_owned(),
            rank: 20,
            score: 5000.0,
            season: "2026-10".to_owned(),
            user: "u2".to_owned(),
        };
        let season: official::LeaderboardFindResponse = serde_json::from_str(rank).unwrap();
        assert_eq!(
            Vec::<LeaderboardResult>::from(season),
            std::slice::from_ref(&expected)
        );
        let all = format!(r#"{{"ok":1,"list":[{rank},{rank}]}}"#);
        let openscreeps: openscreeps::LeaderboardFindResponse = serde_json::from_str(&all).unwrap();
        assert_eq!(Vec::<LeaderboardResult>::from(openscreeps).len(), 2);
        let community: community::LeaderboardFindResponse = serde_json::from_str(&all).unwrap();
        assert_eq!(
            Vec::<LeaderboardResult>::from(community),
            [expected.clone(), expected]
        );
    }
}
