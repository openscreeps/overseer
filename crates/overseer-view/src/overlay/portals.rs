//! The portals of every room given to `Viewer::set_portals`, as the overlay draws them.
//!
//! A room holds anything from one cluster of 8 portals (a sector centre) to walls of 48
//! and more along its edges (screeps.com's highways, where every room of a highway segment
//! leads to the same room of the parallel segment ten rooms away), and neighbouring rooms
//! often lead the same way. So the overlay draws them at two levels of detail
//! ([`super::Fades`]):
//!
//! - **Runs**, zoomed out: rooms side by side in a row (else a column) whose portals lead
//!   the same way (the same room offsets within the shard, the same other shards) make one
//!   run; a room unlike its neighbours makes a run of its own. A run gets one mark (a disc
//!   around a single room's portals, a capsule along a longer run), one arc per room offset
//!   it leads to (once per pair of linked runs, either way) and one label naming the other
//!   shards it leads to, however many portals its rooms hold.
//! - **Tiles**, zoomed in: a small mark on each portal tile, lilac when it leads to another
//!   shard.

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
/// Opacity of the arcs between linked runs.
const ARC_ALPHA: u8 = 150;
/// How far an arc's middle bows out, as a share of its length.
const ARC_BEND: f32 = 0.16;
/// Smallest radius of a single room's mark, in CSS pixels (it shows at every zoom).
pub const MARK_MIN_PX: f32 = 5.5;
/// Smallest half width of a run's capsule along several rooms, in CSS pixels.
const RUN_MIN_PX: f32 = 4.0;
/// Tiles a run's mark reaches beyond its farthest portal's centre.
const MARK_MARGIN: f32 = 1.2;
/// Largest radius of a run's mark, in tiles: a wall of portals along a room's edge is not
/// circled whole.
const RUN_RADIUS: f32 = 12.0;
/// A portal tile's mark: radius in tiles, least and greatest radius in CSS pixels.
const TILE_RADIUS: [f32; 3] = [0.5, 2.0, f32::MAX];

/// What the overlay keeps of a room's portals.
#[derive(Clone, Debug, PartialEq)]
struct RoomPortals {
    /// Centre of the portal tiles (world).
    centre: Vec2,
    /// Tiles from the centre to the farthest portal tile's centre.
    reach: f32,
    /// Shards the room's portals lead to, sorted (none: all within the shard).
    shards: Vec<String>,
    /// Rooms of this shard the portals lead to, sorted, with the centre of the portals
    /// leading there (world) and of where they arrive, when known.
    links: Vec<(RoomCoord, Vec2, Option<Vec2>)>,
    /// Each portal's tile, and whether it leads to another shard.
    tiles: Vec<(u8, u8, bool)>,
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
        let tiles = portals
            .iter()
            .map(|portal| (portal.x, portal.y, portal.destination.shard.is_some()))
            .collect();
        Some(Self {
            centre,
            reach,
            shards: shards.into_iter().collect(),
            links,
            tiles,
        })
    }

    /// Whether the portals of `room` (these) lead the same way as those of `other_room`:
    /// to the same room offsets within the shard and to the same other shards.
    fn leads_like(&self, room: RoomCoord, other: &Self, other_room: RoomCoord) -> bool {
        self.shards == other.shards
            && self.links.len() == other.links.len()
            && self
                .links
                .iter()
                .zip(&other.links)
                .all(|((to, ..), (other_to, ..))| {
                    offset(room, *to) == offset(other_room, *other_to)
                })
    }

    /// Where the portals leading to `destination` stand and arrive.
    fn link(&self, destination: RoomCoord) -> Option<(Vec2, Option<Vec2>)> {
        self.links
            .iter()
            .find(|(to, ..)| *to == destination)
            .map(|&(_, from, arrival)| (from, arrival))
    }
}

/// The rooms from `from` to `to`.
const fn offset(from: RoomCoord, to: RoomCoord) -> (i32, i32) {
    (to.x - from.x, to.y - from.y)
}

/// `room` moved by `offset` rooms.
const fn shifted(room: RoomCoord, offset: (i32, i32)) -> RoomCoord {
    RoomCoord {
        x: room.x + offset.0,
        y: room.y + offset.1,
    }
}

/// The world position of a tile's centre.
fn tile_centre(room: RoomCoord, x: u8, y: u8) -> Vec2 {
    tile_origin(room, x.min(49), y.min(49)) + Vec2::splat(0.5)
}

/// The mean centre of tiles of a room (`None` without tiles).
fn mean_centre(room: RoomCoord, tiles: impl Iterator<Item = (u8, u8)>) -> Option<Vec2> {
    mean(tiles.map(|(x, y)| tile_centre(room, x, y)))
}

/// The mean of points (`None` without points).
fn mean(points: impl Iterator<Item = Vec2>) -> Option<Vec2> {
    let (mut sum, mut count) = (Vec2::ZERO, 0.0_f32);
    for point in points {
        sum += point;
        count += 1.0;
    }
    (count > 0.0).then(|| sum / count)
}

/// Rooms side by side whose portals lead the same way: west to east, or north to south.
type Run = Vec<RoomCoord>;

/// The mark of a run: its centre (world), the half of its capsule's spine (tiles; zero for
/// a disc) and its radius in tiles and CSS pixels.
struct RunMark {
    centre: Vec2,
    half: Vec2,
    radius: [f32; 3],
}

/// The portals of every room.
#[derive(Debug, Default)]
pub struct Portals {
    rooms: BTreeMap<RoomCoord, RoomPortals>,
}

impl Portals {
    /// Sets the portals of a room (none: it has none). Returns whether anything changed,
    /// and whether shard names may have (the room leads, or led, to other shards).
    pub fn set(&mut self, room: RoomCoord, portals: &[Portal]) -> (bool, bool) {
        let new = RoomPortals::of(room, portals);
        let old = self.rooms.get(&room);
        if old == new.as_ref() {
            return (false, false);
        }
        let other_shards = |portals: Option<&RoomPortals>| {
            portals.is_some_and(|portals| !portals.shards.is_empty())
        };
        let names_changed = other_shards(old) || other_shards(new.as_ref());
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

    /// Whether the portals of rooms `a` and `b` (both with portals) lead the same way.
    fn alike(&self, a: RoomCoord, b: RoomCoord) -> bool {
        match (self.rooms.get(&a), self.rooms.get(&b)) {
            (Some(first), Some(second)) => first.leads_like(a, second, b),
            _ => false,
        }
    }

    /// The runs of rooms with portals, by their first room: rows of two rooms or more
    /// first, then columns (of one room or more) of the rooms left.
    fn runs(&self) -> Vec<Run> {
        let east = |room| shifted(room, (1, 0));
        let west = |room| shifted(room, (-1, 0));
        let south = |room| shifted(room, (0, 1));
        let north = |room| shifted(room, (0, -1));
        let mut runs = Vec::new();
        let mut taken = BTreeSet::new();
        for &room in self.rooms.keys() {
            if self.alike(west(room), room) {
                continue;
            }
            let mut row = vec![room];
            let mut last = room;
            while self.alike(last, east(last)) {
                last = east(last);
                row.push(last);
            }
            if row.len() > 1 {
                taken.extend(row.iter().copied());
                runs.push(row);
            }
        }
        for &room in self.rooms.keys() {
            if taken.contains(&room)
                || (!taken.contains(&north(room)) && self.alike(north(room), room))
            {
                continue;
            }
            let mut column = vec![room];
            let mut last = room;
            while !taken.contains(&south(last)) && self.alike(last, south(last)) {
                last = south(last);
                column.push(last);
            }
            runs.push(column);
        }
        runs.sort_unstable();
        runs
    }

    /// The rooms' portals of a run, in its order.
    fn members<'a>(&'a self, run: &'a [RoomCoord]) -> impl Iterator<Item = &'a RoomPortals> {
        run.iter().filter_map(|room| self.rooms.get(room))
    }

    /// The mark of a run: a disc around a single room's portals, a capsule through the
    /// centres of a longer run's.
    fn run_mark(&self, run: &[RoomCoord]) -> Option<RunMark> {
        let first = self.members(run).next()?;
        let last = self.members(run).last()?;
        let reach = self
            .members(run)
            .map(|portals| portals.reach)
            .fold(0.0, f32::max);
        let radius = (reach + MARK_MARGIN).min(RUN_RADIUS);
        let min_px = if run.len() > 1 {
            RUN_MIN_PX
        } else {
            MARK_MIN_PX
        };
        Some(RunMark {
            centre: (first.centre + last.centre) * 0.5,
            half: (last.centre - first.centre) * 0.5,
            radius: [radius, min_px, f32::MAX],
        })
    }

    /// Appends a mark per run, then one per portal tile.
    pub fn marks(&self, out: &mut Vec<MarkInstance>) {
        for run in self.runs() {
            let (Some(mark), Some(first)) = (self.run_mark(&run), self.members(&run).next()) else {
                continue;
            };
            let (color, kind) = if first.shards.is_empty() {
                (PORTAL, mark::PORTAL)
            } else {
                (SHARD_PORTAL, mark::SHARD_PORTAL)
            };
            out.push(MarkInstance {
                centre: mark.centre.to_array(),
                radius: mark.radius,
                uv: [mark.half.x, mark.half.y, 0.0, 0.0],
                color: rgba(color, 255),
                kind,
            });
        }
        for (&room, portals) in &self.rooms {
            out.extend(portals.tiles.iter().map(|&(x, y, other_shard)| {
                let (color, kind) = if other_shard {
                    (SHARD_PORTAL, mark::SHARD_PORTAL_TILE)
                } else {
                    (PORTAL, mark::PORTAL_TILE)
                };
                MarkInstance {
                    centre: tile_centre(room, x, y).to_array(),
                    radius: TILE_RADIUS,
                    uv: [0.0; 4],
                    color: rgba(color, 255),
                    kind,
                }
            }));
        }
    }

    /// Appends an arc per run and room offset its portals lead to within the shard, from
    /// the centre of the portals leading there to where they arrive (the centre of the
    /// portals leading back, else of where these arrive, else the rooms' centres). An arc
    /// is skipped when most of its pairs of rooms are linked already (the other way, by
    /// the run they lead to), so each pair of linked runs gets one arc, bowing to the left
    /// of the lesser end towards the greater one.
    pub fn arcs(&self, out: &mut Vec<ArcInstance>) {
        let mut linked = BTreeSet::new();
        for run in self.runs() {
            let (Some(&first), Some(first_portals)) = (run.first(), self.members(&run).next())
            else {
                continue;
            };
            for &(destination, ..) in &first_portals.links {
                let offset = offset(first, destination);
                let pairs: Vec<(RoomCoord, RoomCoord)> = run
                    .iter()
                    .map(|&room| {
                        let other = shifted(room, offset);
                        (room.min(other), room.max(other))
                    })
                    .collect();
                let known = pairs.iter().filter(|pair| linked.contains(*pair)).count();
                if known * 2 > pairs.len() {
                    continue;
                }
                linked.extend(pairs);
                let ends = || {
                    run.iter().filter_map(move |&room| {
                        let other = shifted(room, offset);
                        let (from, arrival) = self.rooms.get(&room)?.link(other)?;
                        let back = self
                            .rooms
                            .get(&other)
                            .and_then(|portals| portals.link(room))
                            .map(|(back, _)| back);
                        Some((from, back.or(arrival).unwrap_or_else(|| room_centre(other))))
                    })
                };
                let (Some(from), Some(to)) = (
                    mean(ends().map(|(from, _)| from)),
                    mean(ends().map(|(_, to)| to)),
                ) else {
                    continue;
                };
                let (from, to) = if first < destination {
                    (from, to)
                } else {
                    (to, from)
                };
                out.push(ArcInstance {
                    from: from.to_array(),
                    to: to.to_array(),
                    color: rgba(PORTAL, ARC_ALPHA),
                    bend: ARC_BEND,
                });
            }
        }
    }

    /// The runs with portals to other shards: their middle room, where their label goes
    /// (the south edge of the run's mark, world) and the shards' names.
    pub fn shard_names(&self) -> impl Iterator<Item = (RoomCoord, Vec2, String)> + '_ {
        self.runs().into_iter().filter_map(|run| {
            let shards = &self.members(&run).next()?.shards;
            if shards.is_empty() {
                return None;
            }
            let mark = self.run_mark(&run)?;
            let middle = *run.get(run.len() / 2)?;
            let edge = mark.centre + Vec2::new(0.0, mark.half.y.abs() + mark.radius[0]);
            Some((middle, edge, shards.join(", ")))
        })
    }
}
