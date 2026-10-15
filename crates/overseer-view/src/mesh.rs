//! Vertex formats, the object shapes and the terrain mesh.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use overseer::core::api::{Terrain, Tile};

/// Height of a raised natural wall.
pub const WALL_HEIGHT: f32 = 0.6;
/// Terrain colours (sRGB); the flat world shader uses the same.
pub const PLAIN: u32 = 0x2b_2b_2b;
pub const SWAMP: u32 = 0x2f_3a_1c;
pub const WALL_TOP: u32 = 0x11_11_11;
/// A room whose terrain has not arrived yet (sRGB): neutral, lighter than any terrain so
/// that loading shows.
pub const PENDING: u32 = 0x33_35_39;
const WALL_SIDE: u32 = 0x26_26_26;

/// A terrain vertex: position, normal and linear colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct TerrainVertex {
    /// World position.
    pub position: [f32; 3],
    /// Face normal.
    pub normal: [f32; 3],
    /// Linear RGBA.
    pub color: [f32; 4],
}

/// A vertex of an object shape: position in the unit footprint (`-0.5..0.5` across, `0..1`
/// up) and normal.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ShapeVertex {
    /// Position in the unit footprint.
    pub position: [f32; 3],
    /// Face normal.
    pub normal: [f32; 3],
}

/// One drawn object shape: its footprint centre, size and linear colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Instance {
    /// World position of the footprint centre at the base.
    pub offset: [f32; 3],
    /// Width, height and depth.
    pub scale: [f32; 3],
    /// Linear RGBA.
    pub color: [f32; 4],
}

/// A room of the flat world layer: its corner, its terrain and the overlays of its overview.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct RoomInstance {
    /// World X and Z of the room's north-west corner.
    pub origin: [f32; 2],
    /// The terrain slot, or `PENDING` / `ABSENT` (`world::terrain::state`).
    pub terrain: u32,
    /// sRGB colour and strength (alpha) of the status and owner over the overview terrain.
    pub overlay: [u8; 4],
    /// sRGB colour and strength (alpha) of the status and owner over full-resolution
    /// terrain.
    pub tint: [u8; 4],
}

/// A point of a room minimap: a square over a tile.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct PointInstance {
    /// World X and Z of the tile's north-west corner.
    pub position: [f32; 2],
    /// Side of the square, in tiles (centred on the tile).
    pub size: f32,
    /// sRGB colour; alpha is the opacity.
    pub color: [u8; 4],
}

/// sRGB `0xRRGGBB` with an alpha byte, as vertex bytes.
pub const fn rgba(rgb: u32, alpha: u8) -> [u8; 4] {
    let [_, r, g, b] = rgb.to_be_bytes();
    [r, g, b, alpha]
}

/// The shapes objects are drawn with, in instance-buffer order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shape {
    /// A box.
    Box,
    /// A 12-sided prism.
    Cylinder,
    /// A square frame (ramparts, extractors, the selection).
    Ring,
}

impl Shape {
    /// Every shape, in instance-buffer order.
    pub const ALL: [Self; 3] = [Self::Box, Self::Cylinder, Self::Ring];
}

/// Vertices and indices of a mesh; indices are absolute (no base vertex: WebGL2 lacks it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh<V> {
    /// The vertices.
    pub vertices: Vec<V>,
    /// Triangle list indices.
    pub indices: Vec<u32>,
}

impl<V> Mesh<V> {
    /// Adds a quad facing `normal`: the winding is fixed to be counter-clockwise seen from
    /// that side, whatever the corner order (corners go around the quad).
    fn quad(&mut self, corners: [Vec3; 4], normal: Vec3, vertex: impl Fn(Vec3) -> V) {
        let Ok(base) = u32::try_from(self.vertices.len()) else {
            return;
        };
        self.vertices.extend(corners.map(vertex));
        let facing = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .dot(normal);
        if facing >= 0.0 {
            self.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            self.indices
                .extend([base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }

    /// Adds a triangle facing `normal` (winding fixed like [`Self::quad`]).
    fn triangle(&mut self, corners: [Vec3; 3], normal: Vec3, vertex: impl Fn(Vec3) -> V) {
        let Ok(base) = u32::try_from(self.vertices.len()) else {
            return;
        };
        self.vertices.extend(corners.map(vertex));
        let facing = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .dot(normal);
        if facing >= 0.0 {
            self.indices.extend([base, base + 1, base + 2]);
        } else {
            self.indices.extend([base, base + 2, base + 1]);
        }
    }
}

/// The shapes in one mesh, and the index range of each [`Shape`] (in [`Shape::ALL`] order).
pub fn shapes() -> (Mesh<ShapeVertex>, [std::ops::Range<u32>; 3]) {
    let mut mesh = Mesh::default();
    let mut ranges = [0..0, 0..0, 0..0];
    for (shape, range) in Shape::ALL.into_iter().zip(&mut ranges) {
        let start = mesh.indices.len();
        match shape {
            Shape::Box => add_box(&mut mesh),
            Shape::Cylinder => add_cylinder(&mut mesh, 12),
            Shape::Ring => add_ring(&mut mesh, 0.36),
        }
        *range = index(start)..index(mesh.indices.len());
    }
    (mesh, ranges)
}

/// An index count as `u32` (meshes here stay far below `u32::MAX`).
fn index(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn shape_vertex(normal: Vec3) -> impl Fn(Vec3) -> ShapeVertex {
    move |position| ShapeVertex {
        position: position.to_array(),
        normal: normal.to_array(),
    }
}

/// A box over `-0.5..0.5` × `0..1` × `-0.5..0.5` without a bottom face.
fn add_box(mesh: &mut Mesh<ShapeVertex>) {
    let (low, high) = (-0.5, 0.5);
    let top = [
        Vec3::new(low, 1.0, low),
        Vec3::new(high, 1.0, low),
        Vec3::new(high, 1.0, high),
        Vec3::new(low, 1.0, high),
    ];
    mesh.quad(top, Vec3::Y, shape_vertex(Vec3::Y));
    add_walls(mesh, high, 1.0, Vec3::ONE);
}

/// The four outer (`sign` = 1) or inner (`sign` = -1, normals inward) side walls of a square
/// of half size `half`.
fn add_walls(mesh: &mut Mesh<ShapeVertex>, half: f32, height: f32, sign: Vec3) {
    for normal in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        let along = Vec3::new(normal.z, 0.0, normal.x) * half;
        let centre = normal * half;
        let corners = [
            centre - along,
            centre + along,
            centre + along + Vec3::Y * height,
            centre - along + Vec3::Y * height,
        ];
        let normal = normal * sign;
        mesh.quad(corners, normal, shape_vertex(normal));
    }
}

/// A prism of `sides` sides inscribed in the unit footprint, without a bottom face.
fn add_cylinder(mesh: &mut Mesh<ShapeVertex>, sides: u16) {
    let corner = |side: u16| {
        let angle = f32::from(side) / f32::from(sides) * std::f32::consts::TAU;
        Vec3::new(angle.cos() * 0.5, 0.0, angle.sin() * 0.5)
    };
    for side in 0..sides {
        let (a, b) = (corner(side), corner(side + 1));
        let normal = ((a + b) / 2.0).normalize_or(Vec3::X);
        mesh.quad(
            [a, b, b + Vec3::Y, a + Vec3::Y],
            normal,
            shape_vertex(normal),
        );
        mesh.triangle(
            [Vec3::Y, a + Vec3::Y, b + Vec3::Y],
            Vec3::Y,
            shape_vertex(Vec3::Y),
        );
    }
}

/// A square frame: outer half size 0.5, inner half size `inner`, height 1.
fn add_ring(mesh: &mut Mesh<ShapeVertex>, inner: f32) {
    for normal in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z] {
        let along = Vec3::new(normal.z, 0.0, normal.x);
        let top = [
            normal * 0.5 - along * 0.5 + Vec3::Y,
            normal * 0.5 + along * 0.5 + Vec3::Y,
            normal * inner + along * inner + Vec3::Y,
            normal * inner - along * inner + Vec3::Y,
        ];
        mesh.quad(top, Vec3::Y, shape_vertex(Vec3::Y));
    }
    add_walls(mesh, 0.5, 1.0, Vec3::ONE);
    add_walls(mesh, inner, 1.0, Vec3::NEG_ONE);
}

/// Linear RGBA of an sRGB `0xRRGGBB` colour.
pub fn linear(rgb: u32) -> [f32; 4] {
    let channel = |shift: u32| {
        let value = f32::from(u8::try_from((rgb >> shift) & 0xff).unwrap_or(u8::MAX)) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    [channel(16), channel(8), channel(0), 1.0]
}

/// The terrain's height at a tile.
pub fn tile_height(tile: Tile) -> f32 {
    match tile {
        Tile::Plain | Tile::Swamp => 0.0,
        Tile::Wall => WALL_HEIGHT,
    }
}

/// The terrain mesh: one top quad per tile and the sides of walls next to lower tiles or the
/// room edge.
pub fn terrain(terrain: &Terrain) -> Mesh<TerrainVertex> {
    let mut mesh = Mesh::default();
    let is_wall = |x: i16, y: i16| {
        u8::try_from(x)
            .ok()
            .zip(u8::try_from(y).ok())
            .and_then(|(x, y)| terrain.get(x, y))
            == Some(Tile::Wall)
    };
    for y in 0..50_u8 {
        for x in 0..50_u8 {
            let tile = terrain.get(x, y).unwrap_or(Tile::Plain);
            let height = tile_height(tile);
            let color = linear(match tile {
                Tile::Plain => PLAIN,
                Tile::Swamp => SWAMP,
                Tile::Wall => WALL_TOP,
            });
            let (left, top) = (f32::from(x), f32::from(y));
            let corners = [
                Vec3::new(left, height, top),
                Vec3::new(left + 1.0, height, top),
                Vec3::new(left + 1.0, height, top + 1.0),
                Vec3::new(left, height, top + 1.0),
            ];
            mesh.quad(corners, Vec3::Y, terrain_vertex(Vec3::Y, color));
            if tile != Tile::Wall {
                continue;
            }
            let side = linear(WALL_SIDE);
            let centre = Vec3::new(left + 0.5, 0.0, top + 0.5);
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                if is_wall(i16::from(x) + dx, i16::from(y) + dy) {
                    continue;
                }
                let normal = Vec3::new(f32::from(dx), 0.0, f32::from(dy));
                let along = Vec3::new(normal.z, 0.0, normal.x) * 0.5;
                let base = centre + normal * 0.5;
                let corners = [
                    base - along,
                    base + along,
                    base + along + Vec3::Y * height,
                    base - along + Vec3::Y * height,
                ];
                mesh.quad(corners, normal, terrain_vertex(normal, side));
            }
        }
    }
    mesh
}

fn terrain_vertex(normal: Vec3, color: [f32; 4]) -> impl Fn(Vec3) -> TerrainVertex {
    move |position| TerrainVertex {
        position: position.to_array(),
        normal: normal.to_array(),
        color,
    }
}
