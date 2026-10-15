//! The overview: one cell per room of the bounds, coloured by status and owner, and the
//! terrain tint derived from the same overview.

use std::collections::HashMap;

use overseer::map::{RoomCoord, RoomRect, RoomStat, RoomStatus};

use super::Dirty;
use crate::camera::room_origin;
use crate::mesh::{CellInstance, mix, rgba};
use crate::scene::style::Owner;

/// Cell colours (sRGB) by status, before ownership.
const UNKNOWN: u32 = 0x23_23_23;
const NORMAL: u32 = 0x3a_3a_3a;
const NOVICE: u32 = 0x2b_5e_4b;
const RESPAWN: u32 = 0x2d_48_72;
const CLOSED: u32 = 0x11_11_11;
const OUT_OF_BORDERS: u32 = 0x07_07_07;
/// Share of the owner colour in an owned room's cell: `OWNED + OWNED_PER_LEVEL × RCL`.
const OWNED: f32 = 0.55;
const OWNED_PER_LEVEL: f32 = 0.05;
/// Share of the reserving player's colour in a reserved room's cell.
const RESERVED: f32 = 0.35;
/// Tint strengths (of 255) over the terrain; the mix is linear, so bright owner colours
/// need little to show while the terrain stays readable.
const OWNED_TINT: u8 = 36;
const RESERVED_TINT: u8 = 20;
const AREA_TINT: u8 = 32;
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

    /// The cell colour (sRGB).
    fn color(&self, me: Option<&str>) -> u32 {
        let base = match self.status {
            RoomStatus::Normal => NORMAL,
            RoomStatus::Novice => NOVICE,
            RoomStatus::Respawn => RESPAWN,
            RoomStatus::Closed => CLOSED,
            RoomStatus::OutOfBorders => OUT_OF_BORDERS,
        };
        if let Some((owner, level)) = &self.owner
            && let Some(color) = Owner::of(Some(owner), me).color()
        {
            return mix(base, color, OWNED + OWNED_PER_LEVEL * f32::from(*level));
        }
        if let Some(reserved) = &self.reserved
            && let Some(color) = Owner::of(Some(reserved), me).color()
        {
            return mix(base, color, RESERVED);
        }
        base
    }

    /// The terrain tint: colour and strength.
    fn tint(&self, me: Option<&str>) -> [u8; 4] {
        let owner = |user: &str| Owner::of(Some(user), me).color();
        if let Some(color) = self.owner.as_ref().and_then(|(id, _)| owner(id)) {
            return rgba(color, OWNED_TINT);
        }
        if let Some(color) = self.reserved.as_deref().and_then(owner) {
            return rgba(color, RESERVED_TINT);
        }
        match self.status {
            RoomStatus::Normal => [0; 4],
            RoomStatus::Novice => rgba(NOVICE, AREA_TINT),
            RoomStatus::Respawn => rgba(RESPAWN, AREA_TINT),
            RoomStatus::Closed | RoomStatus::OutOfBorders => rgba(0, CLOSED_TINT),
        }
    }
}

/// The overview cells of the rooms in the bounds (row-major from the north-west corner)
/// and the overview of every room seen.
#[derive(Debug)]
pub struct Cells {
    bounds: RoomRect,
    stats: HashMap<RoomCoord, CellStat>,
    instances: Vec<CellInstance>,
    /// Cells whose instance changed since the GPU took them.
    pub dirty: Dirty,
}

impl Cells {
    /// Cells of `bounds`, all unknown.
    pub fn new(bounds: RoomRect) -> Self {
        let mut cells = Self {
            bounds,
            stats: HashMap::new(),
            instances: Vec::new(),
            dirty: Dirty::default(),
        };
        cells.set_bounds(bounds, None);
        cells
    }

    /// Re-lays the cells over `bounds`.
    pub fn set_bounds(&mut self, bounds: RoomRect, me: Option<&str>) {
        self.bounds = bounds;
        self.instances.clear();
        self.instances.reserve(bounds.len());
        for room in bounds.iter() {
            let color = self.stats.get(&room).map_or(UNKNOWN, |stat| stat.color(me));
            self.instances.push(CellInstance {
                origin: room_origin(room).to_array(),
                color: rgba(color, 255),
            });
        }
        self.dirty.mark_all();
    }

    /// Recolours every cell (the signed-in user changed).
    pub fn recolor(&mut self, me: Option<&str>) {
        self.set_bounds(self.bounds, me);
    }

    /// The index of a room's cell.
    fn index(&self, room: RoomCoord) -> Option<usize> {
        if !self.bounds.contains(room) {
            return None;
        }
        let column = usize::try_from(room.x - self.bounds.min.x).ok()?;
        let row = usize::try_from(room.y - self.bounds.min.y).ok()?;
        let width = usize::try_from(self.bounds.width()).ok()?;
        Some(row * width + column)
    }

    /// Sets (`None`: forgets) a room's overview. Returns whether it changed.
    pub fn set_stat(&mut self, room: RoomCoord, stat: Option<&RoomStat>, me: Option<&str>) -> bool {
        let stat = stat.map(CellStat::of);
        if self.stats.get(&room) == stat.as_ref() {
            return false;
        }
        let color = stat.as_ref().map_or(UNKNOWN, |stat| stat.color(me));
        match stat {
            Some(stat) => self.stats.insert(room, stat),
            None => self.stats.remove(&room),
        };
        if let Some(index) = self.index(room)
            && let Some(instance) = self.instances.get_mut(index)
        {
            instance.color = rgba(color, 255);
            self.dirty.mark(u32::try_from(index).unwrap_or(u32::MAX));
        }
        true
    }

    /// The terrain tint of a room (transparent without an overview).
    pub fn tint(&self, room: RoomCoord, me: Option<&str>) -> [u8; 4] {
        self.stats.get(&room).map_or([0; 4], |stat| stat.tint(me))
    }

    /// The cell instances, row-major over the bounds.
    pub fn instances(&self) -> &[CellInstance] {
        &self.instances
    }
}
