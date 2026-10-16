//! Portals as the app shows and keeps them: where a room's portals lead
//! ([`destinations`], one entry per destination however many portal tiles lead there), where
//! "Go to" takes the user ([`GoTo`]), and the cache record of a scanned room
//! ([`portals_json`], read by [`parse_portals`]).
//!
//! The world map scans rooms for portals (`WorldMap`, `Interest::portals`); the app persists
//! each scanned room with its scan time (`terrain_cache`) and seeds the next session with
//! `WorldMap::insert_portals`, which rescans once the record is stale.

use overseer::map::{Portal, PortalDestination, RoomCoord};
use serde::{Deserialize, Serialize};

/// Version of the portal records ([`portals_json`]).
pub const PORTAL_VERSION: u64 = 1;

/// A portal record as stored: version, scan time, portals.
#[derive(Serialize, Deserialize)]
struct Record {
    v: u64,
    at: u64,
    portals: Vec<Entry>,
}

/// A portal of a [`Record`]: its tile, and its destination's room (none: pending), shard and
/// tile, when it starts to decay (`exp`, milliseconds since the epoch) and the tick it
/// disappears.
#[derive(Serialize, Deserialize)]
struct Entry {
    x: u8,
    y: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shard: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tx: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ty: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exp: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    decay: Option<u64>,
}

/// The cache record of a room's `portals` scanned at `scanned_at` (milliseconds since the
/// epoch); an empty list records a room without portals.
pub fn portals_json(portals: &[Portal], scanned_at: u64) -> String {
    let record = Record {
        v: PORTAL_VERSION,
        at: scanned_at,
        portals: portals
            .iter()
            .map(|portal| {
                let destination = portal.destination.as_ref();
                Entry {
                    x: portal.x,
                    y: portal.y,
                    to: destination.map(|destination| destination.room.to_string()),
                    shard: destination.and_then(|destination| destination.shard.clone()),
                    tx: destination.and_then(|destination| destination.x),
                    ty: destination.and_then(|destination| destination.y),
                    exp: portal.expires_at,
                    decay: portal.decay_time,
                }
            })
            .collect(),
    };
    serde_json::to_string(&record).unwrap_or_default()
}

/// Reads the [`portals_json`] record of `room`: its portals and scan time; `None` when it is
/// malformed or of another [`PORTAL_VERSION`].
pub fn parse_portals(record: &str, room: RoomCoord) -> Option<(Vec<Portal>, u64)> {
    let record: Record = serde_json::from_str(record).ok()?;
    if record.v != PORTAL_VERSION {
        return None;
    }
    let portals = record
        .portals
        .into_iter()
        .map(|entry| {
            let tiles = [Some(entry.x), Some(entry.y), entry.tx, entry.ty];
            if tiles.into_iter().flatten().any(|tile| tile >= 50) {
                return None;
            }
            let destination = match entry.to {
                Some(to) => Some(PortalDestination {
                    room: to.parse().ok()?,
                    shard: entry.shard,
                    x: entry.tx,
                    y: entry.ty,
                }),
                None => None,
            };
            Some(Portal {
                room,
                x: entry.x,
                y: entry.y,
                destination,
                expires_at: entry.exp,
                decay_time: entry.decay,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((portals, record.at))
}

/// Where a room's portals lead: one entry per destination room (and shard), and one for the
/// portals whose destination is not known yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Destination {
    /// Where they lead (the tile is the first portal's); `None`: not known yet.
    pub to: Option<PortalDestination>,
    /// Portal tiles leading there.
    pub tiles: usize,
    /// The earliest time one of them starts to decay, in milliseconds since the epoch.
    pub expires_at: Option<u64>,
    /// The earliest game tick at which one of them disappears.
    pub decay_time: Option<u64>,
}

/// The destinations of `portals`, in the order they first appear.
pub fn destinations(portals: &[Portal]) -> Vec<Destination> {
    let mut found: Vec<Destination> = Vec::new();
    for portal in portals {
        let target = portal.destination.as_ref();
        let earliest = |a: Option<u64>, b: Option<u64>| match (a, b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let same = |entry: &Destination| match (&entry.to, target) {
            (Some(to), Some(target)) => to.room == target.room && to.shard == target.shard,
            (None, None) => true,
            _ => false,
        };
        if let Some(entry) = found.iter_mut().find(|entry| same(entry)) {
            entry.tiles += 1;
            entry.expires_at = earliest(entry.expires_at, portal.expires_at);
            entry.decay_time = earliest(entry.decay_time, portal.decay_time);
        } else {
            found.push(Destination {
                to: target.cloned(),
                tiles: 1,
                expires_at: portal.expires_at,
                decay_time: portal.decay_time,
            });
        }
    }
    found
}

/// A destination in words, seen from `shard` (the shard shown; `None` on single-world
/// servers): `W10N5`, or `shard1 W10N5` on another shard; `…` while not known yet.
pub fn destination_text(destination: Option<&PortalDestination>, shard: Option<&str>) -> String {
    let Some(destination) = destination else {
        return "…".to_owned();
    };
    match destination.shard.as_deref() {
        Some(other) if Some(other) != shard => format!("{other} {}", destination.room),
        _ => destination.room.to_string(),
    }
}

/// Where "Go to" takes the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GoTo {
    /// A room of the shard shown.
    Room(RoomCoord),
    /// A room of another shard: the map switches to it.
    Shard(String, RoomCoord),
}

impl GoTo {
    /// Where `destination` is, seen from `shard` (the shard shown).
    pub fn of(destination: &PortalDestination, shard: Option<&str>) -> Self {
        match destination.shard.as_deref() {
            Some(other) if Some(other) != shard => Self::Shard(other.to_owned(), destination.room),
            _ => Self::Room(destination.room),
        }
    }
}

#[cfg(test)]
mod tests {
    use overseer::map::{Portal, PortalDestination, RoomCoord};

    use super::{Destination, GoTo, destination_text, destinations, parse_portals, portals_json};

    fn room(name: &str) -> RoomCoord {
        name.parse().unwrap()
    }

    fn portal(x: u8, to: &str, shard: Option<&str>) -> Portal {
        Portal {
            room: room("W5N5"),
            x,
            y: 20,
            destination: Some(PortalDestination {
                room: room(to),
                shard: shard.map(str::to_owned),
                x: shard.is_none().then_some(x + 2),
                y: shard.is_none().then_some(22),
            }),
            expires_at: None,
            decay_time: None,
        }
    }

    #[test]
    fn records_round_trip_intra_and_inter_shard_portals() {
        let mut decaying = portal(11, "W0N0", None);
        decaying.expires_at = Some(1_700_000_000_000);
        decaying.decay_time = Some(123_456);
        let portals = vec![
            decaying,
            portal(12, "E10S20", Some("shard2")),
            Portal::pending(room("W5N5"), 13, 20),
        ];
        let record = portals_json(&portals, 1_700_000_000_123);
        assert_eq!(
            parse_portals(&record, room("W5N5")),
            Some((portals, 1_700_000_000_123))
        );
        // A room scanned without portals is a record too.
        assert_eq!(
            parse_portals(&portals_json(&[], 5), room("W5N5")),
            Some((Vec::new(), 5))
        );
    }

    #[test]
    fn malformed_or_foreign_records_are_ignored() {
        let room = room("W5N5");
        for record in [
            "",
            "[]",
            r#"{"v":2,"at":1,"portals":[]}"#,
            r#"{"v":1,"portals":[]}"#,
            r#"{"v":1,"at":1}"#,
            r#"{"v":1,"at":1,"portals":[{"y":2}]}"#,
            r#"{"v":1,"at":1,"portals":[{"x":1,"y":2,"to":"nowhere"}]}"#,
            r#"{"v":1,"at":1,"portals":[{"x":50,"y":2,"to":"W1N1"}]}"#,
            r#"{"v":1,"at":1,"portals":[{"x":1,"y":2,"to":"W1N1","shard":3}]}"#,
            r#"{"v":1,"at":1,"portals":[{"x":1,"y":2,"to":"W1N1","exp":"soon"}]}"#,
        ] {
            assert_eq!(parse_portals(record, room), None, "{record}");
        }
    }

    #[test]
    fn a_cluster_is_one_destination() {
        let mut later = portal(12, "W0N0", None);
        later.expires_at = Some(20);
        let mut sooner = portal(13, "W0N0", None);
        sooner.expires_at = Some(10);
        let found = destinations(&[
            portal(11, "W0N0", None),
            later,
            portal(14, "W0N0", Some("shard1")),
            sooner,
            Portal::pending(room("W5N5"), 15, 20),
            Portal::pending(room("W5N5"), 16, 20),
        ]);
        assert_eq!(
            found,
            vec![
                Destination {
                    to: portal(11, "W0N0", None).destination,
                    tiles: 3,
                    expires_at: Some(10),
                    decay_time: None,
                },
                Destination {
                    to: portal(14, "W0N0", Some("shard1")).destination,
                    tiles: 1,
                    expires_at: None,
                    decay_time: None,
                },
                // Pending destinations: one entry, shown as pending.
                Destination {
                    to: None,
                    tiles: 2,
                    expires_at: None,
                    decay_time: None,
                },
            ]
        );
        assert_eq!(destination_text(None, None), "…");
    }

    #[test]
    fn go_to_switches_shards_only_for_another_shard() {
        let here = portal(11, "W0N0", None).destination.unwrap();
        let there = portal(11, "E10S20", Some("shard2")).destination.unwrap();
        let same = portal(11, "E10S20", Some("shard3")).destination.unwrap();
        assert_eq!(GoTo::of(&here, Some("shard3")), GoTo::Room(room("W0N0")));
        assert_eq!(GoTo::of(&here, None), GoTo::Room(room("W0N0")));
        assert_eq!(
            GoTo::of(&there, Some("shard3")),
            GoTo::Shard("shard2".to_owned(), room("E10S20"))
        );
        assert_eq!(GoTo::of(&same, Some("shard3")), GoTo::Room(room("E10S20")));
        assert_eq!(destination_text(Some(&here), Some("shard3")), "W0N0");
        assert_eq!(
            destination_text(Some(&there), Some("shard3")),
            "shard2 E10S20"
        );
        assert_eq!(destination_text(Some(&same), Some("shard3")), "E10S20");
    }
}
