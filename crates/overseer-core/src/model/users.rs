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

/// `From` the official rooms of a player, grouped by shard.
impl From<official::UserRoomsShardedResponse> for Vec<UserRoom> {
    fn from(reply: official::UserRoomsShardedResponse) -> Self {
        reply
            .shards
            .into_iter()
            .flat_map(|(shard, rooms)| {
                rooms.into_iter().map(move |room| UserRoom {
                    shard: Some(shard.clone()),
                    room,
                })
            })
            .collect()
    }
}

/// `From` the single-world rooms of a player on each private contract.
macro_rules! user_rooms_from {
    ($($reply:ty),+) => {$(
        impl From<$reply> for Vec<UserRoom> {
            fn from(reply: $reply) -> Self {
                reply
                    .rooms
                    .into_iter()
                    .map(|room| UserRoom { shard: None, room })
                    .collect()
            }
        }
    )+};
}

user_rooms_from!(
    openscreeps::UserRoomsFlatResponse,
    community::UserRoomsFlatResponse
);

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
    use super::{LeaderboardPage, LeaderboardResult, PublicProfile, UserRoom};
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

    #[test]
    fn rooms_convert_from_the_sharded_and_the_flat_replies() {
        let room = |shard: Option<&str>, room: &str| UserRoom {
            shard: shard.map(str::to_owned),
            room: room.to_owned(),
        };
        let official: official::UserRoomsShardedResponse = serde_json::from_str(
            r#"{"ok":1,"shards":{"shard0":["W1N1"],"shard3":["W2N2","W3N3"]},
                "reservations":{"shard0":["W1N2"]}}"#,
        )
        .unwrap();
        assert_eq!(
            Vec::<UserRoom>::from(official),
            [
                room(Some("shard0"), "W1N1"),
                room(Some("shard3"), "W2N2"),
                room(Some("shard3"), "W3N3"),
            ]
        );
        let flat = r#"{"ok":1,"rooms":["W2N2","W3N2"]}"#;
        let expected = [room(None, "W2N2"), room(None, "W3N2")];
        let openscreeps: openscreeps::UserRoomsFlatResponse = serde_json::from_str(flat).unwrap();
        assert_eq!(Vec::<UserRoom>::from(openscreeps), expected);
        let community: community::UserRoomsFlatResponse = serde_json::from_str(flat).unwrap();
        assert_eq!(Vec::<UserRoom>::from(community), expected);
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
