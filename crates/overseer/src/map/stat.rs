//! The overview of one room on the world map, from `owner0` map statistics.

use overseer_net::core::api::types::{GameMapStatsRoom, GamePublicUsers};

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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomOwner {
    /// The user id (`2` for invader cores).
    pub id: String,
    /// The username, when the server sent the user.
    pub username: Option<String>,
    /// The controller level (RCL), or the invader core's level.
    pub level: u8,
}

/// The user reserving a room's controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoomReservation {
    /// The user id.
    pub id: String,
    /// The username, when the server sent the user.
    pub username: Option<String>,
}

/// The overview of a room on the world map: status, owner or reservation, safe mode and
/// mineral, from `POST /api/game/map-stats` (`owner0`).
#[derive(Clone, Debug, PartialEq, Eq)]
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
}

impl RoomStat {
    /// A room the server does not know.
    pub(crate) const fn unknown() -> Self {
        Self {
            status: RoomStatus::OutOfBorders,
            status_until: None,
            owner: None,
            reservation: None,
            safe_mode: false,
            mineral: None,
            mineral_density: None,
        }
    }

    /// The overview of `room` at `now` (milliseconds since the Unix epoch), naming users from
    /// the response's `users`.
    pub(crate) fn new(room: &GameMapStatsRoom, users: &GamePublicUsers, now: u64) -> Self {
        let (status, status_until) = status(room, now);
        let username = |id: &str| users.get(id).and_then(|user| user.username.clone());
        let (mut owner, mut reservation) = (None, None);
        if let Some(own) = &room.own
            && let Some(id) = &own.user
        {
            match u8::try_from(own.level) {
                Ok(0) => {
                    reservation = Some(RoomReservation {
                        id: id.clone(),
                        username: username(id),
                    });
                }
                Ok(level) => {
                    owner = Some(RoomOwner {
                        id: id.clone(),
                        username: username(id),
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
        }
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
    use overseer_net::core::api::types::GameMapStatsResponse;

    use super::{RoomStat, RoomStatus};

    const NOW: u64 = 1_000_000;

    fn stat(room: &str, response: &str) -> RoomStat {
        let response: GameMapStatsResponse = serde_json::from_str(response).unwrap();
        RoomStat::new(&response.stats[room], &response.users, NOW)
    }

    #[test]
    fn owners_and_reservations_are_named_from_the_users() {
        let response = r#"{"gameTime":1,"stats":{
            "W1N1":{"status":"normal","own":{"user":"u1","level":8},"safeMode":true,"minerals0":{"type":"H","density":3}},
            "W2N2":{"status":"normal","own":{"user":"u2","level":0}},
            "W3N3":{"status":"normal","own":{"user":"2","level":5}}},
            "users":{"u1":{"_id":"u1","username":"Alice"},"2":{"_id":"2","username":"Invader"}}}"#;
        let controlled = stat("W1N1", response);
        assert_eq!(controlled.status, RoomStatus::Normal);
        let owner = controlled.owner.unwrap();
        assert_eq!(
            (owner.id.as_str(), owner.username.as_deref(), owner.level),
            ("u1", Some("Alice"), 8)
        );
        assert!(controlled.reservation.is_none() && controlled.safe_mode);
        assert_eq!(
            (controlled.mineral.as_deref(), controlled.mineral_density),
            (Some("H"), Some(3))
        );
        let reserved = stat("W2N2", response);
        assert!(reserved.owner.is_none());
        let reservation = reserved.reservation.unwrap();
        assert_eq!(
            (reservation.id.as_str(), reservation.username),
            ("u2", None)
        );
        assert_eq!(
            stat("W3N3", response).owner.unwrap().username.as_deref(),
            Some("Invader")
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
        assert_eq!(RoomStat::unknown().status, RoomStatus::OutOfBorders);
    }
}
