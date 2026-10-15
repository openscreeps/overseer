//! The drawn room: terrain mesh, objects with their movement interpolation, ownership colours,
//! the selection and the per-frame instance list.

mod style;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use glam::Vec2;
use overseer::RoomSnapshot;
use overseer::core::api::{Terrain, Tile};

use crate::MOVE_DURATION_MS;
use crate::mesh::{self, Instance, Mesh, Shape, TerrainVertex};
use style::{Owner, Style};

/// Colour (sRGB) of the selection frame.
const SELECTION: u32 = 0xff_eb_3b;

/// An object of the room as drawn.
#[derive(Clone, Debug)]
struct Object {
    id: String,
    user: Option<String>,
    style: Style,
    /// The tile the object is on.
    tile: (u8, u8),
    /// Interpolation from `from` (tile coordinates) to `tile`, started at `start` (ms).
    from: Vec2,
    start: f64,
}

impl Object {
    /// Tile coordinates of the object at `now` (ms): eased from `from` to `tile` over
    /// [`MOVE_DURATION_MS`].
    fn position(&self, now: f64) -> Vec2 {
        let to = tile_vec(self.tile);
        let progress = progress(now, self.start);
        if progress >= 1.0 {
            return to;
        }
        // Smoothstep easing.
        let eased = progress * progress * (3.0 - 2.0 * progress);
        self.from.lerp(to, eased)
    }

    fn animating(&self, now: f64) -> bool {
        progress(now, self.start) < 1.0
    }
}

/// Elapsed fraction of a movement started at `start`, from 0 to 1.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a fraction in 0..=1 fits f32"
)]
fn progress(now: f64, start: f64) -> f32 {
    ((now - start) / MOVE_DURATION_MS).clamp(0.0, 1.0) as f32
}

fn tile_vec(tile: (u8, u8)) -> Vec2 {
    Vec2::new(f32::from(tile.0), f32::from(tile.1))
}

/// One shape of an object.
#[derive(Clone, Copy, Debug)]
struct Part {
    object: usize,
    width: f32,
    height: f32,
    color: [f32; 4],
}

/// The room as drawn.
#[derive(Debug, Default)]
pub struct Scene {
    /// `(shard, room)` of the objects shown.
    room: Option<(Option<String>, String)>,
    terrain: Option<Arc<Terrain>>,
    /// Bumped whenever the terrain mesh is rebuilt.
    terrain_version: u64,
    terrain_mesh: Mesh<TerrainVertex>,
    /// Terrain height per tile, indexed by `y * 50 + x`.
    heights: Vec<f32>,
    user: Option<String>,
    objects: Vec<Object>,
    /// Shapes of the objects, grouped by [`Shape`] in [`Shape::ALL`] order.
    parts: Vec<Part>,
    /// Range of `parts` per shape.
    part_ranges: [Range<usize>; 3],
    selected: Option<(u8, u8)>,
    /// The instances of the latest frame (reused: no allocation per frame).
    instances: Vec<Instance>,
}

/// What [`Scene::frame`] produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Instance range per [`Shape`] (in [`Shape::ALL`] order), selection excluded.
    pub ranges: [Range<u32>; 3],
    /// Index of the selection frame instance (a [`Shape::Ring`]).
    pub selection: Option<u32>,
    /// Whether an object is still moving.
    pub animating: bool,
}

impl Scene {
    /// Shows `snapshot` at time `now` (ms). Returns whether the terrain mesh was rebuilt.
    pub fn set_room(&mut self, snapshot: &RoomSnapshot, now: f64) -> bool {
        let room = (snapshot.shard.clone(), snapshot.room.clone());
        let same_room = self.room.as_ref() == Some(&room);
        self.room = Some(room);
        let rebuilt = self.set_terrain(&snapshot.terrain);
        let previous: HashMap<&str, &Object> = if same_room {
            self.objects
                .iter()
                .map(|object| (object.id.as_str(), object))
                .collect()
        } else {
            HashMap::new()
        };
        let mut objects = Vec::with_capacity(snapshot.objects.len());
        for object in &snapshot.objects {
            let (Some(x), Some(y)) = (coordinate(object.x()), coordinate(object.y())) else {
                continue;
            };
            let tile = (x, y);
            let (from, start) = match previous.get(object.id()) {
                // Unmoved: keep any movement in progress.
                Some(old) if old.tile == tile => (old.from, old.start),
                // Moved: from where it is drawn now.
                Some(old) => (old.position(now), now),
                // New: snapped in place.
                None => (tile_vec(tile), f64::NEG_INFINITY),
            };
            objects.push(Object {
                id: object.id().to_owned(),
                user: object.user().map(str::to_owned),
                style: Style::of(object),
                tile,
                from,
                start,
            });
        }
        self.objects = objects;
        self.build_parts();
        rebuilt
    }

    /// Replaces the terrain when it differs; returns whether the mesh was rebuilt.
    fn set_terrain(&mut self, terrain: &Arc<Terrain>) -> bool {
        if let Some(current) = &self.terrain
            && (Arc::ptr_eq(current, terrain) || current == terrain)
        {
            return false;
        }
        self.terrain_mesh = mesh::terrain(terrain);
        self.heights = (0..2500_u16)
            .map(|index| {
                let (x, y) = (index % 50, index / 50);
                let tile = u8::try_from(x)
                    .ok()
                    .zip(u8::try_from(y).ok())
                    .and_then(|(x, y)| terrain.get(x, y));
                mesh::tile_height(tile.unwrap_or(Tile::Plain))
            })
            .collect();
        self.terrain = Some(Arc::clone(terrain));
        self.terrain_version += 1;
        true
    }

    /// Sets the signed-in user (own objects are coloured differently).
    pub fn set_user(&mut self, user: Option<String>) {
        if self.user != user {
            self.user = user;
            self.build_parts();
        }
    }

    /// Highlights a tile.
    pub const fn select(&mut self, tile: Option<(u8, u8)>) {
        self.selected = tile;
    }

    /// Rebuilds the shapes of the objects, grouped by shape, and reserves the instances of
    /// a frame.
    fn build_parts(&mut self) {
        self.parts.clear();
        let mut start = 0;
        for (shape, range) in Shape::ALL.into_iter().zip(&mut self.part_ranges) {
            for (index, object) in self.objects.iter().enumerate() {
                if object.style.shape != shape {
                    continue;
                }
                let owner = Owner::of(object.user.as_deref(), self.user.as_deref());
                self.parts.push(Part {
                    object: index,
                    width: object.style.width,
                    height: object.style.height,
                    color: mesh::linear(object.style.color(owner)),
                });
            }
            *range = start..self.parts.len();
            start = self.parts.len();
        }
        // Every part plus the selection: `frame` never grows the list.
        self.instances
            .reserve((self.parts.len() + 1).saturating_sub(self.instances.len()));
    }

    /// Fills the instances of the frame at `now` (ms).
    pub fn frame(&mut self, now: f64) -> Frame {
        self.instances.clear();
        let mut animating = false;
        for part in &self.parts {
            let Some(object) = self.objects.get(part.object) else {
                continue;
            };
            animating |= object.animating(now);
            let position = object.position(now);
            let base = self.height_at(position);
            self.instances.push(Instance {
                offset: [position.x + 0.5, base, position.y + 0.5],
                scale: [part.width, part.height, part.width],
                color: part.color,
            });
        }
        let selection = self.selected.map(|tile| {
            let position = tile_vec(tile);
            let base = self.height_at(position);
            (position, base)
        });
        let selection = selection.map(|(position, base)| {
            self.instances.push(Instance {
                offset: [position.x + 0.5, base, position.y + 0.5],
                scale: [1.08, 0.12, 1.08],
                color: mesh::linear(SELECTION),
            });
            count(self.instances.len() - 1)
        });
        let range = |range: &Range<usize>| count(range.start)..count(range.end);
        Frame {
            ranges: [
                range(&self.part_ranges[0]),
                range(&self.part_ranges[1]),
                range(&self.part_ranges[2]),
            ],
            selection,
            animating,
        }
    }

    /// Terrain height under a position in tile coordinates (of its nearest tile).
    fn height_at(&self, position: Vec2) -> f32 {
        let tile = position.round();
        if !(0.0..50.0).contains(&tile.x) || !(0.0..50.0).contains(&tile.y) {
            return 0.0;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "whole numbers in 0..50"
        )]
        let index = tile.y as usize * 50 + tile.x as usize;
        self.heights.get(index).copied().unwrap_or(0.0)
    }

    /// Terrain height of a tile.
    pub fn tile_height(&self, x: u8, y: u8) -> f32 {
        self.heights
            .get(usize::from(y) * 50 + usize::from(x))
            .copied()
            .unwrap_or(0.0)
    }

    /// Ids of the objects on a tile, topmost first (higher layer first, then by id).
    pub fn objects_at(&self, tile: (u8, u8)) -> Vec<String> {
        let mut on_tile: Vec<&Object> = self
            .objects
            .iter()
            .filter(|object| object.tile == tile)
            .collect();
        on_tile.sort_by(|a, b| b.style.layer.cmp(&a.style.layer).then(a.id.cmp(&b.id)));
        on_tile
            .into_iter()
            .map(|object| object.id.clone())
            .collect()
    }

    /// The instances of the latest [`Self::frame`].
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Instances needed by a frame.
    pub const fn instance_capacity(&self) -> usize {
        self.parts.len() + 1
    }

    /// Number of objects shown.
    pub const fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// The terrain mesh and its version (bumped on every rebuild; 0 before any terrain).
    pub const fn terrain(&self) -> (&Mesh<TerrainVertex>, u64) {
        (&self.terrain_mesh, self.terrain_version)
    }

    /// The world position of a tile's centre on top of its terrain.
    #[cfg(test)]
    pub fn tile_centre(&self, x: u8, y: u8) -> glam::Vec3 {
        glam::Vec3::new(
            f32::from(x) + 0.5,
            self.tile_height(x, y),
            f32::from(y) + 0.5,
        )
    }
}

/// A room coordinate in `0..50`.
fn coordinate(value: u64) -> Option<u8> {
    u8::try_from(value).ok().filter(|value| *value < 50)
}

/// An instance index as `u32` (instance lists stay far below `u32::MAX`).
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
