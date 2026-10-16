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
//!   run; a room unlike its neighbours makes a run of its own. A run gets one mark, one
//!   [`Link`] per room offset it leads to (once per pair of linked runs, either way) and
//!   one label naming the other shards it leads to, however many portals its rooms hold.
//! - **Tiles**, zoomed in: a small mark on each portal tile, lilac when it leads to another
//!   shard.
//!
//! A run is a **wall** when its portals line up: at least [`WALL_TILES`] of them spread
//! over at least [`WALL_EXTENT`] tiles (a highway lined with portals, any number of
//! rooms long); otherwise it is **compact** (a cluster in a sector centre, a few portals
//! at a crossroad, to another shard). Compact runs are what a map is read by: a ring (a
//! capsule along several rooms) and arcs to where they lead. Walls are only borders: a
//! thin beaded line through their portals, and their links are drawn only when few are
//! in view or one of their rooms has the focus (see `Overlay::update`).

use std::collections::{BTreeMap, BTreeSet};

use glam::Vec2;
use overseer::map::{Portal, RoomCoord, RoomRect};

use crate::camera::{room_centre, room_origin};
use crate::mesh::{ArcInstance, MarkInstance, arc, mark, rgba};
use crate::world::tile_origin;

/// Colour (sRGB) of portals within the shard: the portal token's cyan.
pub const PORTAL: u32 = 0x5a_d1_ff;
/// Colour (sRGB) of portals to other shards.
pub const SHARD_PORTAL: u32 = 0xc3_9b_ff;
/// Opacity of the arcs between linked runs.
const ARC_ALPHA: u8 = 150;
/// Opacity of the arcs of the runs that have the focus.
const FOCUS_ALPHA: u8 = 235;
/// How far an arc's middle bows out, as a share of its length.
const ARC_BEND: f32 = 0.16;
/// Smallest radius of a single room's mark, in CSS pixels (it shows at every zoom).
pub const MARK_MIN_PX: f32 = 5.5;
/// Smallest half width of a run's capsule along several rooms, in CSS pixels.
const RUN_MIN_PX: f32 = 4.0;
/// Tiles a run's mark reaches beyond its farthest portal's centre.
const MARK_MARGIN: f32 = 1.2;
/// Largest radius of a compact run's mark, in tiles.
const RUN_RADIUS: f32 = 12.0;
/// A portal tile's mark: radius in tiles, least and greatest radius in CSS pixels.
const TILE_RADIUS: [f32; 3] = [0.5, 2.0, f32::MAX];
/// Fewest portals of a wall.
pub const WALL_TILES: usize = 12;
/// Fewest tiles a wall's portals spread over, along its longer side.
pub const WALL_EXTENT: f32 = 15.0;
/// A wall's line: half its width in tiles, least and greatest in CSS pixels.
const WALL_RADIUS: [f32; 3] = [0.6, 0.75, 1.5];

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
/// a disc), its radius in tiles and CSS pixels, and whether the run is a wall.
struct RunMark {
    centre: Vec2,
    half: Vec2,
    radius: [f32; 3],
    wall: bool,
}

/// The link of a run to the rooms its portals lead to within the shard (one room offset),
/// as an arc.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// The arc, [`arc::LINK`] (compact runs) or [`arc::WALL`] (walls).
    pub arc: ArcInstance,
    /// The rooms of the run it leaves.
    run: Run,
    /// Where the run's rooms lead, in rooms.
    offset: (i32, i32),
    /// The arc's bounds (world): its ends and how far its middle bows out.
    min: Vec2,
    max: Vec2,
}

impl Link {
    /// Whether it joins walls.
    pub const fn is_wall(&self) -> bool {
        self.arc.kind == arc::WALL
    }

    /// Whether `room` is one of the rooms it joins (either end).
    pub fn touches(&self, room: RoomCoord) -> bool {
        self.run.contains(&room)
            || self
                .run
                .contains(&shifted(room, (-self.offset.0, -self.offset.1)))
    }

    /// Whether its arc may cross `rooms`.
    pub fn meets(&self, rooms: RoomRect) -> bool {
        let min = room_origin(rooms.min);
        let max = room_origin(rooms.max) + Vec2::splat(50.0);
        self.min.cmple(max).all() && self.max.cmpge(min).all()
    }

    /// Its arc drawn as `kind`, with the focus's opacity for [`arc::FOCUS`].
    pub fn drawn(&self, kind: u32) -> ArcInstance {
        let alpha = if kind == arc::FOCUS {
            FOCUS_ALPHA
        } else {
            ARC_ALPHA
        };
        ArcInstance {
            kind,
            color: rgba(PORTAL, alpha),
            ..self.arc
        }
    }
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

    /// The mark of a run. A wall gets a line along its longer side through the middle of
    /// its portals; a compact run a disc around a single room's portals, a capsule through
    /// the centres of a longer run's.
    fn run_mark(&self, run: &[RoomCoord]) -> Option<RunMark> {
        let tiles: Vec<Vec2> = run
            .iter()
            .filter_map(|&room| Some((room, self.rooms.get(&room)?)))
            .flat_map(|(room, portals)| {
                portals
                    .tiles
                    .iter()
                    .map(move |&(x, y, _)| tile_centre(room, x, y))
            })
            .collect();
        let start = *tiles.first()?;
        let (min, max) = tiles.iter().fold((start, start), |(min, max), &tile| {
            (min.min(tile), max.max(tile))
        });
        let size = max - min;
        if tiles.len() >= WALL_TILES && size.max_element() >= WALL_EXTENT {
            let middle = mean(tiles.iter().copied())?;
            let (centre, half) = if size.x >= size.y {
                (
                    Vec2::new(f32::midpoint(min.x, max.x), middle.y),
                    Vec2::new(size.x * 0.5, 0.0),
                )
            } else {
                (
                    Vec2::new(middle.x, f32::midpoint(min.y, max.y)),
                    Vec2::new(0.0, size.y * 0.5),
                )
            };
            return Some(RunMark {
                centre,
                half,
                radius: WALL_RADIUS,
                wall: true,
            });
        }
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
            wall: false,
        })
    }

    /// Appends a mark per run, then one per portal tile.
    pub fn marks(&self, out: &mut Vec<MarkInstance>) {
        for run in self.runs() {
            let (Some(mark), Some(first)) = (self.run_mark(&run), self.members(&run).next()) else {
                continue;
            };
            let color = if first.shards.is_empty() {
                PORTAL
            } else {
                SHARD_PORTAL
            };
            let kind = match (mark.wall, first.shards.is_empty()) {
                (true, _) => mark::WALL,
                (false, true) => mark::PORTAL,
                (false, false) => mark::SHARD_PORTAL,
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

    /// A link per run and room offset its portals lead to within the shard, from the centre
    /// of the portals leading there to where they arrive (the centre of the portals leading
    /// back, else of where these arrive, else the rooms' centres). A link is skipped when
    /// most of its pairs of rooms are linked already (the other way, by the run they lead
    /// to), so each pair of linked runs gets one, bowing to the left of the lesser end
    /// towards the greater one; it joins walls when the run it leaves is one.
    pub fn links(&self) -> Vec<Link> {
        let mut links = Vec::new();
        let mut linked = BTreeSet::new();
        for run in self.runs() {
            let (Some(&first), Some(first_portals), Some(mark)) =
                (run.first(), self.members(&run).next(), self.run_mark(&run))
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
                let bulge = Vec2::splat(from.distance(to) * ARC_BEND);
                links.push(Link {
                    arc: ArcInstance {
                        from: from.to_array(),
                        to: to.to_array(),
                        color: rgba(PORTAL, ARC_ALPHA),
                        bend: ARC_BEND,
                        kind: if mark.wall { arc::WALL } else { arc::LINK },
                    },
                    run: run.clone(),
                    offset,
                    min: from.min(to) - bulge,
                    max: from.max(to) + bulge,
                });
            }
        }
        links
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
