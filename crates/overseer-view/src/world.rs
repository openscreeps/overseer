//! The world as drawn, without the GPU: room instances, resident terrain, the overview layer,
//! minimaps and detail scenes, each recording which of its GPU data changed so that updates
//! only touch the rooms that changed.

pub mod cells;
mod minimap;
mod overview;
pub mod terrain;

use std::ops::Range;

use glam::Vec2;
use overseer::RoomSnapshot;
use overseer::core::api::Terrain;
use overseer::map::{RoomCoord, RoomMap, RoomRect, RoomStat};

pub use cells::Cells;
pub use minimap::{Minimaps, POINTS};
pub use overview::Overview;
pub use terrain::TerrainStore;

use crate::camera::{room_centre, room_origin};
use crate::overlay::{self, Overlay};
use crate::scene::motion::TickClock;
use crate::scene::{Eye, Features, Frame, Scene, Timing};
use crate::text::Atlas;

/// Detail scenes kept at once: the rooms of [`DETAIL_ROOMS`](crate::DETAIL_ROOMS) plus room
/// for those the interest is moving away from.
pub const DETAIL_SLOTS: usize = 4;
/// How long a detail scene fades in after its first snapshot, in milliseconds.
const ARRIVAL_MS: f64 = 300.0;

/// The device's texture limits that size the terrain texture and the overview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest side of a 2D texture, in texels.
    pub texture_side: u32,
    /// Most layers of a 2D array texture.
    pub array_layers: u32,
}

impl Limits {
    /// The limits every WebGL2 device has (wgpu's downlevel WebGL2 defaults).
    #[cfg(test)]
    pub const WEBGL2: Self = Self {
        texture_side: 2048,
        array_layers: 256,
    };

    /// The limits of a device.
    #[cfg_attr(
        not(any(target_arch = "wasm32", test)),
        expect(
            dead_code,
            reason = "only the wasm canvas constructor and the tests create a renderer"
        )
    )]
    pub const fn of(limits: &wgpu::Limits) -> Self {
        Self {
            texture_side: limits.max_texture_dimension_2d,
            array_layers: limits.max_texture_array_layers,
        }
    }
}

/// Texels for a texture: a rectangle of one mip level and array layer, and its bytes
/// (`data` starts at the first texel; rows are `bytes_per_row` apart).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upload<'a> {
    /// Mip level.
    pub level: u32,
    /// Array layer.
    pub layer: u32,
    /// Texel of the north-west corner.
    pub origin: (u32, u32),
    /// Width and height in texels.
    pub size: (u32, u32),
    /// Bytes from one row to the next in `data`.
    pub bytes_per_row: u32,
    /// The texels.
    pub data: &'a [u8],
}

/// Where the camera is, for the nearest-first policies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Focus {
    /// The scope's rooms.
    pub bounds: RoomRect,
    /// The ground point looked at (world X, Z).
    pub centre: Vec2,
}

impl Focus {
    /// Distance from the camera's centre to a room's centre; infinite outside the bounds.
    pub fn distance(&self, room: RoomCoord) -> f32 {
        if self.bounds.contains(room) {
            room_centre(room).distance(self.centre)
        } else {
            f32::INFINITY
        }
    }
}

/// Indices whose GPU copy is stale.
#[derive(Debug, Default)]
pub struct Dirty {
    all: bool,
    items: Vec<u32>,
}

impl Dirty {
    /// Marks one index.
    pub fn mark(&mut self, index: u32) {
        if !self.all {
            self.items.push(index);
        }
    }

    /// Marks everything.
    pub fn mark_all(&mut self) {
        self.all = true;
        self.items.clear();
    }

    /// Hands the marked indices below `len` as merged runs and clears the marks.
    pub fn drain(&mut self, len: usize, mut run: impl FnMut(Range<usize>)) {
        if std::mem::take(&mut self.all) {
            if len > 0 {
                run(0..len);
            }
            self.items.clear();
            return;
        }
        self.items.sort_unstable();
        self.items.dedup();
        let mut current: Option<Range<usize>> = None;
        for &index in &self.items {
            let index = index as usize;
            if index >= len {
                break;
            }
            current = match current {
                Some(range) if range.end == index => Some(range.start..index + 1),
                Some(range) => {
                    run(range);
                    Some(index..index + 1)
                }
                None => Some(index..index + 1),
            };
        }
        if let Some(range) = current {
            run(range);
        }
        self.items.clear();
    }

    /// The marks, as `(everything, sorted indices)`.
    #[cfg(test)]
    pub fn marks(&self) -> (bool, Vec<u32>) {
        let mut items = self.items.clone();
        items.sort_unstable();
        items.dedup();
        (self.all, items)
    }
}

/// A room drawn in full detail.
#[derive(Debug)]
pub struct Detail {
    /// Unique per detail scene created (GPU resources follow it).
    pub id: u64,
    /// The room.
    pub room: RoomCoord,
    /// The scene, in room-local tiles.
    pub scene: Scene,
    /// When the first snapshot arrived (ms).
    since: f64,
    /// The latest frame of the scene.
    pub frame: Frame,
}

impl Detail {
    /// Fade-in after the first snapshot, `0..=1`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a fraction in 0..=1 fits f32"
    )]
    pub fn arrival(&self, now: f64) -> f32 {
        ((now - self.since) / ARRIVAL_MS).clamp(0.0, 1.0) as f32
    }

    /// Opacity of the scene at `now` with the detail layer at `layer` (`Fades::detail`);
    /// its room's minimap points are drawn at the rest.
    pub fn opacity(&self, layer: f32, now: f64) -> f32 {
        layer * self.arrival(now)
    }
}

/// The drawn world.
#[derive(Debug)]
pub struct World {
    me: Option<String>,
    limits: Limits,
    focus: Focus,
    /// Room instances of the bounds.
    pub cells: Cells,
    /// Every room's terrain (resident).
    pub terrain: TerrainStore,
    /// The overview layer of the bounds.
    pub overview: Overview,
    /// Minimaps.
    pub minimaps: Minimaps,
    details: Vec<Option<Detail>>,
    next_id: u64,
    /// Tick duration measured from snapshot arrivals.
    clock: TickClock,
    /// Glyphs of room visual text.
    pub atlas: Atlas,
    /// Badges, portals, labels and the hover outline.
    pub overlay: Overlay,
    features: Features,
}

impl World {
    /// An empty world over `bounds` for a device with `limits`.
    pub fn new(bounds: RoomRect, limits: Limits) -> Self {
        let mut terrain = TerrainStore::new(limits.array_layers);
        terrain.reserve(bounds.len());
        Self {
            me: None,
            limits,
            focus: Focus {
                bounds,
                centre: room_centre(bounds.min),
            },
            cells: Cells::new(bounds),
            terrain,
            overview: Overview::new(bounds, limits.texture_side),
            minimaps: Minimaps::default(),
            details: (0..DETAIL_SLOTS).map(|_| None).collect(),
            next_id: 1,
            clock: TickClock::default(),
            atlas: Atlas::browser(),
            overlay: Overlay::new(limits.array_layers),
            features: Features::default(),
        }
    }

    /// The scope's rooms.
    pub const fn bounds(&self) -> RoomRect {
        self.focus.bounds
    }

    /// Lays the world over new bounds; reserves terrain room for all of them.
    pub fn set_bounds(&mut self, bounds: RoomRect) {
        if bounds == self.focus.bounds {
            return;
        }
        self.focus.bounds = bounds;
        let store = &mut self.terrain;
        let missing = bounds
            .iter()
            .filter(|room| !store.has_terrain(*room))
            .count();
        store.reserve(missing);
        let store = &self.terrain;
        self.cells
            .set_bounds(bounds, self.me.as_deref(), |room| store.state(room));
        self.overview
            .set_bounds(bounds, self.limits.texture_side, store);
    }

    /// Sets where the camera looks.
    pub fn set_focus(&mut self, centre: Vec2) {
        self.focus.centre = centre;
    }

    /// Sets the signed-in user: recolours cells, tints, minimaps and scenes.
    pub fn set_user(&mut self, user: Option<String>) {
        if self.me == user {
            return;
        }
        self.me = user;
        let me = self.me.as_deref();
        self.cells.recolor(me);
        self.minimaps.recolor(me);
        for detail in self.details.iter_mut().flatten() {
            detail.scene.set_user(self.me.clone());
        }
    }

    /// Stores a room's terrain (kept for the world's life) and draws it.
    pub fn set_terrain(&mut self, room: RoomCoord, terrain: &Terrain) {
        if let Some(slot) = self.terrain.set(room, terrain) {
            self.cells.set_terrain(room, slot);
            self.overview.set_room(room, slot, &self.terrain);
        }
    }

    /// Records that the server has no terrain for a room: it is drawn as outside the world
    /// (unless its terrain is known).
    pub fn mark_no_terrain(&mut self, room: RoomCoord) {
        if self.terrain.mark_absent(room) {
            self.cells.set_terrain(room, terrain::state::ABSENT);
            self.overlay.room_changed(room);
        }
    }

    /// Sets (`None`: forgets) a room's overview.
    pub fn set_stat(&mut self, room: RoomCoord, stat: Option<&RoomStat>) {
        if self.cells.set_stat(room, stat, self.me.as_deref()) {
            self.overlay.owners_changed();
        }
    }

    /// Brings the overlay up to date for `frame` (labels for the view, badges and portal
    /// marks for what changed).
    pub fn update_overlay(&mut self, frame: &overlay::Frame) {
        let terrain = &self.terrain;
        self.overlay.update(
            frame,
            &self.cells,
            |room| terrain.state(room) == terrain::state::ABSENT,
            &mut self.atlas,
        );
    }

    /// Shows (`Some`) or removes (`None`) a room's minimap.
    pub fn set_minimap(&mut self, room: RoomCoord, map: Option<&RoomMap>) {
        self.minimaps
            .set(room, map, self.me.as_deref(), &self.focus);
    }

    /// Shows a snapshot as the detail scene of its room (and stores its terrain). Returns
    /// `false` for rooms without world coordinates (they are not shown).
    pub fn set_room(&mut self, snapshot: &RoomSnapshot, now: f64) -> bool {
        let Ok(room) = snapshot.room.parse::<RoomCoord>() else {
            return false;
        };
        if !self.terrain.has_terrain(room) {
            self.set_terrain(room, &snapshot.terrain);
        }
        let index = self
            .details
            .iter()
            .position(|detail| detail.as_ref().is_some_and(|detail| detail.room == room))
            .or_else(|| self.details.iter().position(Option::is_none))
            .or_else(|| {
                // Every slot taken: replace the scene farthest from the camera.
                self.details
                    .iter()
                    .enumerate()
                    .filter_map(|(index, detail)| {
                        Some((self.focus.distance(detail.as_ref()?.room), index))
                    })
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, index)| index)
            });
        let Some(index) = index else {
            return false;
        };
        let slot = &mut self.details[index];
        if slot.as_ref().is_none_or(|detail| detail.room != room) {
            let mut scene = Scene::default();
            scene.set_user(self.me.clone());
            scene.set_features(self.features);
            *slot = Some(Detail {
                id: self.next_id,
                room,
                scene,
                since: now,
                frame: Frame::default(),
            });
            self.next_id += 1;
            self.clock.forget(room);
        }
        self.clock.observe(room, snapshot.game_time, now);
        let timing = Timing {
            now,
            tick_ms: self.clock.tick_ms(),
            glide_ms: self.clock.glide_ms(),
        };
        if let Some(detail) = slot {
            detail.scene.set_room(snapshot, timing, &mut self.atlas);
        }
        true
    }

    /// Removes a room's detail scene.
    pub fn clear_room(&mut self, room: RoomCoord) {
        for slot in &mut self.details {
            if slot.as_ref().is_some_and(|detail| detail.room == room) {
                *slot = None;
            }
        }
        self.clock.forget(room);
    }

    /// Sets the feature switches of every scene.
    pub fn set_features(&mut self, features: Features) {
        self.features = features;
        for detail in self.details.iter_mut().flatten() {
            detail.scene.set_features(features);
        }
    }

    /// The measured tick duration (ms).
    pub fn tick_ms(&self) -> f64 {
        self.clock.tick_ms()
    }

    /// The detail scene of a room.
    pub fn detail(&self, room: RoomCoord) -> Option<&Detail> {
        self.details
            .iter()
            .flatten()
            .find(|detail| detail.room == room)
    }

    /// The detail slots.
    pub fn details(&self) -> &[Option<Detail>] {
        &self.details
    }

    /// The slot (index in [`Self::details`]) of a room's detail scene: its minimap points
    /// fade out as the scene fades in ([`Detail::opacity`]), the scene drawing every object
    /// at its true size where the minimap has coarse squares.
    pub fn detail_slot(&self, room: RoomCoord) -> Option<usize> {
        self.details
            .iter()
            .position(|detail| detail.as_ref().is_some_and(|detail| detail.room == room))
    }

    /// Advances the detail scenes to `now` (ms) seen from `eye` (world coordinates).
    /// Returns the objects drawn and whether anything still moves, fades or plays.
    pub fn frame(&mut self, now: f64, eye: &Eye) -> (usize, bool) {
        let (mut objects, mut animating) = (0, false);
        for detail in self.details.iter_mut().flatten() {
            if !self.focus.bounds.contains(detail.room) {
                continue;
            }
            let origin = room_origin(detail.room);
            let local = Eye {
                position: eye.position - glam::Vec3::new(origin.x, 0.0, origin.y),
                ..*eye
            };
            detail.frame = detail.scene.frame(now, &local);
            objects += detail.scene.object_count();
            animating |= detail.frame.animating || detail.arrival(now) < 1.0;
        }
        (objects, animating)
    }

    /// The column height of a world tile: the detail scene's terrain where there is one,
    /// the ground elsewhere; `None` outside the bounds.
    pub fn height(&self, tile: (i32, i32)) -> Option<f32> {
        let (room, x, y) = split(tile);
        if !self.focus.bounds.contains(room) {
            return None;
        }
        Some(
            self.detail(room)
                .map_or(0.0, |detail| detail.scene.tile_height(x, y)),
        )
    }

    /// The terrain tint of a room.
    pub fn tint(&self, room: RoomCoord) -> [u8; 4] {
        self.cells.tint(room, self.me.as_deref())
    }
}

/// A world tile as its room and room-local tile.
pub fn split(tile: (i32, i32)) -> (RoomCoord, u8, u8) {
    let room = RoomCoord {
        x: tile.0.div_euclid(50),
        y: tile.1.div_euclid(50),
    };
    let local = |value: i32| u8::try_from(value.rem_euclid(50)).unwrap_or(0);
    (room, local(tile.0), local(tile.1))
}

/// The world position (X, Z) of a tile's north-west corner.
pub fn tile_origin(room: RoomCoord, x: u8, y: u8) -> Vec2 {
    room_origin(room) + Vec2::new(f32::from(x), f32::from(y))
}
