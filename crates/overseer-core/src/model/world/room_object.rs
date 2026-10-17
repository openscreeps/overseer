//! The amount of a dropped resource ([`GameRoomObjectResource::amount`]) and the upgrade
//! progress a controller needs for its next level ([`GameRoomObjectController::progress_needed`]).

use super::{GameRoomObjectController, GameRoomObjectResource};

/// Upgrade progress a controller needs to reach the next level, for levels 1 to 7 (the game's
/// `CONTROLLER_LEVELS`); level 8 is the last.
const CONTROLLER_LEVELS: [u64; 7] = [
    200, 45_000, 135_000, 405_000, 1_215_000, 3_645_000, 10_935_000,
];

impl GameRoomObjectResource {
    /// The amount of the dropped resource: the value stored under its resource type (the
    /// `resourceType` property, or `type` for a pile created without one); 0 when absent.
    #[must_use]
    pub fn amount(&self) -> u64 {
        let key = self
            .resource_type
            .map_or(self.r#type.as_str(), |resource| resource.as_str());
        self.other.get(key).copied().unwrap_or(0)
    }
}

impl GameRoomObjectController {
    /// The upgrade progress needed for the next level: `progressTotal` when the object
    /// carries it, else the game's value for its level, as the game API derives it (room
    /// objects of the official and private servers store only `progress` and `level`).
    /// `None` for an unowned controller (level 0) and at level 8.
    #[must_use]
    pub fn progress_needed(&self) -> Option<u64> {
        self.progress_total.filter(|&total| total > 0).or_else(|| {
            let index = usize::try_from(self.level).ok()?.checked_sub(1)?;
            CONTROLLER_LEVELS.get(index).copied()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{GameRoomObject, GameRoomObjectController};

    fn controller(level: u64, progress_total: Option<u64>) -> GameRoomObjectController {
        GameRoomObjectController {
            level,
            progress_total,
            ..GameRoomObjectController::default()
        }
    }

    #[test]
    fn progress_needed_comes_from_the_level_unless_the_object_carries_it() {
        assert_eq!(controller(1, None).progress_needed(), Some(200));
        assert_eq!(controller(7, None).progress_needed(), Some(10_935_000));
        assert_eq!(controller(8, None).progress_needed(), None);
        assert_eq!(controller(0, None).progress_needed(), None);
        assert_eq!(controller(3, Some(1234)).progress_needed(), Some(1234));
        assert_eq!(controller(3, Some(0)).progress_needed(), Some(135_000));
    }

    #[test]
    fn a_dropped_resource_amount_is_stored_under_its_resource_type() {
        let pile = |json: &str| match serde_json::from_str(json).unwrap() {
            GameRoomObject::Resource(pile) => pile.amount(),
            other => panic!("not a resource: {other:?}"),
        };
        assert_eq!(
            pile(
                r#"{"_id":"r1","type":"energy","room":"W1N1","x":1,"y":2,
                    "resourceType":"energy","energy":75}"#
            ),
            75
        );
        assert_eq!(
            pile(r#"{"_id":"r2","type":"H","room":"W1N1","x":1,"y":2,"resourceType":"H"}"#),
            0
        );
    }
}
