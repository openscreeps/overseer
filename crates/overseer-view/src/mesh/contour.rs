//! Smoothed terrain outlines ("rounded marching squares"), shared by the flat world shader
//! (`world.wgsl`, `cell_sd`) and the raised walls of detail scenes.
//!
//! Samples are tile centres. A cell is the square between four of them (its corners: NW,
//! NE, SW, SE), and what of it is inside a terrain kind follows only from which corners are:
//!
//! | corners inside | region of the cell |
//! |---|---|
//! | none | nothing |
//! | one | the quarter disc of radius ½ around it |
//! | two sharing a side | the half of the cell along that side |
//! | two diagonal | both quarter discs (diagonal neighbours stay apart, as in the game) |
//! | three | the cell but the quarter disc around the fourth corner |
//! | four | the whole cell |
//!
//! So a lone tile is a disc of the tile's width, straight runs keep their tile edges, outer
//! corners are rounded with radius ½ and inner corners filleted with the same radius. A
//! region only depends on the two corners of a shared cell side along that side, so shapes
//! (and their signed distance, [`cell_sd`]) are continuous from cell to cell. Tiles outside
//! the room repeat the border tile, which keeps cells over the room edge straight.

use glam::{Vec2, Vec3};
use overseer::model::world::{Terrain, Tile};

use super::{Mesh, TerrainVertex, WALL_HEIGHT, WALL_SIDE, WALL_TOP, linear};

/// Radius of rounded corners, in tiles: half a tile, so that a lone tile is a disc.
pub const RADIUS: f32 = 0.5;
/// Straight segments per quarter arc.
pub const ARC_SEGMENTS: u16 = 8;

/// Corners of a cell in cell coordinates (`0..1`, x east, y south): NW, NE, SW, SE.
pub const CORNERS: [Vec2; 4] = [
    Vec2::new(0.0, 0.0),
    Vec2::new(1.0, 0.0),
    Vec2::new(0.0, 1.0),
    Vec2::new(1.0, 1.0),
];

/// Signed distance (tiles; negative inside) from `q` (cell coordinates) to the region of a
/// cell whose corners `inside` (NW, NE, SW, SE) are of the kind. Exact near the outline;
/// a constant ±1 in empty and full cells. `world.wgsl` evaluates it; the tests check the
/// wall mesh against it.
#[cfg(test)]
pub fn cell_sd(inside: [bool; 4], q: Vec2) -> f32 {
    let count = inside.iter().filter(|corner| **corner).count();
    let corner = |wanted: bool| {
        inside
            .iter()
            .position(|corner| *corner == wanted)
            .map_or(Vec2::ZERO, |index| CORNERS[index])
    };
    match count {
        0 => 1.0,
        1 => q.distance(corner(true)) - RADIUS,
        3 => RADIUS - q.distance(corner(false)),
        4 => -1.0,
        _ if inside[0] == inside[3] => {
            // Diagonal: two quarter discs.
            let (a, b) = if inside[0] {
                (CORNERS[0], CORNERS[3])
            } else {
                (CORNERS[1], CORNERS[2])
            };
            q.distance(a).min(q.distance(b)) - RADIUS
        }
        // Top or bottom half.
        _ if inside[0] == inside[1] => {
            if inside[0] {
                q.y - 0.5
            } else {
                0.5 - q.y
            }
        }
        // Left or right half.
        _ => {
            if inside[0] {
                q.x - 0.5
            } else {
                0.5 - q.x
            }
        }
    }
}

/// The corners (NW, NE, SW, SE) of the cell whose NW corner is the centre of tile `(x, y)`
/// that `is` (tiles outside the room repeat the border tile).
pub fn cell_corners(x: i32, y: i32, is: impl Fn(u8, u8) -> bool) -> [bool; 4] {
    let tile = |x: i32, y: i32| {
        let clamp = |value: i32| u8::try_from(value.clamp(0, 49)).unwrap_or(0);
        is(clamp(x), clamp(y))
    };
    [
        tile(x, y),
        tile(x + 1, y),
        tile(x, y + 1),
        tile(x + 1, y + 1),
    ]
}

/// The outline of a cell's region: segments with the outward normal at each end, in cell
/// coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// Start and end.
    pub points: [Vec2; 2],
    /// Outward unit normals at the start and end (they differ along arcs).
    pub normals: [Vec2; 2],
}

/// The region of a cell as triangles and its outline, in cell coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cell {
    /// Triangles covering the region.
    pub triangles: Vec<[Vec2; 3]>,
    /// The outline inside the cell (not the cell's own sides).
    pub edges: Vec<Edge>,
}

impl Cell {
    /// The region of a cell whose corners `inside` (NW, NE, SW, SE) are of the kind.
    pub fn of(inside: [bool; 4]) -> Self {
        let mut cell = Self::default();
        let count = inside.iter().filter(|corner| **corner).count();
        match count {
            0 => {}
            4 => cell.square(Vec2::ZERO, Vec2::ONE),
            3 => {
                let out = inside.iter().position(|corner| !corner).unwrap_or(0);
                cell.fillet(out);
            }
            1 => {
                let corner = inside.iter().position(|corner| *corner).unwrap_or(0);
                cell.quarter_disc(corner);
            }
            _ if inside[0] == inside[3] => {
                let (a, b) = if inside[0] { (0, 3) } else { (1, 2) };
                cell.quarter_disc(a);
                cell.quarter_disc(b);
            }
            _ if inside[0] == inside[1] => {
                let (min, max, normal) = if inside[0] {
                    (Vec2::ZERO, Vec2::new(1.0, 0.5), Vec2::Y)
                } else {
                    (Vec2::new(0.0, 0.5), Vec2::ONE, Vec2::NEG_Y)
                };
                cell.square(min, max);
                let y = 0.5;
                cell.line(Vec2::new(0.0, y), Vec2::new(1.0, y), normal);
            }
            _ => {
                let (min, max, normal) = if inside[0] {
                    (Vec2::ZERO, Vec2::new(0.5, 1.0), Vec2::X)
                } else {
                    (Vec2::new(0.5, 0.0), Vec2::ONE, Vec2::NEG_X)
                };
                cell.square(min, max);
                let x = 0.5;
                cell.line(Vec2::new(x, 0.0), Vec2::new(x, 1.0), normal);
            }
        }
        cell
    }

    fn square(&mut self, min: Vec2, max: Vec2) {
        let (a, b, c, d) = (min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y));
        self.triangles.push([a, b, c]);
        self.triangles.push([a, c, d]);
    }

    fn line(&mut self, from: Vec2, to: Vec2, normal: Vec2) {
        self.edges.push(Edge {
            points: [from, to],
            normals: [normal, normal],
        });
    }

    /// The points of the quarter arc of radius [`RADIUS`] around a corner, from the cell side
    /// along x to the side along y.
    fn arc(corner: usize) -> impl Iterator<Item = Vec2> {
        let centre = CORNERS[corner];
        let towards = Vec2::ONE - 2.0 * centre;
        (0..=ARC_SEGMENTS).map(move |step| {
            let angle = f32::from(step) / f32::from(ARC_SEGMENTS) * std::f32::consts::FRAC_PI_2;
            centre + Vec2::new(angle.cos(), angle.sin()) * towards * RADIUS
        })
    }

    /// The quarter disc around an inside corner.
    fn quarter_disc(&mut self, corner: usize) {
        let centre = CORNERS[corner];
        let mut previous: Option<Vec2> = None;
        for point in Self::arc(corner) {
            if let Some(last) = previous {
                self.triangles.push([centre, last, point]);
                self.edges.push(Edge {
                    points: [last, point],
                    normals: [(last - centre) / RADIUS, (point - centre) / RADIUS],
                });
            }
            previous = Some(point);
        }
    }

    /// The cell but the quarter disc around an outside corner.
    fn fillet(&mut self, out: usize) {
        let centre = CORNERS[out];
        let opposite = CORNERS[3 - out];
        let towards = Vec2::ONE - 2.0 * centre;
        let along_x = centre + Vec2::new(towards.x, 0.0);
        let along_y = centre + Vec2::new(0.0, towards.y);
        let mut previous = along_x;
        let mut first = true;
        for point in Self::arc(out) {
            if first {
                first = false;
            } else {
                self.edges.push(Edge {
                    points: [previous, point],
                    normals: [(centre - previous) / RADIUS, (centre - point) / RADIUS],
                });
            }
            self.triangles.push([opposite, previous, point]);
            previous = point;
        }
        self.triangles.push([opposite, previous, along_y]);
    }
}

/// The raised natural walls of a room: tops at [`WALL_HEIGHT`] and sides along the
/// smoothed outline and the room edge. Plains and swamps are not part of it (the flat world
/// layer draws the ground).
pub fn walls(terrain: &Terrain) -> Mesh<TerrainVertex> {
    let mut mesh = Mesh::default();
    let is_wall = |x: u8, y: u8| terrain.get(x, y) == Some(Tile::Wall);
    let top = linear(WALL_TOP);
    let side = linear(WALL_SIDE);
    let world = |origin: Vec2, point: Vec2| (origin + point).clamp(Vec2::ZERO, Vec2::splat(50.0));
    // The same cell shapes are reused: 16 corner combinations.
    let cells: Vec<Cell> = (0..16_u8)
        .map(|bits| Cell::of([0, 1, 2, 3].map(|bit| bits >> bit & 1 == 1)))
        .collect();
    for y in -1..50 {
        for x in -1..50 {
            let inside = cell_corners(x, y, is_wall);
            let bits = inside
                .iter()
                .enumerate()
                .fold(0, |bits, (bit, inside)| bits | usize::from(*inside) << bit);
            let Some(cell) = cells.get(bits) else {
                continue;
            };
            #[expect(clippy::cast_precision_loss, reason = "small tile numbers")]
            let origin = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            for triangle in &cell.triangles {
                let [a, b, c] = triangle.map(|point| world(origin, point));
                if (b - a).perp_dot(c - a).abs() < 1e-6 {
                    continue;
                }
                let lift = |point: Vec2| Vec3::new(point.x, WALL_HEIGHT, point.y);
                mesh.triangle([lift(a), lift(b), lift(c)], Vec3::Y, |position| {
                    TerrainVertex {
                        position: position.to_array(),
                        normal: Vec3::Y.to_array(),
                        color: top,
                    }
                });
            }
            for edge in &cell.edges {
                let [a, b] = edge.points.map(|point| world(origin, point));
                if a.distance_squared(b) < 1e-9 {
                    continue;
                }
                side_quad(&mut mesh, [a, b], edge.normals, side);
            }
        }
    }
    // The room edge.
    for index in 0..50_u8 {
        let at = f32::from(index);
        let sides = [
            (
                0,
                index,
                Vec2::new(0.0, at),
                Vec2::new(0.0, at + 1.0),
                Vec2::NEG_X,
            ),
            (
                49,
                index,
                Vec2::new(50.0, at),
                Vec2::new(50.0, at + 1.0),
                Vec2::X,
            ),
            (
                index,
                0,
                Vec2::new(at, 0.0),
                Vec2::new(at + 1.0, 0.0),
                Vec2::NEG_Y,
            ),
            (
                index,
                49,
                Vec2::new(at, 50.0),
                Vec2::new(at + 1.0, 50.0),
                Vec2::Y,
            ),
        ];
        for (x, y, a, b, normal) in sides {
            if is_wall(x, y) {
                side_quad(&mut mesh, [a, b], [normal, normal], side);
            }
        }
    }
    mesh
}

/// A vertical wall quad from the ground to [`WALL_HEIGHT`] along `points` (ground X, Z),
/// facing outward with per-end `normals` (smooth along arcs).
fn side_quad(
    mesh: &mut Mesh<TerrainVertex>,
    points: [Vec2; 2],
    normals: [Vec2; 2],
    color: [f32; 4],
) {
    let Ok(base) = u32::try_from(mesh.vertices.len()) else {
        return;
    };
    let [a, b] = points;
    let [na, nb] = normals.map(|normal| Vec3::new(normal.x, 0.0, normal.y));
    let vertex = |point: Vec2, height: f32, normal: Vec3| TerrainVertex {
        position: [point.x, height, point.y],
        normal: normal.to_array(),
        color,
    };
    mesh.vertices.extend([
        vertex(a, 0.0, na),
        vertex(b, 0.0, nb),
        vertex(b, WALL_HEIGHT, nb),
        vertex(a, WALL_HEIGHT, na),
    ]);
    // Counter-clockwise seen from outside.
    let corners = [
        Vec3::new(a.x, 0.0, a.y),
        Vec3::new(b.x, 0.0, b.y),
        Vec3::new(b.x, WALL_HEIGHT, b.y),
    ];
    let facing = (corners[1] - corners[0])
        .cross(corners[2] - corners[0])
        .dot(na + nb);
    if facing >= 0.0 {
        mesh.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    } else {
        mesh.indices
            .extend([base, base + 2, base + 1, base, base + 3, base + 2]);
    }
}
