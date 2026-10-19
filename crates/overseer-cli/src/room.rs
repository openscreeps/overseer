//! Rooms of the world and the checks for the first spawn of an account. The selector chooses a
//! room and a tile. The CLI uses these checks to decide whether it can place the spawn there.
//! The game rules come from the library ([`RoomCoord`], [`Terrain::check_structure_spot`]).
//! The other checks are the policy of the CLI: they are stricter than the server.

use overseer::map::RoomCoord;
use overseer::model::account::split_shard;
use overseer::model::world::{GameRoomObject, GameRoomStatus, MapStatsRoom, SpotError, Terrain};

/// The room status that lets a spawn in.
const NORMAL: &str = "normal";

/// Returns the rooms at a maximum distance of `radius` from `start`, nearest first, then by
/// name.
pub(crate) fn around(start: RoomCoord, radius: u16) -> Vec<RoomCoord> {
    let mut rooms: Vec<RoomCoord> = start.around(radius).collect();
    // The key cache formats each name one time, not two times for each comparison.
    rooms.sort_by_cached_key(|room| (start.distance(*room), room.to_string()));
    rooms
}

/// Returns the room part of `name` for `shard`. `name` is `W1N1` or
/// `shard3/W1N1`. A prefix must be `shard`, or `shard` must be `None`.
/// Returns `None` for a different shard.
pub(crate) fn on_shard<'a>(name: &'a str, shard: Option<&str>) -> Option<&'a str> {
    let (prefix, room) = split_shard(name);
    of_shard(prefix, shard).then_some(room)
}

/// Whether a room of the shard `prefix` is a room of `shard`. A room without
/// a shard prefix matches each shard, and each room matches when `shard` is
/// `None`.
pub(crate) fn of_shard(prefix: Option<&str>, shard: Option<&str>) -> bool {
    match (prefix, shard) {
        (Some(prefix), Some(shard)) => prefix == shard,
        _ => true,
    }
}

/// The reason that a room or a tile does not take the first spawn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Unfit {
    /// The room is not open to players.
    #[error("status {0}")]
    Status(String),
    /// The room opens later.
    #[error("not open yet")]
    NotOpen,
    /// The room is on the server list of rooms where a respawn is prohibited.
    #[error("prohibited for respawn")]
    Prohibited,
    /// A user owns or reserves the room.
    #[error("owned or reserved")]
    Owned,
    /// The room has no controller, or several.
    #[error("{0} controllers")]
    Controllers(usize),
    /// The room has Source Keepers or an Invader core.
    #[error("has a {0}")]
    Hostile(String),
    /// The room has an object of a player or of an NPC.
    #[error("has a {0} of another owner")]
    Foreign(String),
    /// A field of the server response is missing or has an incorrect type.
    #[error("unexpected field {0}")]
    Malformed(&'static str),
    /// The terrain does not let a structure stand on the tile (a game rule).
    #[error(transparent)]
    Spot(#[from] SpotError),
    /// An object is on the tile.
    #[error("a {0} is on the tile")]
    Occupied(String),
}

/// Checks whether the `room-status` result `status` lets a spawn in. The
/// status must be `normal`, and the room must be open at `now` (milliseconds
/// since the Unix epoch). An unknown room has no status, so it does not let
/// a spawn in.
///
/// # Errors
///
/// Returns the reason that the room does not let a spawn in.
pub(crate) fn check_status(status: &GameRoomStatus, now: f64) -> Result<(), Unfit> {
    check_open(status.status.as_deref(), status.open_time, now)
}

/// Checks whether the map statistics `room` of a candidate let a spawn in.
/// The status must be `normal`, and the room must be open at `now`
/// (milliseconds since the Unix epoch). The room must have no owner, no
/// reservation and no Invader core (`own`).
///
/// # Errors
///
/// Returns the reason that the room does not let a spawn in.
pub(crate) fn check_map_status(room: &MapStatsRoom, now: f64) -> Result<(), Unfit> {
    check_open(room.status.as_deref(), room.open_time, now)?;
    if room.own.is_some() {
        return Err(Unfit::Owned);
    }
    Ok(())
}

/// Checks that the status `status` is `normal` and that the opening time
/// `open` (milliseconds since the Unix epoch) is not after `now`.
fn check_open(status: Option<&str>, open: Option<f64>, now: f64) -> Result<(), Unfit> {
    match status {
        Some(NORMAL) => {}
        Some(other) => return Err(Unfit::Status(other.to_owned())),
        None => return Err(Unfit::Malformed("status")),
    }
    if open.is_some_and(|open| open > now) {
        return Err(Unfit::NotOpen);
    }
    Ok(())
}

/// Checks whether the objects `objects` of a room let a spawn in. The room
/// must have one neutral controller without a reservation. It must have no
/// Source Keepers and no Invader core. It must have no object of a different
/// owner. Tombstones and ruins are exceptions: they keep the owner of the
/// original object, but they do not show a presence in the room.
///
/// # Errors
///
/// Returns the reason that the room does not let a spawn in.
pub(crate) fn check_objects(objects: &[GameRoomObject]) -> Result<(), Unfit> {
    let mut controllers = 0;
    for object in objects {
        match object {
            GameRoomObject::KeeperLair(_) | GameRoomObject::InvaderCore(_) => {
                return Err(Unfit::Hostile(object.kind().to_owned()));
            }
            GameRoomObject::Controller(controller) => {
                controllers += 1;
                if controller.user.is_some() || controller.reservation.is_some() {
                    return Err(Unfit::Owned);
                }
            }
            GameRoomObject::Tombstone(_) | GameRoomObject::Ruin(_) => {}
            _ if object.user().is_some() => {
                return Err(Unfit::Foreign(object.kind().to_owned()));
            }
            _ => {}
        }
    }
    if controllers == 1 {
        Ok(())
    } else {
        Err(Unfit::Controllers(controllers))
    }
}

/// Checks whether a spawn can stand at `x`, `y` in a room with the terrain
/// `terrain` and the objects `objects`. The terrain must let a structure stand
/// on the tile ([`Terrain::check_structure_spot`]). No object can be on the
/// tile: this rule of the CLI is stricter than the server.
///
/// # Errors
///
/// Returns the reason that the tile does not take a spawn.
pub(crate) fn check_tile(
    terrain: &Terrain,
    objects: &[GameRoomObject],
    x: u8,
    y: u8,
) -> Result<(), Unfit> {
    terrain.check_structure_spot(x, y)?;
    let tile = (u64::from(x), u64::from(y));
    if let Some(object) = objects
        .iter()
        .find(|object| (object.x(), object.y()) == tile)
    {
        return Err(Unfit::Occupied(object.kind().to_owned()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use overseer::model::world::{
        GameRoomObjectController, GameRoomObjectCreep, GameRoomObjectKeeperLair,
        GameRoomObjectOther, GameRoomObjectReservation, GameRoomObjectSource,
        GameRoomObjectTombstone, MapStatsOwner,
    };

    use super::*;

    /// Parses a room name.
    fn room(name: &str) -> RoomCoord {
        name.parse().expect("a room name")
    }

    /// A room name with a shard prefix counts only on its shard.
    #[test]
    fn shard_prefixes() {
        assert_eq!(on_shard("shard3/W1N1", Some("shard3")), Some("W1N1"));
        assert_eq!(on_shard("shard0/W1N1", Some("shard3")), None);
        assert_eq!(on_shard("W1N1", Some("shard3")), Some("W1N1"));
        assert_eq!(on_shard("shard0/W1N1", None), Some("W1N1"));
    }

    /// The rooms around a room come nearest first, then by name.
    #[test]
    fn rooms_around() {
        let rooms: Vec<String> = around(room("W1N1"), 1)
            .into_iter()
            .map(|room| room.to_string())
            .collect();
        assert_eq!(
            rooms,
            [
                "W1N1", "W0N0", "W0N1", "W0N2", "W1N0", "W1N2", "W2N0", "W2N1", "W2N2"
            ]
        );
        assert_eq!(around(room("E5S5"), 5).len(), 121);
    }

    /// A plain room with walls on the edge and a wall at (30, 25).
    fn terrain() -> Terrain {
        let mut tiles = vec![b'0'; 2500];
        for at in 0..50 {
            for (x, y) in [(at, 0), (at, 49), (0, at), (49, at)] {
                tiles[y * 50 + x] = b'1';
            }
        }
        tiles[25 * 50 + 30] = b'1';
        Terrain::decode(&String::from_utf8(tiles).expect("ASCII")).expect("valid terrain")
    }

    /// A source at `x`, `y`.
    fn source(x: u64, y: u64) -> GameRoomObject {
        GameRoomObject::Source(GameRoomObjectSource {
            r#type: "source".to_owned(),
            x,
            y,
            ..GameRoomObjectSource::default()
        })
    }

    /// A controller at (10, 10) with the owner `user` and the reservation
    /// `reservation`.
    fn controller(
        user: Option<&str>,
        reservation: Option<GameRoomObjectReservation>,
    ) -> GameRoomObject {
        GameRoomObject::Controller(GameRoomObjectController {
            r#type: "controller".to_owned(),
            x: 10,
            y: 10,
            level: u64::from(user.is_some()),
            user: user.map(str::to_owned),
            reservation,
            ..GameRoomObjectController::default()
        })
    }

    /// Tiles: the game rules of the terrain and every object on the tile block a spawn.
    #[test]
    fn tiles() {
        let terrain = terrain();
        let objects = [source(20, 20)];
        assert_eq!(check_tile(&terrain, &objects, 25, 25), Ok(()));
        assert_eq!(
            check_tile(&terrain, &objects, 30, 25),
            Err(Unfit::Spot(SpotError::Wall))
        );
        assert_eq!(
            check_tile(&terrain, &objects, 20, 20),
            Err(Unfit::Occupied("source".to_owned()))
        );
    }

    /// Rooms: one neutral controller, no Source Keepers, no other owner.
    #[test]
    fn rooms() {
        let neutral = controller(None, None);
        assert_eq!(check_objects(&[neutral.clone(), source(5, 5)]), Ok(()));
        let tombstone = GameRoomObject::Tombstone(GameRoomObjectTombstone {
            r#type: "tombstone".to_owned(),
            user: "u".to_owned(),
            x: 1,
            y: 1,
            ..GameRoomObjectTombstone::default()
        });
        assert_eq!(check_objects(&[neutral.clone(), tombstone]), Ok(()));
        assert_eq!(check_objects(&[source(5, 5)]), Err(Unfit::Controllers(0)));
        assert_eq!(
            check_objects(&[neutral.clone(), neutral.clone()]),
            Err(Unfit::Controllers(2))
        );
        let reservation = GameRoomObjectReservation {
            end_time: 5000,
            user: "u".to_owned(),
        };
        assert_eq!(
            check_objects(&[controller(None, Some(reservation))]),
            Err(Unfit::Owned)
        );
        assert_eq!(
            check_objects(&[controller(Some("u"), None)]),
            Err(Unfit::Owned)
        );
        let lair = GameRoomObject::KeeperLair(GameRoomObjectKeeperLair {
            r#type: "keeperLair".to_owned(),
            ..GameRoomObjectKeeperLair::default()
        });
        assert_eq!(
            check_objects(&[neutral.clone(), lair]),
            Err(Unfit::Hostile("keeperLair".to_owned()))
        );
        let invader = GameRoomObject::Creep(GameRoomObjectCreep {
            r#type: "creep".to_owned(),
            user: "2".to_owned(),
            ..GameRoomObjectCreep::default()
        });
        assert_eq!(
            check_objects(&[neutral.clone(), invader]),
            Err(Unfit::Foreign("creep".to_owned()))
        );
        let modded = GameRoomObject::Other(GameRoomObjectOther {
            r#type: "scoreCollector".to_owned(),
            user: Some("u".to_owned()),
            ..GameRoomObjectOther::default()
        });
        assert_eq!(
            check_objects(&[neutral, modded]),
            Err(Unfit::Foreign("scoreCollector".to_owned()))
        );
    }

    /// Statuses: the status must be `normal`, and the room must be open. An
    /// unknown room has no status.
    #[test]
    fn statuses() {
        let status = |status: Option<&str>, open_time| {
            let status = GameRoomStatus {
                status: status.map(str::to_owned),
                open_time,
                ..GameRoomStatus::default()
            };
            check_status(&status, 1000.0)
        };
        assert_eq!(status(Some("normal"), None), Ok(()));
        assert_eq!(status(Some("normal"), Some(999.0)), Ok(()));
        assert_eq!(status(Some("normal"), Some(2000.0)), Err(Unfit::NotOpen));
        assert_eq!(
            status(Some("out of borders"), None),
            Err(Unfit::Status("out of borders".to_owned()))
        );
        assert_eq!(status(None, None), Err(Unfit::Malformed("status")));
    }

    /// Map statistics: normal, open, and without an owner or a reservation.
    #[test]
    fn map_statuses() {
        let normal = MapStatsRoom {
            status: Some("normal".to_owned()),
            ..MapStatsRoom::default()
        };
        assert_eq!(check_map_status(&normal, 1000.0), Ok(()));
        let closed = MapStatsRoom {
            open_time: Some(2000.0),
            ..normal.clone()
        };
        assert_eq!(check_map_status(&closed, 1000.0), Err(Unfit::NotOpen));
        let reserved = MapStatsRoom {
            own: Some(MapStatsOwner {
                user: Some("u".to_owned()),
                level: 0,
            }),
            ..normal
        };
        assert_eq!(check_map_status(&reserved, 1000.0), Err(Unfit::Owned));
        assert_eq!(
            check_map_status(&MapStatsRoom::default(), 1000.0),
            Err(Unfit::Malformed("status"))
        );
    }
}
