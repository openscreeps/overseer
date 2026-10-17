//! Power creeps: the account's power creep records (not their room objects, which are part of
//! [`world`](super::world)), creation, renaming, upgrades and deletion.

use crate::api::official::types as official;
use crate::api::openscreeps::types as openscreeps;

/// Every contract's power creep record (`GET /api/game/power-creeps/list`), its class and power
/// levels are these types (codegen re-exports the official ones from the others).
pub use crate::api::official::types::{PowerCreep, PowerCreepClassName, PowerCreepPowerLevel};

/// The class of a new power creep in the creation request every contract shares.
impl From<PowerCreepClassName> for official::PowerCreepCreateRequestClassName {
    fn from(class: PowerCreepClassName) -> Self {
        match class {
            PowerCreepClassName::Operator => Self::Operator,
        }
    }
}

/// The new record `POST /api/game/power-creeps/create` answers on `OpenScreeps` (`{ok: 1,
/// ...creep}`); the official and community contracts only acknowledge.
impl From<openscreeps::PowerCreepCreateResponse> for PowerCreep {
    fn from(created: openscreeps::PowerCreepCreateResponse) -> Self {
        Self {
            id: created.id,
            class_name: match created.class_name {
                openscreeps::PowerCreepCreateResponseClassName::Operator => {
                    PowerCreepClassName::Operator
                }
            },
            delete_time: created.delete_time,
            hits_max: created.hits_max,
            level: created.level,
            name: created.name,
            powers: created.powers,
            shard: created.shard,
            spawn_cooldown_time: created.spawn_cooldown_time,
            store: created.store,
            store_capacity: created.store_capacity,
            user: created.user,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PowerCreep, PowerCreepClassName};
    use crate::api::official::types as official;
    use crate::api::openscreeps::types as openscreeps;

    #[test]
    fn openscreeps_creation_reply_is_the_new_record() {
        let reply = r#"{"ok":1,"_id":"pc2","name":"Op2","className":"operator","user":"u1","level":0,"hitsMax":1000,"store":{},"storeCapacity":100,"spawnCooldownTime":0,"powers":{}}"#;
        let created: openscreeps::PowerCreepCreateResponse = serde_json::from_str(reply).unwrap();
        let created = PowerCreep::from(created);
        // The same record as the list holds it.
        let listed: PowerCreep = serde_json::from_str(reply).unwrap();
        assert_eq!(created, listed);
        assert_eq!(created.class_name, PowerCreepClassName::Operator);
        assert_eq!((created.level, created.store_capacity), (0, 100));
    }

    #[test]
    fn creation_requests_name_the_class() {
        let request = official::PowerCreepCreateRequest {
            class_name: PowerCreepClassName::Operator.into(),
            name: "Op".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"className":"operator","name":"Op"}"#
        );
    }
}
