//! Vertex and instance formats, the silhouettes of objects (and their prism meshes) and the
//! raised-wall mesh of detail scenes.

pub mod contour;

use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3};
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
const WALL_SIDE: u32 = 0x24_24_24;

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

/// A vertex of an object prism: position in the unit footprint (`-0.5..0.5` across, `0..1`
/// up) and normal.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ShapeVertex {
    /// Position in the unit footprint.
    pub position: [f32; 3],
    /// Normal.
    pub normal: [f32; 3],
}

/// One drawn prism (the sides of an object in perspective, the selection frame): its
/// footprint centre, size and linear colour.
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

/// The top of an object: a square on the ground (or on its prism) whose fragment shader
/// draws the object's emblem (`objects.wgsl`), and the source of its drop shadow.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Token {
    /// Centre of the top, room-local (x, height, z).
    pub position: [f32; 3],
    /// Footprint width of the silhouette, in tiles (the drawn square is at least a tile).
    pub size: f32,
    /// Emblem (low byte, [`Glyph`](crate::scene::style::Glyph)), neighbour links (second
    /// byte), silhouette ([`Shape`], third byte) and flags (high byte, `scene::style::flag`).
    pub kind: u32,
    /// Owner colour (sRGB); alpha 0 for unowned objects.
    pub accent: [u8; 4],
    /// Colour of the stored resource (sRGB); alpha: how full the store is.
    pub fill: [u8; 4],
    /// Hits (255: undamaged, no bar), progress, controller level, a spare byte.
    pub gauges: [u8; 4],
    /// Body part counts of a creep: move, work, carry, attack.
    pub parts_a: [u8; 4],
    /// Body part counts of a creep: ranged attack, heal, claim, tough.
    pub parts_b: [u8; 4],
    /// Opacity (fades in and out).
    pub fade: f32,
    /// Height of the ground under the object (its shadow falls there).
    pub base: f32,
}

/// An action effect: a beam, bolt or burst between two room-local points.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct EffectInstance {
    /// Start (the actor), room-local.
    pub from: [f32; 3],
    /// Progress of the effect, `0..1`.
    pub progress: f32,
    /// End (the target), room-local.
    pub to: [f32; 3],
    /// Width (beams) or radius (bursts), tiles.
    pub width: f32,
    /// Colour (sRGB) and strength.
    pub color: [u8; 4],
    /// How it is drawn (`scene::effects::Look`).
    pub look: u32,
}

/// A room visual shape (line, circle, rectangle) drawn with a signed distance field.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct VisualInstance {
    /// Line: start; circle: centre; rectangle: north-west corner (room-local tiles).
    pub a: [f32; 2],
    /// Line: end; circle: unused; rectangle: south-east corner.
    pub b: [f32; 2],
    /// Line: width; circle: radius; rectangle: unused.
    pub size: f32,
    /// Outline width (0: none).
    pub stroke_width: f32,
    /// Fill colour (sRGB, alpha = opacity; 0: none).
    pub fill: [u8; 4],
    /// Outline colour (sRGB, alpha = opacity).
    pub stroke: [u8; 4],
    /// Kind (low byte: 0 line, 1 circle, 2 rectangle) and dash style (second byte: 0 solid,
    /// 1 dashed, 2 dotted).
    pub kind: u32,
}

/// A vertex of a filled room visual polygon.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct FillVertex {
    /// Room-local position (tiles).
    pub position: [f32; 2],
    /// sRGB colour and opacity.
    pub color: [u8; 4],
}

/// A glyph of room visual text: a rectangle of the glyph atlas.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct GlyphInstance {
    /// Room-local rectangle: north-west corner and size (tiles).
    pub rect: [f32; 4],
    /// Atlas rectangle: corner and size (`0..1`).
    pub uv: [f32; 4],
    /// Text colour (sRGB, alpha = opacity).
    pub color: [u8; 4],
    /// Outline colour (sRGB, alpha = opacity; 0: none).
    pub stroke: [u8; 4],
    /// Outline width in distance-field units (`0..0.5`).
    pub stroke_width: f32,
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

/// Kinds of [`MarkInstance`] (`overlay.wgsl` draws each).
pub mod mark {
    /// An owner's badge, centred in their room.
    pub const BADGE: u32 = 0;
    /// A reserving user's badge, small and faded.
    pub const RESERVED: u32 = 1;
    /// A run of rooms whose portals lead within the shard (`overlay::portals`).
    pub const PORTAL: u32 = 2;
    /// A run of rooms whose portals lead to other shards.
    pub const SHARD_PORTAL: u32 = 3;
    /// The outline of the hovered room.
    pub const HOVER: u32 = 4;
    /// A portal tile leading within the shard.
    pub const PORTAL_TILE: u32 = 5;
    /// A portal tile leading to another shard.
    pub const SHARD_PORTAL_TILE: u32 = 6;
}

/// A mark of the overlay: a disc (badge, portal tile), capsule (a run of portal rooms) or
/// square (hover) centred on a ground point, sized in tiles within a range of CSS pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct MarkInstance {
    /// World X and Z of the centre.
    pub centre: [f32; 2],
    /// Radius in tiles, then its least and greatest size in CSS pixels.
    pub radius: [f32; 3],
    /// Badge atlas rectangle: corner and side (`0..1`). Runs of portal rooms: the half
    /// of the capsule's spine in tiles (`x`, `y`; zero for a disc).
    pub uv: [f32; 4],
    /// sRGB colour and opacity.
    pub color: [u8; 4],
    /// One of [`mark`].
    pub kind: u32,
}

/// A thin arc of the overlay on the ground between two points (linked portals).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ArcInstance {
    /// World X and Z of one end.
    pub from: [f32; 2],
    /// World X and Z of the other end.
    pub to: [f32; 2],
    /// sRGB colour and opacity.
    pub color: [u8; 4],
    /// How far the middle bows out, as a share of the length (to the left of `from` →
    /// `to`).
    pub bend: f32,
}

/// A glyph of an overlay label: a rectangle of the glyph atlas at a CSS-pixel offset from
/// a ground point, so that labels keep their size at every zoom.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct LabelGlyph {
    /// World X and Z of the label's anchor.
    pub anchor: [f32; 2],
    /// Rectangle from the anchor in CSS pixels (y down): left, top, width, height.
    pub rect: [f32; 4],
    /// Atlas rectangle: corner and size (`0..1`).
    pub uv: [f32; 4],
    /// Text colour (sRGB, alpha = opacity).
    pub color: [u8; 4],
    /// Halo colour (sRGB, alpha = opacity).
    pub halo: [u8; 4],
    /// Halo width in distance-field units (`0..0.5`).
    pub halo_width: f32,
    /// Room size (CSS pixels) from which the label is shown.
    pub appear: f32,
    /// Label class: its fade (`labels` in `overlay.wgsl`).
    pub class: u32,
}

/// sRGB `0xRRGGBB` with an alpha byte, as vertex bytes.
pub const fn rgba(rgb: u32, alpha: u8) -> [u8; 4] {
    let [_, r, g, b] = rgb.to_be_bytes();
    [r, g, b, alpha]
}

/// The silhouettes of objects, in prism-mesh order; [`Shape::Ring`] is the selection frame.
/// Each fits the unit footprint; `objects.wgsl` (`silhouette`) evaluates the same outlines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shape {
    /// A disc.
    Circle,
    /// A square with rounded corners.
    Square,
    /// A square standing on a corner.
    Diamond,
    /// A hexagon with flat north and south sides.
    Hexagon,
    /// An octagon.
    Octagon,
    /// A triangle pointing north.
    Triangle,
    /// A pentagon pointing north.
    Pentagon,
    /// A square frame (the selection).
    Ring,
}

/// A rounded regular polygon: sides, angle of the first side's normal (from north,
/// clockwise), apothem and corner radius, in the unit footprint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Polygon {
    /// Number of sides (0: a circle of radius `apothem`).
    pub sides: u8,
    /// Angle of the first side's outward normal, radians clockwise from north.
    pub angle: f32,
    /// Distance from the centre to the sides.
    pub apothem: f32,
    /// Corner radius.
    pub rounding: f32,
}

/// The direction of an angle measured clockwise from north (`-y`), in footprint
/// coordinates (x east, y south).
fn direction(angle: f32) -> Vec2 {
    Vec2::new(angle.sin(), -angle.cos())
}

impl Polygon {
    /// Signed distance from `p` (footprint coordinates) to the outline (`objects.wgsl`
    /// evaluates it; the tests check the outline against it).
    #[cfg(test)]
    pub fn sd(&self, p: Vec2) -> f32 {
        if self.sides == 0 {
            return p.length() - self.apothem;
        }
        let sector = std::f32::consts::TAU / f32::from(self.sides);
        let half = sector / 2.0;
        let angle = p.x.atan2(-p.y);
        let local = (angle - self.angle + half).rem_euclid(sector) - half;
        let length = p.length();
        let q = Vec2::new(length * local.cos(), (length * local.sin()).abs());
        let apothem = self.apothem - self.rounding;
        let x = q.x - apothem;
        let y = q.y - apothem * half.tan();
        Vec2::new(x, y.max(0.0)).length() * x.signum() - self.rounding
    }

    /// The outline, counter-clockwise on screen (x east, y south: clockwise in the plane),
    /// with outward normals.
    pub fn outline(&self) -> Vec<(Vec2, Vec2)> {
        if self.sides == 0 {
            return (0..32_u16)
                .map(|step| {
                    let normal = direction(f32::from(step) / 32.0 * std::f32::consts::TAU);
                    (normal * self.apothem, normal)
                })
                .collect();
        }
        let sector = std::f32::consts::TAU / f32::from(self.sides);
        let inner = (self.apothem - self.rounding) / (sector / 2.0).cos();
        let mut points = Vec::new();
        for side in 0..self.sides {
            let from = self.angle + sector * f32::from(side);
            let corner = direction(from + sector / 2.0) * inner;
            for step in 0..=4_u8 {
                let normal = direction(from + sector * f32::from(step) / 4.0);
                points.push((corner + normal * self.rounding, normal));
            }
        }
        points
    }
}

impl Shape {
    /// The prism shapes, in mesh order.
    pub const ALL: [Self; 8] = [
        Self::Circle,
        Self::Square,
        Self::Diamond,
        Self::Hexagon,
        Self::Octagon,
        Self::Triangle,
        Self::Pentagon,
        Self::Ring,
    ];

    /// The outline of a silhouette (`None` for the ring).
    pub fn polygon(self) -> Option<Polygon> {
        use std::f32::consts::{FRAC_PI_4, PI};
        let polygon = |sides, angle, apothem, rounding| Polygon {
            sides,
            angle,
            apothem,
            rounding,
        };
        Some(match self {
            Self::Circle => polygon(0, 0.0, 0.5, 0.0),
            Self::Square => polygon(4, 0.0, 0.5, 0.1),
            Self::Diamond => polygon(4, FRAC_PI_4, 0.5 * FRAC_PI_4.cos(), 0.05),
            Self::Hexagon => polygon(6, 0.0, 0.5 * (PI / 6.0).cos(), 0.05),
            Self::Octagon => polygon(8, 0.0, 0.5, 0.05),
            Self::Triangle => polygon(3, PI, 0.25, 0.06),
            Self::Pentagon => polygon(5, PI, 0.5 * (PI / 5.0).cos(), 0.05),
            Self::Ring => return None,
        })
    }

    /// The index of the shape (`Token::kind`'s third byte).
    #[expect(clippy::cast_possible_truncation, reason = "fewer than 256 shapes")]
    pub fn index(self) -> u32 {
        Self::ALL
            .iter()
            .position(|shape| *shape == self)
            .unwrap_or(0) as u32
    }
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

/// The prisms in one mesh, and the index range of each [`Shape`] (in [`Shape::ALL`] order).
pub fn shapes() -> (Mesh<ShapeVertex>, [std::ops::Range<u32>; 8]) {
    let mut mesh = Mesh::default();
    let mut ranges: [std::ops::Range<u32>; 8] = Default::default();
    for (shape, range) in Shape::ALL.into_iter().zip(&mut ranges) {
        let start = mesh.indices.len();
        match shape.polygon() {
            Some(polygon) => add_prism(&mut mesh, &polygon),
            None => add_ring(&mut mesh, 0.36),
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

/// A prism of a silhouette from the ground to height 1, with smooth side normals and a top
/// cap a little below 1 (the object's token covers it; far enough below not to fight it
/// for depth).
fn add_prism(mesh: &mut Mesh<ShapeVertex>, polygon: &Polygon) {
    let outline = polygon.outline();
    let Ok(base) = u32::try_from(mesh.vertices.len()) else {
        return;
    };
    let ground = |point: Vec2, height: f32| Vec3::new(point.x, height, point.y);
    for (point, normal) in &outline {
        let normal = Vec3::new(normal.x, 0.0, normal.y);
        mesh.vertices
            .push(shape_vertex(normal)(ground(*point, 0.0)));
        mesh.vertices
            .push(shape_vertex(normal)(ground(*point, 1.0)));
    }
    let count = index(outline.len());
    for side in 0..count {
        let next = (side + 1) % count;
        let (a, b) = (base + side * 2, base + next * 2);
        // Counter-clockwise seen from outside: the outline runs clockwise seen from above.
        let corners = [
            ground(outline[side as usize].0, 0.0),
            ground(outline[next as usize].0, 0.0),
            ground(outline[next as usize].0, 1.0),
        ];
        let outward = Vec3::new(
            outline[side as usize].1.x + outline[next as usize].1.x,
            0.0,
            outline[side as usize].1.y + outline[next as usize].1.y,
        );
        let facing = (corners[1] - corners[0])
            .cross(corners[2] - corners[0])
            .dot(outward);
        if facing >= 0.0 {
            mesh.indices.extend([a, b, b + 1, a, b + 1, a + 1]);
        } else {
            mesh.indices.extend([a, b + 1, b, a, a + 1, b + 1]);
        }
    }
    let cap = 0.94;
    for side in 0..outline.len() {
        let next = (side + 1) % outline.len();
        mesh.triangle(
            [
                Vec3::new(0.0, cap, 0.0),
                ground(outline[side].0, cap),
                ground(outline[next].0, cap),
            ],
            Vec3::Y,
            shape_vertex(Vec3::Y),
        );
    }
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

/// The raised walls of a detail scene (see [`contour`]).
pub fn terrain(terrain: &Terrain) -> Mesh<TerrainVertex> {
    contour::walls(terrain)
}
