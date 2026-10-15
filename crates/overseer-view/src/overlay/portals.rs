//! The portals of every room given to `Viewer::set_portals`, as the overlay draws them: one
//! mark per room around its portal tiles, an arc between each pair of rooms linked within
//! the shard, and the names of the shards the room's other portals lead to.

use std::collections::{BTreeMap, BTreeSet};

use glam::Vec2;
use overseer::map::{Portal, RoomCoord};

use crate::camera::room_centre;
use crate::mesh::{ArcInstance, MarkInstance, mark, rgba};
use crate::world::tile_origin;

/// Colour (sRGB) of portals within the shard: the portal token's cyan.
pub const PORTAL: u32 = 0x5a_d1_ff;
/// Colour (sRGB) of portals to other shards.
pub const SHARD_PORTAL: u32 = 0xc3_9b_ff;
/// Opacity of the arcs between linked rooms.
const ARC_ALPHA: u8 = 150;
/// How far an arc's middle bows out, as a share of its length.
const ARC_BEND: f32 = 0.16;
/// Smallest radius of a portal mark, in CSS pixels (it shows at every zoom).
pub const MARK_MIN_PX: f32 = 5.5;
/// Tiles a portal mark reaches beyond its farthest portal's centre.
const MARK_MARGIN: f32 = 1.2;

/// What the overlay keeps of a room's portals.
#[derive(Clone, Debug, PartialEq)]
struct RoomPortals {
    /// Centre of the portal tiles (world).
    centre: Vec2,
    /// Tiles from the centre to the farthest portal tile's centre.
    reach: f32,
    /// Shards the room's portals lead to, sorted (none: all within the shard).
    shards: Vec<String>,
    /// Rooms of this shard the portals lead to, with the centre of the portals leading
    /// there (world) and of where they arrive, when known.
    links: Vec<(RoomCoord, Vec2, Option<Vec2>)>,
}

impl RoomPortals {
    fn of(room: RoomCoord, portals: &[Portal]) -> Option<Self> {
        let centre = mean_centre(room, portals.iter().map(|portal| (portal.x, portal.y)))?;
        let reach = portals
            .iter()
            .map(|portal| tile_centre(room, portal.x, portal.y).distance(centre))
            .fold(0.0, f32::max);
        let shards: BTreeSet<String> = portals
            .iter()
            .filter_map(|portal| portal.destination.shard.clone())
            .collect();
        let destinations: BTreeSet<RoomCoord> = portals
            .iter()
            .filter(|portal| portal.destination.shard.is_none() && portal.destination.room != room)
            .map(|portal| portal.destination.room)
            .collect();
        let links = destinations
            .into_iter()
            .filter_map(|destination| {
                let leading = || {
                    portals.iter().filter(move |portal| {
                        portal.destination.shard.is_none() && portal.destination.room == destination
                    })
                };
                let from = mean_centre(room, leading().map(|portal| (portal.x, portal.y)))?;
                let to = mean_centre(
                    destination,
                    leading()
                        .filter_map(|portal| Some((portal.destination.x?, portal.destination.y?))),
                );
                Some((destination, from, to))
            })
            .collect();
        Some(Self {
            centre,
            reach,
            shards: shards.into_iter().collect(),
            links,
        })
    }
}

/// The world position of a tile's centre.
fn tile_centre(room: RoomCoord, x: u8, y: u8) -> Vec2 {
    tile_origin(room, x.min(49), y.min(49)) + Vec2::splat(0.5)
}

/// The mean centre of tiles of a room (`None` without tiles).
fn mean_centre(room: RoomCoord, tiles: impl Iterator<Item = (u8, u8)>) -> Option<Vec2> {
    let (mut sum, mut count) = (Vec2::ZERO, 0.0_f32);
    for (x, y) in tiles {
        sum += tile_centre(room, x, y);
        count += 1.0;
    }
    (count > 0.0).then(|| sum / count)
}

/// The portals of every room.
#[derive(Debug, Default)]
pub struct Portals {
    rooms: BTreeMap<RoomCoord, RoomPortals>,
}

impl Portals {
    /// Sets the portals of a room (none: it has none). Returns whether anything changed,
    /// and whether the shards it leads to did.
    pub fn set(&mut self, room: RoomCoord, portals: &[Portal]) -> (bool, bool) {
        let new = RoomPortals::of(room, portals);
        let old = self.rooms.get(&room);
        let shards = |portals: Option<&RoomPortals>| portals.map(|portals| portals.shards.clone());
        if old == new.as_ref() {
            return (false, false);
        }
        let names_changed = shards(old) != shards(new.as_ref());
        match new {
            Some(portals) => self.rooms.insert(room, portals),
            None => self.rooms.remove(&room),
        };
        (true, names_changed)
    }

    /// Whether a room has portals.
    #[cfg(test)]
    pub fn contains(&self, room: RoomCoord) -> bool {
        self.rooms.contains_key(&room)
    }

    /// Appends a mark per room with portals.
    pub fn marks(&self, out: &mut Vec<MarkInstance>) {
        for portals in self.rooms.values() {
            let (color, kind) = if portals.shards.is_empty() {
                (PORTAL, mark::PORTAL)
            } else {
                (SHARD_PORTAL, mark::SHARD_PORTAL)
            };
            out.push(MarkInstance {
                centre: portals.centre.to_array(),
                radius: [portals.reach + MARK_MARGIN, MARK_MIN_PX, f32::MAX],
                uv: [0.0; 4],
                color: rgba(color, 255),
                kind,
            });
        }
    }

    /// Appends an arc per pair of rooms linked within the shard (either way, once),
    /// bowing to the left of the lesser room towards the greater one.
    pub fn arcs(&self, out: &mut Vec<ArcInstance>) {
        let mut linked = BTreeSet::new();
        for (&room, portals) in &self.rooms {
            for &(other, from, arrival) in &portals.links {
                let pair = (room.min(other), room.max(other));
                if !linked.insert(pair) {
                    continue;
                }
                // The other end: where the other room's portals back to this one are, else
                // where these arrive, else the other room's centre.
                let back = self.rooms.get(&other).and_then(|portals| {
                    portals
                        .links
                        .iter()
                        .find(|(destination, ..)| *destination == room)
                        .map(|(_, from, _)| *from)
                });
                let to = back.or(arrival).unwrap_or_else(|| room_centre(other));
                let (from, to) = if room < other { (from, to) } else { (to, from) };
                out.push(ArcInstance {
                    from: from.to_array(),
                    to: to.to_array(),
                    color: rgba(PORTAL, ARC_ALPHA),
                    bend: ARC_BEND,
                });
            }
        }
    }

    /// The rooms with portals to other shards: where their name goes (the south edge of
    /// the mark, world) and the shards' names.
    pub fn shard_names(&self) -> impl Iterator<Item = (RoomCoord, Vec2, String)> + '_ {
        self.rooms
            .iter()
            .filter(|(_, portals)| !portals.shards.is_empty())
            .map(|(&room, portals)| {
                let edge = portals.centre + Vec2::new(0.0, portals.reach + MARK_MARGIN);
                (room, edge, portals.shards.join(", "))
            })
    }
}
