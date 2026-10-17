//! Socket payloads: the live data of the socket's channels (room updates, map views, map
//! visuals, CPU, resources, code and branch changes, console and message events).
//!
//! The session ([`SocketSession`](crate::socket::SocketSession)) decodes every channel event
//! with the contract of the server it is connected to and converts it into these types, the
//! [`world`](super::world) types (room objects, users, visuals and map views), the
//! [`code`](super::code) and [`console`](super::console) events and the
//! [`messages`](super::messages) feed.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::world::{GameRoomDecoration, RoomUser};
use crate::socket::community::types as community;
use crate::socket::official::types as official;
use crate::socket::openscreeps::types as openscreeps;

/// The `user:<user>/cpu` and `user:<user>/resources` events of every contract are these types
/// (codegen re-exports the official ones from the others).
pub use crate::socket::official::types::{UserCpu, UserCpuCpu, UserResources};

/// One room update (`room:` channel), or one tick of a room's history: what
/// [`RoomState::apply`](crate::socket::RoomState::apply) applies.
///
/// Every property but `objects` is `None` when the update does not carry it, which keeps the
/// room's previous value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomEvent {
    /// Room object changes keyed by object id: `null` removes the object, an unknown id adds
    /// it, a known one is a patch merged into it (see
    /// [`apply_objects_diff`](crate::socket::room::apply_objects_diff)).
    pub objects: Map<String, Value>,
    /// Users not yet sent on this subscription, keyed by user id.
    pub users: Option<BTreeMap<String, RoomUser>>,
    /// The subscribing user's flags in the room (entries joined by `|`, each
    /// `name~color~secondaryColor~x~y`): `Some(None)` when the user has no flags.
    pub flags: Option<Option<String>>,
    /// The room mode (`world` for world rooms): `Some` when the update carries `info`, whose
    /// `mode` may be absent.
    pub mode: Option<Option<String>>,
    /// Game tick of the update.
    pub game_time: Option<u64>,
    /// The subscribing user's room visuals of the previous tick: newline-terminated JSON
    /// lines, each a [`RoomVisualItem`](super::world::RoomVisualItem); empty when none.
    pub visual: Option<String>,
    /// The decorations active in the room; sent by official servers only.
    pub decorations: Option<Vec<GameRoomDecoration>>,
}

impl From<official::RoomEvent> for RoomEvent {
    fn from(event: official::RoomEvent) -> Self {
        Self {
            objects: event.objects,
            users: event.users,
            flags: event.flags,
            mode: event.info.map(|info| info.mode),
            game_time: event.game_time,
            visual: event.visual,
            decorations: event.decorations,
        }
    }
}

/// `From` the room update of the contracts without decorations, `OpenScreeps` and community.
macro_rules! room_event_from {
    ($($event:ty),+) => {$(
        impl From<$event> for RoomEvent {
            fn from(event: $event) -> Self {
                Self {
                    objects: event.objects,
                    users: event.users,
                    flags: event.flags,
                    mode: event.info.map(|info| info.mode),
                    game_time: event.game_time,
                    visual: event.visual,
                    decorations: None,
                }
            }
        }
    )+};
}

room_event_from!(openscreeps::RoomEvent, community::RoomEvent);

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{RoomEvent, UserCpu, UserCpuCpu, UserResources};
    use crate::socket::community::types as community;
    use crate::socket::official::types as official;
    use crate::socket::openscreeps::types as openscreeps;

    #[test]
    fn cpu_is_a_number_or_the_limit_marker() {
        let used: UserCpu = serde_json::from_str(r#"{"cpu":12.4,"memory":754}"#).unwrap();
        assert_eq!(used.cpu, UserCpuCpu::Number(12.4));
        assert_eq!(used.memory, Some(754));
        let start: UserCpu = serde_json::from_str(r#"{"cpu":0}"#).unwrap();
        assert_eq!(start.cpu, UserCpuCpu::Number(0.0));
        let limit: UserCpu = serde_json::from_str(r#"{"cpu":"error"}"#).unwrap();
        assert_eq!(limit.cpu, UserCpuCpu::Error);
        assert!(serde_json::from_str::<UserCpu>(r#"{"cpu":"fail"}"#).is_err());
        assert_eq!(serde_json::to_string(&limit).unwrap(), r#"{"cpu":"error"}"#);
    }

    #[test]
    fn resources_split_credits_from_the_rest() {
        let resources: UserResources =
            serde_json::from_str(r#"{"pixel":120,"credits":15000.5}"#).unwrap();
        assert!((resources.credits - 15_000.5).abs() < f64::EPSILON);
        assert_eq!(resources.other.len(), 1);
        assert!((resources.other["pixel"] - 120.0).abs() < f64::EPSILON);
        assert!(serde_json::from_str::<UserResources>(r#"{"pixel":1}"#).is_err());
    }

    /// The first event of a screeps.com room subscription (`decorations` and a world `info`).
    const OFFICIAL: &str = r#"{"objects":{"5bbcaa":{"_id":"5bbcaa","type":"source","x":4,"y":7,
        "room":"W5N8","energy":3000,"energyCapacity":3000}},
        "users":{"3":{"_id":"3","username":"Source Keeper"}},
        "flags":null,"info":{"mode":"world"},"decorations":[]}"#;

    /// A later event of an `OpenScreeps` or community subscription: no `info`, `visual` and a
    /// tick.
    const STANDALONE: &str = r#"{"objects":{"5bbcaa":{"energy":2990}},"gameTime":72,
        "visual":"{\"t\":\"c\",\"x\":1,\"y\":2}\n"}"#;

    #[test]
    fn room_events_convert_from_every_contract() {
        let event: official::RoomEvent = serde_json::from_str(OFFICIAL).unwrap();
        let event = RoomEvent::from(event);
        assert_eq!(event.decorations, Some(Vec::new()));
        assert_eq!(event.flags, Some(None));
        assert_eq!(event.mode, Some(Some("world".to_owned())));
        assert_eq!(event.game_time, None);
        assert_eq!(event.users.unwrap()["3"].username, "Source Keeper");
        assert_eq!(event.objects["5bbcaa"]["energy"], json!(3000));

        let expected = RoomEvent {
            objects: json!({"5bbcaa": {"energy": 2990}})
                .as_object()
                .unwrap()
                .clone(),
            game_time: Some(72),
            visual: Some("{\"t\":\"c\",\"x\":1,\"y\":2}\n".to_owned()),
            ..RoomEvent::default()
        };
        let event: openscreeps::RoomEvent = serde_json::from_str(STANDALONE).unwrap();
        assert_eq!(RoomEvent::from(event), expected);
        let event: community::RoomEvent = serde_json::from_str(STANDALONE).unwrap();
        assert_eq!(RoomEvent::from(event), expected);

        // `info` without a mode is still sent: it clears the mode.
        let event: community::RoomEvent =
            serde_json::from_str(r#"{"objects":{},"info":{}}"#).unwrap();
        assert_eq!(RoomEvent::from(event).mode, Some(None));
        // Standalone servers send no decorations; their contracts do not read any.
        let event: openscreeps::RoomEvent = serde_json::from_str(OFFICIAL).unwrap();
        assert_eq!(RoomEvent::from(event).decorations, None);
    }
}
