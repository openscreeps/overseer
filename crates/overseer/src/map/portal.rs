//! Portals on the world map: where they lead and when they go away.

use overseer_net::model::world::{GameRoomObject, GameRoomObjectPortal};

use super::RoomCoord;
use super::coord::name_number;

/// Where a [`Portal`] leads.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PortalDestination {
    /// The destination room.
    pub room: RoomCoord,
    /// The destination shard of an inter-shard portal; `None` within the shard.
    pub shard: Option<String>,
    /// The tile it leads to; `None` for inter-shard portals, which name only the room.
    pub x: Option<u8>,
    /// The tile it leads to; `None` for inter-shard portals.
    pub y: Option<u8>,
}

/// One portal tile (`GameRoomObjectPortal`); a portal cluster is several of them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Portal {
    /// The room it stands in.
    pub room: RoomCoord,
    /// Its tile.
    pub x: u8,
    /// Its tile.
    pub y: u8,
    /// Where it leads; `None` while only the room's map view showed it (the destination is
    /// pending, see [`WorldMap`](super::WorldMap)).
    pub destination: Option<PortalDestination>,
    /// When it becomes unstable (`unstableDate`), in milliseconds since the Unix epoch: from
    /// then on it decays and disappears `PORTAL_DECAY` (30 000) ticks later. `None` for
    /// permanent portals and those already decaying.
    pub expires_at: Option<u64>,
    /// The game tick at which a decaying portal disappears (`decayTime`).
    pub decay_time: Option<u64>,
}

impl Portal {
    /// A portal at a tile of `room`'s map view whose destination is not known yet.
    #[must_use]
    pub const fn pending(room: RoomCoord, x: u8, y: u8) -> Self {
        Self {
            room,
            x,
            y,
            destination: None,
            expires_at: None,
            decay_time: None,
        }
    }

    /// The portal of a room object; `None` for other objects and portals whose room,
    /// destination or tile does not parse.
    #[must_use]
    pub fn from_object(object: &GameRoomObject) -> Option<Self> {
        match object {
            GameRoomObject::Portal(portal) => Self::new(portal),
            _ => None,
        }
    }

    /// The portal of a room object document; `None` when its room, destination or tile does
    /// not parse.
    #[must_use]
    pub fn new(portal: &GameRoomObjectPortal) -> Option<Self> {
        let tile = |value: u64| u8::try_from(value).ok().filter(|&value| value < 50);
        let destination = &portal.destination;
        Some(Self {
            room: portal.room.parse().ok()?,
            x: tile(portal.x)?,
            y: tile(portal.y)?,
            destination: Some(PortalDestination {
                room: destination.room.parse().ok()?,
                shard: destination.shard.clone(),
                x: destination.x.and_then(tile),
                y: destination.y.and_then(tile),
            }),
            expires_at: portal.unstable_date,
            decay_time: portal.decay_time,
        })
    }
}

/// Whether `room` is where portals usually stand: sector centres (`W5N5`, `E15S25`:
/// intra-shard pairs on screeps.com and `OpenScreeps`) and highway rooms (a name number ending
/// in 0: inter-shard portals at crossroads such as `W10N0` on screeps.com, intra-shard pairs
/// anywhere on highways such as `W42N0` ↔ `W42N10` on `shard0`). Worlds differ, so
/// [`WorldMap`](super::WorldMap) scans every room; these go first in the background.
#[must_use]
pub const fn may_have_portals(room: RoomCoord) -> bool {
    let (x, y) = (name_number(room.x) % 10, name_number(room.y) % 10);
    (x == 5 && y == 5) || x == 0 || y == 0
}

/// The portals among `objects`, in a stable order whatever order the server sent them in:
/// by tile, row by row.
pub(crate) fn portals_of<'a>(objects: impl IntoIterator<Item = &'a GameRoomObject>) -> Vec<Portal> {
    let mut portals = objects
        .into_iter()
        .filter_map(Portal::from_object)
        .collect::<Vec<_>>();
    portals.sort_unstable_by_key(|portal| (portal.y, portal.x));
    portals
}

#[cfg(test)]
mod tests {
    use overseer_net::model::world::GameRoomObject;

    use super::{Portal, PortalDestination, may_have_portals};
    use crate::map::RoomCoord;

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    #[test]
    fn portals_usually_stand_in_sector_centres_and_on_highways() {
        for name in [
            "W5N5", "E5S5", "W15N25", "E25S5", "W0N0", "E0S0", "W10N0", "E20S10", "E0N10", "W5N0",
            "W0N5", "W42N0", "E31S40", "W11N10",
        ] {
            assert!(may_have_portals(room(name)), "{name}");
        }
        for name in ["W4N5", "E6S5", "W1N1", "E9S9", "W42N1", "E31S41"] {
            assert!(!may_have_portals(room(name)), "{name}");
        }
    }

    #[test]
    fn portals_parse_intra_and_inter_shard_destinations() {
        // Shapes seen on screeps.com shard3 (October 2026).
        let objects: Vec<GameRoomObject> = serde_json::from_str(
            r#"[
            {"_id":"a","type":"portal","room":"W15N15","x":36,"y":4,"unstableDate":1792273804461,
             "destination":{"room":"E25N5","x":32,"y":22}},
            {"_id":"b","type":"portal","room":"W10N10","x":18,"y":8,
             "destination":{"room":"W10N10","shard":"shard2"}},
            {"_id":"c","type":"portal","room":"W5N5","x":10,"y":11,"decayTime":123456,
             "destination":{"room":"W5N5","x":1,"y":2},"disabled":true},
            {"_id":"d","type":"source","room":"W5N5","x":1,"y":1,"energy":0,"energyCapacity":3000,
             "ticksToRegeneration":0}
            ]"#,
        )
        .unwrap();
        let portals = objects
            .iter()
            .filter_map(Portal::from_object)
            .collect::<Vec<_>>();
        assert_eq!(
            portals,
            [
                Portal {
                    room: room("W15N15"),
                    x: 36,
                    y: 4,
                    destination: Some(PortalDestination {
                        room: room("E25N5"),
                        shard: None,
                        x: Some(32),
                        y: Some(22),
                    }),
                    expires_at: Some(1_792_273_804_461),
                    decay_time: None,
                },
                Portal {
                    room: room("W10N10"),
                    x: 18,
                    y: 8,
                    destination: Some(PortalDestination {
                        room: room("W10N10"),
                        shard: Some("shard2".to_owned()),
                        x: None,
                        y: None,
                    }),
                    expires_at: None,
                    decay_time: None,
                },
                Portal {
                    room: room("W5N5"),
                    x: 10,
                    y: 11,
                    destination: Some(PortalDestination {
                        room: room("W5N5"),
                        shard: None,
                        x: Some(1),
                        y: Some(2),
                    }),
                    expires_at: None,
                    decay_time: Some(123_456),
                },
            ]
        );
    }
}
