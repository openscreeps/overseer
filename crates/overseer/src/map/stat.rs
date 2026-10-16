//! The overview of one room on the world map, from `owner0` map statistics or a live room.

use overseer_net::core::api::types::{
    GameMapStatsRoom, GamePublicUsers, GameRoomObject, UserBadge,
};

use crate::RoomSnapshot;

/// Whether players can enter a room (`Game.map.getRoomStatus` semantics).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoomStatus {
    /// Open to everyone.
    Normal,
    /// In a novice area until [`RoomStat::status_until`].
    Novice,
    /// In a respawn area until [`RoomStat::status_until`].
    Respawn,
    /// Not open yet; opens at [`RoomStat::status_until`].
    Closed,
    /// Outside the playable world, including rooms the server does not know.
    OutOfBorders,
}

/// The owner of a room's controller, or of an invader core.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomOwner {
    /// The user id (`2` for invader cores).
    pub id: String,
    /// The username, when the server sent the user.
    pub username: Option<String>,
    /// The user's badge parameters, when the server sent them: what
    /// [`Map::badge_svg`](crate::Map::badge_svg) draws, so a changed badge changes them.
    pub badge: Option<UserBadge>,
    /// The controller level (RCL), or the invader core's level.
    pub level: u8,
}

/// The user reserving a room's controller.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomReservation {
    /// The user id.
    pub id: String,
    /// The username, when the server sent the user.
    pub username: Option<String>,
    /// The user's badge parameters, when the server sent them.
    pub badge: Option<UserBadge>,
}

/// The overview of a room on the world map: status, owner or reservation, safe mode and
/// mineral, from `POST /api/game/map-stats` (`owner0`) or, while the room is live, from its
/// objects (see [`WorldMap`](super::WorldMap)).
#[derive(Clone, Debug, PartialEq)]
pub struct RoomStat {
    /// Whether players can enter the room.
    pub status: RoomStatus,
    /// When a novice or respawn area ends or a closed room opens, in milliseconds since the
    /// Unix epoch.
    pub status_until: Option<u64>,
    /// The controller's owner (or an active invader core's).
    pub owner: Option<RoomOwner>,
    /// The controller's reservation.
    pub reservation: Option<RoomReservation>,
    /// Whether the controller's safe mode is active.
    pub safe_mode: bool,
    /// The mineral's resource type, such as `H`.
    pub mineral: Option<String>,
    /// The mineral's density (1 to 4).
    pub mineral_density: Option<u8>,
    /// When the server sent it (or when the live room showed it), in milliseconds since the
    /// Unix epoch ([`SystemTime`](overseer_net::rt::SystemTime)): [`WorldMap::insert_stats`]
    /// uses it to tell how stale a cached overview is.
    ///
    /// [`WorldMap::insert_stats`]: super::WorldMap::insert_stats
    pub fetched_at: u64,
}

impl RoomStat {
    /// A room the server does not know, as of `fetched_at` (milliseconds since the Unix
    /// epoch).
    pub(crate) const fn unknown(fetched_at: u64) -> Self {
        Self {
            status: RoomStatus::OutOfBorders,
            status_until: None,
            owner: None,
            reservation: None,
            safe_mode: false,
            mineral: None,
            mineral_density: None,
            fetched_at,
        }
    }

    /// The overview of `room` at `now` (milliseconds since the Unix epoch, also its
    /// [`Self::fetched_at`]), naming users and their badges from the response's `users`.
    pub(crate) fn new(room: &GameMapStatsRoom, users: &GamePublicUsers, now: u64) -> Self {
        let (status, status_until) = status(room, now);
        let username = |id: &str| users.get(id).and_then(|user| user.username.clone());
        let badge = |id: &str| users.get(id).and_then(|user| user.badge.clone());
        let (mut owner, mut reservation) = (None, None);
        if let Some(own) = &room.own
            && let Some(id) = &own.user
        {
            match u8::try_from(own.level) {
                Ok(0) => {
                    reservation = Some(RoomReservation {
                        id: id.clone(),
                        username: username(id),
                        badge: badge(id),
                    });
                }
                Ok(level) => {
                    owner = Some(RoomOwner {
                        id: id.clone(),
                        username: username(id),
                        badge: badge(id),
                        level,
                    });
                }
                Err(_) => {}
            }
        }
        let mineral = room.minerals0.as_ref();
        Self {
            status,
            status_until,
            owner,
            reservation,
            safe_mode: room.safe_mode == Some(true),
            mineral: mineral.and_then(|mineral| mineral.r#type.clone()),
            mineral_density: mineral
                .and_then(|mineral| mineral.density)
                .and_then(|density| u8::try_from(density).ok()),
            fetched_at: now,
        }
    }

    /// The overview of the live room `snapshot` at `now` (milliseconds since the Unix epoch,
    /// also its [`Self::fetched_at`]): owner and level, reservation and safe mode from its
    /// controller (or the level of an invader core, like map statistics), mineral from its
    /// mineral, users named and badged from the snapshot's users. A live room cannot tell its
    /// status: that of `previous` is kept ([`RoomStatus::Normal`] without one: the room
    /// exists).
    pub(crate) fn live(snapshot: &RoomSnapshot, previous: Option<&Self>, now: u64) -> Self {
        let tick = snapshot.game_time.unwrap_or(0);
        let user = |id: &str| snapshot.users.get(id);
        let username = |id: &str| user(id).map(|user| user.username.clone());
        let badge = |id: &str| user(id).and_then(|user| user.badge.clone());
        let owner = |id: &str, level: u64| {
            Some(RoomOwner {
                id: id.to_owned(),
                username: username(id),
                badge: badge(id),
                level: u8::try_from(level).ok().filter(|&level| level > 0)?,
            })
        };
        let (mut controller_owner, mut core_owner, mut reservation) = (None, None, None);
        let (mut safe_mode, mut mineral) = (false, None);
        for object in &snapshot.objects {
            match object {
                GameRoomObject::Controller(controller) => {
                    controller_owner = controller
                        .user
                        .as_deref()
                        .and_then(|id| owner(id, controller.level));
                    reservation = controller
                        .reservation
                        .as_ref()
                        .filter(|reservation| reservation.end_time > tick)
                        .map(|reservation| RoomReservation {
                            id: reservation.user.clone(),
                            username: username(&reservation.user),
                            badge: badge(&reservation.user),
                        });
                    safe_mode = controller.safe_mode.is_some_and(|end| end > tick);
                }
                GameRoomObject::InvaderCore(core) => {
                    core_owner = core_owner.or_else(|| owner(&core.user, core.level));
                }
                GameRoomObject::Mineral(found) => {
                    mineral = Some((found.mineral_type.clone(), u8::try_from(found.density).ok()));
                }
                _ => {}
            }
        }
        let owner = controller_owner.or(core_owner);
        let (mineral, mineral_density) =
            mineral.map_or((None, None), |(kind, density)| (Some(kind), density));
        Self {
            status: previous.map_or(RoomStatus::Normal, |previous| previous.status),
            status_until: previous.and_then(|previous| previous.status_until),
            reservation: reservation.filter(|_| owner.is_none()),
            owner,
            safe_mode,
            mineral,
            mineral_density,
            fetched_at: now,
        }
    }

    /// Whether `self` and `other` tell the same, whenever they were fetched.
    pub(crate) fn same(&self, other: &Self) -> bool {
        self.status == other.status
            && self.status_until == other.status_until
            && self.owner == other.owner
            && self.reservation == other.reservation
            && self.safe_mode == other.safe_mode
            && self.mineral == other.mineral
            && self.mineral_density == other.mineral_density
    }
}

/// The status at `now` in the engine's order: novice, respawn, not yet open, out of borders.
fn status(room: &GameMapStatsRoom, now: u64) -> (RoomStatus, Option<u64>) {
    let future = |time: Option<f64>| time.and_then(millis).filter(|&time| time > now);
    if let Some(until) = future(room.novice) {
        (RoomStatus::Novice, Some(until))
    } else if let Some(until) = future(room.respawn_area) {
        (RoomStatus::Respawn, Some(until))
    } else if let Some(until) = future(room.open_time) {
        (RoomStatus::Closed, Some(until))
    } else if room.status.as_deref() == Some("out of borders") {
        (RoomStatus::OutOfBorders, None)
    } else {
        (RoomStatus::Normal, None)
    }
}

/// A timestamp in milliseconds as an integer; `None` when negative or not finite.
fn millis(time: f64) -> Option<u64> {
    if !time.is_finite() || time < 0.0 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "finite, non-negative and rounded; larger values saturate"
    )]
    Some(time.round() as u64)
}

#[cfg(test)]
mod tests {
    use overseer_net::core::api::types::{
        GameMapStatsResponse, UserBadgeColor, UserBadgeCustomPaths, UserBadgeType,
    };

    use super::{RoomStat, RoomStatus};

    const NOW: u64 = 1_000_000;

    fn stat(room: &str, response: &str) -> RoomStat {
        let response: GameMapStatsResponse = serde_json::from_str(response).unwrap();
        RoomStat::new(&response.stats[room], &response.users, NOW)
    }

    #[test]
    fn owners_and_reservations_are_named_from_the_users() {
        let response = r##"{"gameTime":1,"stats":{
            "W1N1":{"status":"normal","own":{"user":"u1","level":8},"safeMode":true,"minerals0":{"type":"H","density":3}},
            "W2N2":{"status":"normal","own":{"user":"u2","level":0}},
            "W3N3":{"status":"normal","own":{"user":"2","level":5}},
            "W4N4":{"status":"normal","own":{"user":"u3","level":0}}},
            "users":{"u1":{"_id":"u1","username":"Alice","badge":{"type":19,"color1":"#ff0000","color2":"#00ff00","color3":"#0000ff","param":-42,"flip":true}},
                     "u3":{"_id":"u3","username":"Carol","badge":{"type":{"path1":"M0 0","path2":""},"color1":3,"color2":"#000000","color3":"#ffffff","flip":false,"decoration":"d1"}},
                     "2":{"_id":"2","username":"Invader"}}}"##;
        let controlled = stat("W1N1", response);
        assert_eq!(controlled.status, RoomStatus::Normal);
        let owner = controlled.owner.unwrap();
        assert_eq!(
            (owner.id.as_str(), owner.username.as_deref(), owner.level),
            ("u1", Some("Alice"), 8)
        );
        let badge = owner.badge.unwrap();
        assert!(matches!(badge.r#type, UserBadgeType::Integer(19)));
        assert_eq!(badge.color1, UserBadgeColor::String("#ff0000".to_owned()));
        assert_eq!((badge.param, badge.flip), (Some(-42.0), true));
        assert!(controlled.reservation.is_none() && controlled.safe_mode);
        assert_eq!(
            (controlled.mineral.as_deref(), controlled.mineral_density),
            (Some("H"), Some(3))
        );
        let reserved = stat("W2N2", response);
        assert!(reserved.owner.is_none());
        let reservation = reserved.reservation.unwrap();
        assert_eq!(
            (
                reservation.id.as_str(),
                reservation.username,
                reservation.badge
            ),
            ("u2", None, None)
        );
        let invader = stat("W3N3", response).owner.unwrap();
        assert_eq!(
            (invader.username.as_deref(), invader.badge),
            (Some("Invader"), None)
        );
        let custom = stat("W4N4", response).reservation.unwrap().badge.unwrap();
        assert_eq!(
            custom.r#type,
            UserBadgeType::CustomPaths(UserBadgeCustomPaths {
                path1: "M0 0".to_owned(),
                path2: String::new(),
            })
        );
        assert_eq!(
            (custom.color1, custom.decoration.as_deref()),
            (UserBadgeColor::Integer(3), Some("d1"))
        );
    }

    #[test]
    fn statuses_follow_the_engine_order() {
        let response = r#"{"gameTime":1,"stats":{
            "A":{"status":"normal","novice":2000000,"respawnArea":3000000},
            "B":{"status":"normal","novice":500,"respawnArea":3000000},
            "C":{"status":"out of borders","openTime":2500000},
            "D":{"status":"out of borders","openTime":500},
            "E":{"status":"normal","novice":500}},"users":{}}"#;
        let status = |room| {
            let stat = stat(room, response);
            (stat.status, stat.status_until)
        };
        assert_eq!(status("A"), (RoomStatus::Novice, Some(2_000_000)));
        assert_eq!(status("B"), (RoomStatus::Respawn, Some(3_000_000)));
        assert_eq!(status("C"), (RoomStatus::Closed, Some(2_500_000)));
        assert_eq!(status("D"), (RoomStatus::OutOfBorders, None));
        assert_eq!(status("E"), (RoomStatus::Normal, None));
        assert_eq!(stat("A", response).fetched_at, NOW);
        let unknown = RoomStat::unknown(NOW);
        assert_eq!(
            (unknown.status, unknown.fetched_at),
            (RoomStatus::OutOfBorders, NOW)
        );
    }
}
