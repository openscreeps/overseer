//! The amounts in a store ([`store_amount`], [`store_amounts`]) and its total capacity
//! ([`store_capacity`]), the amount of a dropped resource ([`GameRoomObjectResource::amount`])
//! and of a mineral ([`GameRoomObjectMineral::amount`]), and the upgrade progress a controller
//! needs for its next level ([`GameRoomObjectController::progress_needed`]).

use super::{
    GameRoomObjectController, GameRoomObjectMineral, GameRoomObjectResource, GameRoomObjectStore,
    GameRoomObjectStoreCapacityResource,
};

/// Upgrade progress a controller needs to reach the next level, for levels 1 to 7 (the game's
/// `CONTROLLER_LEVELS`); level 8 is the last.
const CONTROLLER_LEVELS: [u64; 7] = [
    200, 45_000, 135_000, 405_000, 1_215_000, 3_645_000, 10_935_000,
];

/// The amount of `resource` in `store`. A missing, `null` or negative amount gives 0 (unowned
/// NPC market terminals can hold negative amounts).
#[must_use]
pub fn store_amount(store: &GameRoomObjectStore, resource: &str) -> u64 {
    store
        .get(resource)
        .copied()
        .flatten()
        .and_then(|amount| u64::try_from(amount).ok())
        .unwrap_or(0)
}

/// The resources that `store` holds, with their amounts, in resource-name order. The iterator
/// skips `null`, negative and zero amounts.
pub fn store_amounts(store: &GameRoomObjectStore) -> impl Iterator<Item = (&str, u64)> {
    store.iter().filter_map(|(resource, amount)| {
        let amount = u64::try_from((*amount)?).ok()?;
        (amount > 0).then_some((resource.as_str(), amount))
    })
}

/// The sum of the per-resource capacities in `capacity`. `None` when no resource has a
/// capacity (the map is empty or each value is `null`). `Some(0)` when the known capacities
/// are 0.
#[must_use]
pub fn store_capacity(capacity: &GameRoomObjectStoreCapacityResource) -> Option<u64> {
    capacity
        .values()
        .flatten()
        .copied()
        .reduce(u64::saturating_add)
}

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

impl GameRoomObjectMineral {
    /// The amount of the mineral, rounded to the nearest whole number. A negative or NaN
    /// amount gives 0.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "mineral amounts are small non-negative whole numbers"
    )]
    pub fn amount(&self) -> u64 {
        self.mineral_amount.max(0.0).round() as u64
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
    use std::collections::BTreeMap;

    use super::super::{GameRoomObject, GameRoomObjectController, GameRoomObjectMineral};
    use super::{store_amount, store_amounts, store_capacity};

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

    #[test]
    fn a_store_gives_zero_for_missing_null_and_negative_amounts() {
        let store = BTreeMap::from([
            ("energy".to_owned(), Some(50)),
            ("H".to_owned(), None),
            ("O".to_owned(), Some(-3)),
            ("U".to_owned(), Some(0)),
        ]);
        assert_eq!(store_amount(&store, "energy"), 50);
        assert_eq!(store_amount(&store, "H"), 0);
        assert_eq!(store_amount(&store, "O"), 0);
        assert_eq!(store_amount(&store, "power"), 0);
        assert_eq!(store_amounts(&store).collect::<Vec<_>>(), [("energy", 50)]);
    }

    #[test]
    fn a_store_capacity_is_unknown_only_without_values() {
        assert_eq!(store_capacity(&BTreeMap::new()), None);
        assert_eq!(
            store_capacity(&BTreeMap::from([("G".to_owned(), None)])),
            None
        );
        assert_eq!(
            store_capacity(&BTreeMap::from([
                ("G".to_owned(), None),
                ("energy".to_owned(), Some(0))
            ])),
            Some(0)
        );
        assert_eq!(
            store_capacity(&BTreeMap::from([
                ("energy".to_owned(), Some(2000)),
                ("G".to_owned(), Some(5000)),
            ])),
            Some(7000)
        );
    }

    #[test]
    fn a_mineral_amount_is_rounded_and_never_negative() {
        let mineral = |mineral_amount| GameRoomObjectMineral {
            mineral_amount,
            ..GameRoomObjectMineral::default()
        };
        assert_eq!(mineral(1234.6).amount(), 1235);
        assert_eq!(mineral(-5.0).amount(), 0);
        assert_eq!(mineral(f64::NAN).amount(), 0);
    }
}
