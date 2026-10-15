//! The rooms of the bounds as drawn by the flat world layer: one instance per room with its
//! terrain state and the status and owner overlays of every room seen.

use std::collections::HashMap;

use overseer::map::{RoomCoord, RoomRect, RoomStat, RoomStatus};

use super::Dirty;
use crate::camera::room_origin;
use crate::mesh::{RoomInstance, rgba};
use crate::scene::style::Owner;

/// Status colours (sRGB) of novice and respawn areas and closed rooms.
const NOVICE: u32 = 0x2b_5e_4b;
const RESPAWN: u32 = 0x2d_48_72;
const CLOSED: u32 = 0x11_11_11;
const OUT_OF_BORDERS: u32 = 0x07_07_07;
/// Overlay strengths (of 255) over the overview terrain: owners stand out zoomed out while
/// the terrain still shows. An owned room's is `OWNED + OWNED_PER_LEVEL × RCL`.
const OWNED: u8 = 110;
const OWNED_PER_LEVEL: u8 = 10;
const RESERVED: u8 = 80;
const AREA: u8 = 100;
const CLOSED_OVERLAY: u8 = 170;
/// Accent strengths (of 255) over full-resolution terrain (`accent` in `common.wgsl`): the
/// terrain takes the colour's hue at its own brightness (dark colours dim it), so walls,
/// swamps and plains keep their contrast; a line of the colour runs inside the room border.
/// The accent fades out at [`Lod::Detail`](crate::Lod::Detail).
const OWNED_TINT: u8 = 64;
const RESERVED_TINT: u8 = 40;
const AREA_TINT: u8 = 56;
const CLOSED_TINT: u8 = 150;

/// What the overview keeps of a [`RoomStat`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct CellStat {
    status: RoomStatus,
    /// Owner id and controller level.
    owner: Option<(String, u8)>,
    /// Reserving user id.
    reserved: Option<String>,
}

impl CellStat {
    fn of(stat: &RoomStat) -> Self {
        Self {
            status: stat.status,
            owner: stat
                .owner
                .as_ref()
                .map(|owner| (owner.id.clone(), owner.level)),
            reserved: stat
                .reservation
                .as_ref()
                .map(|reservation| reservation.id.clone()),
        }
    }

    /// The colour of the owner, else of the reserving user, with the strengths to use for
    /// each (`(overlay, tint)`).
    fn owner(&self, me: Option<&str>) -> Option<(u32, u8, u8)> {
        let color = |user: &str| Owner::of(Some(user), me).color();
        if let Some((id, level)) = &self.owner
            && let Some(color) = color(id)
        {
            let overlay = OWNED.saturating_add(OWNED_PER_LEVEL.saturating_mul(*level));
            return Some((color, overlay, OWNED_TINT));
        }
        self.reserved
            .as_deref()
            .and_then(color)
            .map(|color| (color, RESERVED, RESERVED_TINT))
    }

    /// The status colour with the strengths to use for it (`(overlay, tint)`); none for
    /// normal rooms.
    const fn status(&self) -> Option<(u32, u8, u8)> {
        match self.status {
            RoomStatus::Normal => None,
            RoomStatus::Novice => Some((NOVICE, AREA, AREA_TINT)),
            RoomStatus::Respawn => Some((RESPAWN, AREA, AREA_TINT)),
            RoomStatus::Closed => Some((CLOSED, CLOSED_OVERLAY, CLOSED_TINT)),
            RoomStatus::OutOfBorders => Some((OUT_OF_BORDERS, CLOSED_OVERLAY, CLOSED_TINT)),
        }
    }

    /// The overlay over the overview terrain and the tint over full-resolution terrain.
    fn colors(&self, me: Option<&str>) -> ([u8; 4], [u8; 4]) {
        match self.owner(me).or_else(|| self.status()) {
            Some((color, overlay, tint)) => (rgba(color, overlay), rgba(color, tint)),
            None => ([0; 4], [0; 4]),
        }
    }
}

/// The room instances of the bounds (row-major from the north-west corner) and the
/// overview of every room seen.
#[derive(Debug)]
pub struct Cells {
    bounds: RoomRect,
    stats: HashMap<RoomCoord, CellStat>,
    instances: Vec<RoomInstance>,
    /// Rooms whose instance changed since the GPU took them.
    pub dirty: Dirty,
}

impl Cells {
    /// Cells of `bounds`, all unknown and without terrain.
    pub fn new(bounds: RoomRect) -> Self {
        let mut cells = Self {
            bounds,
            stats: HashMap::new(),
            instances: Vec::new(),
            dirty: Dirty::default(),
        };
        cells.set_bounds(bounds, None, |_| super::terrain::state::PENDING);
        cells
    }

    /// Re-lays the cells over `bounds`; `terrain` gives each room's terrain state.
    pub fn set_bounds(
        &mut self,
        bounds: RoomRect,
        me: Option<&str>,
        terrain: impl Fn(RoomCoord) -> u32,
    ) {
        self.bounds = bounds;
        self.instances.clear();
        self.instances.reserve(bounds.len());
        for room in bounds.iter() {
            let (overlay, tint) = self.colors(room, me);
            self.instances.push(RoomInstance {
                origin: room_origin(room).to_array(),
                terrain: terrain(room),
                overlay,
                tint,
            });
        }
        self.dirty.mark_all();
    }

    /// Recolours every cell (the signed-in user changed).
    pub fn recolor(&mut self, me: Option<&str>) {
        for (index, room) in self.bounds.iter().enumerate() {
            let (overlay, tint) = self.colors(room, me);
            if let Some(instance) = self.instances.get_mut(index) {
                (instance.overlay, instance.tint) = (overlay, tint);
            }
        }
        self.dirty.mark_all();
    }

    fn colors(&self, room: RoomCoord, me: Option<&str>) -> ([u8; 4], [u8; 4]) {
        self.stats
            .get(&room)
            .map_or(([0; 4], [0; 4]), |stat| stat.colors(me))
    }

    /// The index of a room's cell.
    pub fn index(&self, room: RoomCoord) -> Option<usize> {
        if !self.bounds.contains(room) {
            return None;
        }
        let column = usize::try_from(room.x - self.bounds.min.x).ok()?;
        let row = usize::try_from(room.y - self.bounds.min.y).ok()?;
        let width = usize::try_from(self.bounds.width()).ok()?;
        Some(row * width + column)
    }

    /// The instance of a room in the bounds, to change (marked as such).
    fn instance_mut(&mut self, room: RoomCoord) -> Option<&mut RoomInstance> {
        let index = self.index(room)?;
        let instance = self.instances.get_mut(index)?;
        self.dirty.mark(u32::try_from(index).unwrap_or(u32::MAX));
        Some(instance)
    }

    /// Sets (`None`: forgets) a room's overview. Returns whether it changed.
    pub fn set_stat(&mut self, room: RoomCoord, stat: Option<&RoomStat>, me: Option<&str>) -> bool {
        let stat = stat.map(CellStat::of);
        if self.stats.get(&room) == stat.as_ref() {
            return false;
        }
        match stat {
            Some(stat) => self.stats.insert(room, stat),
            None => self.stats.remove(&room),
        };
        let (overlay, tint) = self.colors(room, me);
        if let Some(instance) = self.instance_mut(room) {
            (instance.overlay, instance.tint) = (overlay, tint);
        }
        true
    }

    /// Sets a room's terrain state (its slot, `PENDING` or `ABSENT`).
    pub fn set_terrain(&mut self, room: RoomCoord, terrain: u32) {
        if self
            .index(room)
            .and_then(|index| self.instances.get(index))
            .is_some_and(|instance| instance.terrain != terrain)
            && let Some(instance) = self.instance_mut(room)
        {
            instance.terrain = terrain;
        }
    }

    /// The terrain tint of a room (transparent without an overview), for detail scenes.
    pub fn tint(&self, room: RoomCoord, me: Option<&str>) -> [u8; 4] {
        self.colors(room, me).1
    }

    /// The room instances, row-major over the bounds.
    pub fn instances(&self) -> &[RoomInstance] {
        &self.instances
    }
}

/// The status and owner accent of `tint` (sRGB and strength) at `strength` (the accent's
/// fade) over linear `color`: what `accent` in `common.wgsl` computes.
#[cfg(test)]
pub fn accent(color: glam::Vec3, tint: [u8; 4], strength: f32) -> glam::Vec3 {
    let luminance = |color: glam::Vec3| color.dot(glam::Vec3::new(0.2126, 0.7152, 0.0722));
    let [r, g, b, _] = crate::mesh::linear(u32::from_be_bytes([0, tint[0], tint[1], tint[2]]));
    let rgb = glam::Vec3::new(r, g, b);
    let t = (luminance(rgb) / 0.05).clamp(0.0, 1.0);
    let hued = color * rgb;
    let kept = hued * (luminance(color) / luminance(hued).max(1e-8));
    color.lerp(
        kept * (t * t * (3.0 - 2.0 * t)),
        f32::from(tint[3]) / 255.0 * strength,
    )
}
