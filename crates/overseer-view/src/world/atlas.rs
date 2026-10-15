//! The terrain of every room seen, packed in memory, and the slots of the GPU terrain atlas
//! holding the rooms nearest to the camera.
//!
//! The atlas is an `Rg8Unorm` texture of `per_row × per_row` slots of 50×50 texels (red:
//! wall, green: swamp) with a second mip level of 25×25 texels per slot (2×2 averages, so
//! slots never bleed into each other). A room gets a slot when its terrain arrives while it
//! is in the bounds, or later when it comes into view; when the atlas is full the slot of the
//! room farthest from the camera is reused (if that room is farther than the new one).

use std::collections::HashMap;

use overseer::core::api::{Terrain, Tile};
use overseer::map::RoomCoord;

use super::{Dirty, Focus};
use crate::camera::room_origin;
use crate::mesh::TerrainInstance;

/// Bytes of a slot's first mip level (50×50 texels, 2 bytes each).
pub const LEVEL0: usize = 50 * 50 * 2;
/// Bytes of a slot's second mip level (25×25 texels, 2 bytes each).
pub const LEVEL1: usize = 25 * 25 * 2;
/// Rooms (re)placed into the atlas per [`Atlas::refresh`].
const REFRESH_CAP: usize = 512;

/// A terrain at 2 bits per tile (0 plain, 1 wall, 2 swamp), indexed by `y * 50 + x`.
type Packed = Box<[u8; 625]>;

fn pack(terrain: &Terrain) -> Packed {
    let mut packed = Box::new([0; 625]);
    for y in 0..50_u8 {
        for x in 0..50_u8 {
            let code = match terrain.get(x, y) {
                Some(Tile::Wall) => 1,
                Some(Tile::Swamp) => 2,
                Some(Tile::Plain) | None => 0,
            };
            let index = usize::from(y) * 50 + usize::from(x);
            packed[index / 4] |= code << ((index % 4) * 2);
        }
    }
    packed
}

/// The tile code at `index` of a packed terrain.
fn code(packed: &[u8; 625], index: usize) -> u8 {
    (packed[index / 4] >> ((index % 4) * 2)) & 3
}

/// The two mip levels of a packed terrain.
fn texels(packed: &[u8; 625], level0: &mut [u8; LEVEL0], level1: &mut [u8; LEVEL1]) {
    for index in 0..2500 {
        let code = code(packed, index);
        level0[index * 2] = if code == 1 { 255 } else { 0 };
        level0[index * 2 + 1] = if code == 2 { 255 } else { 0 };
    }
    for y in 0..25 {
        for x in 0..25 {
            for channel in 0..2 {
                let at = |dx: usize, dy: usize| {
                    u16::from(level0[((y * 2 + dy) * 50 + x * 2 + dx) * 2 + channel])
                };
                let sum = at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1);
                level1[(y * 25 + x) * 2 + channel] = u8::try_from((sum + 2) / 4).unwrap_or(255);
            }
        }
    }
}

/// The instance of an empty slot (drawn as nothing).
const HIDDEN: TerrainInstance = TerrainInstance {
    origin: [0.0; 2],
    cell: [-1.0; 2],
    tint: [0; 4],
};

/// Terrain store and atlas slots.
#[derive(Debug)]
pub struct Atlas {
    per_row: u32,
    terrain: HashMap<RoomCoord, Packed>,
    slots: Vec<Option<RoomCoord>>,
    /// Free slots, popped from the end (lowest index first).
    free: Vec<u32>,
    by_room: HashMap<RoomCoord, u32>,
    instances: Vec<TerrainInstance>,
    /// Slots whose texels must be uploaded (may repeat).
    pub uploads: Vec<u32>,
    /// Slots whose instance changed since the GPU took them.
    pub dirty: Dirty,
    /// Scratch of [`Self::refresh`]: rooms in view without a slot, nearest first.
    missing: Vec<(f32, RoomCoord)>,
    /// Scratch of [`Self::refresh`]: occupied slots by distance.
    evictable: Vec<(f32, u32)>,
}

impl Atlas {
    /// An empty atlas of `per_row × per_row` slots.
    pub fn new(per_row: u32) -> Self {
        let per_row = per_row.max(1);
        let capacity = usize::try_from(per_row * per_row).unwrap_or(1);
        let mut dirty = Dirty::default();
        dirty.mark_all();
        Self {
            per_row,
            terrain: HashMap::new(),
            slots: vec![None; capacity],
            free: (0..per_row * per_row).rev().collect(),
            by_room: HashMap::new(),
            instances: vec![HIDDEN; capacity],
            uploads: Vec::new(),
            dirty,
            missing: Vec::new(),
            evictable: Vec::new(),
        }
    }

    /// Number of slots.
    pub const fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// The slot of a room.
    pub fn slot(&self, room: RoomCoord) -> Option<u32> {
        self.by_room.get(&room).copied()
    }

    /// Whether a room's terrain is known.
    pub fn has_terrain(&self, room: RoomCoord) -> bool {
        self.terrain.contains_key(&room)
    }

    /// The instances, one per slot.
    pub fn instances(&self) -> &[TerrainInstance] {
        &self.instances
    }

    /// Stores a room's terrain and gives it a slot if it is in the bounds and near enough.
    pub fn set_terrain(
        &mut self,
        room: RoomCoord,
        terrain: &Terrain,
        focus: &Focus,
        tint: [u8; 4],
    ) {
        let packed = pack(terrain);
        if self.terrain.get(&room) == Some(&packed) {
            return;
        }
        self.terrain.insert(room, packed);
        match self.slot(room) {
            Some(slot) => self.uploads.push(slot),
            None => {
                self.place(room, focus, tint, None);
            }
        }
    }

    /// Sets the tint of a room in the atlas.
    pub fn set_tint(&mut self, room: RoomCoord, tint: [u8; 4]) {
        if let Some(slot) = self.slot(room)
            && let Some(instance) = self.instances.get_mut(slot as usize)
            && instance.tint != tint
        {
            instance.tint = tint;
            self.dirty.mark(slot);
        }
    }

    /// The bounds changed: hides the rooms outside, shows those inside.
    pub fn set_bounds(&mut self, focus: &Focus, tint: impl Fn(RoomCoord) -> [u8; 4]) {
        for (slot, room) in self.slots.iter().enumerate() {
            self.instances[slot] = match room {
                Some(room) if focus.bounds.contains(*room) => {
                    instance(self.per_row, slot_index(slot), *room, tint(*room))
                }
                _ => HIDDEN,
            };
        }
        self.dirty.mark_all();
    }

    /// Gives a slot to a room (in the bounds): a free one, else that of the farthest room
    /// if it is farther than `room`. `evictable` (sorted by distance) speeds up repeated
    /// evictions.
    fn place(
        &mut self,
        room: RoomCoord,
        focus: &Focus,
        tint: [u8; 4],
        evictable: Option<&mut Vec<(f32, u32)>>,
    ) -> bool {
        if !focus.bounds.contains(room) {
            return false;
        }
        let distance = focus.distance(room);
        let slot = if let Some(slot) = self.free.pop() {
            slot
        } else {
            let farthest = match evictable {
                Some(evictable) => evictable.pop_if(|(far, _)| *far > distance),
                None => self
                    .slots
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, room)| Some((focus.distance((*room)?), slot_index(slot))))
                    .max_by(|a, b| a.0.total_cmp(&b.0))
                    .filter(|(far, _)| *far > distance),
            };
            let Some((_, slot)) = farthest else {
                return false;
            };
            if let Some(old) = self.slots[slot as usize].take() {
                self.by_room.remove(&old);
            }
            slot
        };
        self.slots[slot as usize] = Some(room);
        self.by_room.insert(room, slot);
        self.instances[slot as usize] = instance(self.per_row, slot, room, tint);
        self.uploads.push(slot);
        self.dirty.mark(slot);
        true
    }

    /// Places the rooms in view whose terrain is known but not in the atlas, nearest first
    /// and at most [`REFRESH_CAP`]. Returns whether rooms remain to be placed.
    pub fn refresh(&mut self, focus: &Focus, tint: impl Fn(RoomCoord) -> [u8; 4]) -> bool {
        let mut missing = std::mem::take(&mut self.missing);
        missing.clear();
        for room in focus.visible.iter() {
            if focus.bounds.contains(room)
                && self.terrain.contains_key(&room)
                && !self.by_room.contains_key(&room)
            {
                missing.push((focus.distance(room), room));
            }
        }
        missing.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut evictable = std::mem::take(&mut self.evictable);
        evictable.clear();
        if missing.len() > self.free.len() {
            evictable.extend(
                self.slots
                    .iter()
                    .enumerate()
                    .filter_map(|(slot, room)| Some((focus.distance((*room)?), slot_index(slot)))),
            );
            evictable.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        }
        let mut more = false;
        for (placed, &(_, room)) in missing.iter().enumerate() {
            if placed == REFRESH_CAP {
                more = true;
                break;
            }
            if !self.place(room, focus, tint(room), Some(&mut evictable)) {
                break;
            }
        }
        self.missing = missing;
        self.evictable = evictable;
        more
    }

    /// The texels of a slot's room; `false` for an empty slot.
    pub fn texels(&self, slot: u32, level0: &mut [u8; LEVEL0], level1: &mut [u8; LEVEL1]) -> bool {
        let packed = self
            .slots
            .get(slot as usize)
            .copied()
            .flatten()
            .and_then(|room| self.terrain.get(&room));
        packed.is_some_and(|packed| {
            texels(packed, level0, level1);
            true
        })
    }
}

/// A slot index as `u32` (atlases hold far fewer than `u32::MAX` slots).
fn slot_index(slot: usize) -> u32 {
    u32::try_from(slot).unwrap_or(u32::MAX)
}

/// The instance of `room` in `slot`.
#[expect(
    clippy::cast_precision_loss,
    reason = "slot columns and rows are far below 2^24"
)]
fn instance(per_row: u32, slot: u32, room: RoomCoord, tint: [u8; 4]) -> TerrainInstance {
    TerrainInstance {
        origin: room_origin(room).to_array(),
        cell: [(slot % per_row) as f32, (slot / per_row) as f32],
        tint,
    }
}

#[cfg(test)]
mod tests {
    use super::{LEVEL0, LEVEL1, code, pack, texels};
    use overseer::core::api::Terrain;

    #[test]
    fn terrain_packs_and_expands_into_both_levels() {
        let digits: String = (0..2500)
            .map(|index| match (index % 50, index / 50) {
                (0, _) => '1',
                (1, 0) => '3',
                (2 | 3, 0 | 1) => '2',
                _ => '0',
            })
            .collect();
        let terrain = Terrain::decode(&digits).unwrap();
        let packed = pack(&terrain);
        assert_eq!(code(&packed, 0), 1);
        assert_eq!(code(&packed, 1), 1, "wall and swamp bits: a wall");
        assert_eq!(code(&packed, 2), 2);
        assert_eq!(code(&packed, 4), 0);
        let (mut level0, mut level1) = ([0; LEVEL0], [0; LEVEL1]);
        texels(&packed, &mut level0, &mut level1);
        assert_eq!(&level0[0..8], &[255, 0, 255, 0, 0, 255, 0, 255]);
        // 2×2 averages: the corner has 3 walls of 4, the next block 4 swamps of 4.
        assert_eq!(&level1[0..4], &[191, 0, 0, 255]);
        assert_eq!(&level1[50..52], &[128, 0], "a wall column next to plains");
    }
}
