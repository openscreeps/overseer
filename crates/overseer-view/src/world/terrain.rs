//! The terrain of every room seen, resident for the viewer's life in the layout of the GPU
//! texture that draws it.
//!
//! Terrain stays packed as [`Terrain::packed`] gives it (2 bits per tile, 625 bytes per room)
//! and the shader decodes it. A room takes one *slot*: a block of 25×25 texels of an `R8Uint`
//! 2D array texture whose layers hold [`SLOT_COLUMNS`] × [`SLOT_ROWS`] slots
//! ([`LAYER_WIDTH`] × [`LAYER_HEIGHT`] texels). Byte `b` of a room's packed terrain is the
//! texel `(b % 25, b / 25)` of its block, so tile `(x, y)` (index `i = y × 50 + x`) is bits
//! `2 × (i % 4)` of the texel of byte `i / 4` ([`TerrainStore::tile`] reads it the way
//! `tile_code` in `world.wgsl` does).
//!
//! Slots are given in arrival order and never taken back. The store keeps a copy of the
//! texture (so that a texture grown by layers is filled again without asking the app) and
//! records the slots whose bytes changed; [`TerrainStore::flush`] hands them as rectangles.
//!
//! Layers are never square and at least two: wgpu's WebGL2 backend would otherwise create a
//! plain 2D texture (one layer) or a cube map (square layers, a multiple of 6) instead of an
//! array.

use std::collections::{HashMap, HashSet};

use overseer::map::RoomCoord;
use overseer::model::world::Terrain;

use super::{Dirty, Upload};

/// Texels per side of a slot: 25 × 25 = 625 bytes, one packed terrain.
pub const SLOT_SIDE: u32 = 25;
/// Slots per row of a layer.
pub const SLOT_COLUMNS: u32 = 40;
/// Rows of slots per layer (one more than columns: layers are never square).
pub const SLOT_ROWS: u32 = 41;
/// Texels per row of a layer.
pub const LAYER_WIDTH: u32 = SLOT_SIDE * SLOT_COLUMNS;
/// Texel rows per layer.
pub const LAYER_HEIGHT: u32 = SLOT_SIDE * SLOT_ROWS;
/// Slots per layer.
pub const SLOTS_PER_LAYER: u32 = SLOT_COLUMNS * SLOT_ROWS;
/// Fewest layers of the texture.
const MIN_LAYERS: u32 = 2;
/// Bytes per layer.
const LAYER_BYTES: usize = (LAYER_WIDTH * LAYER_HEIGHT) as usize;
/// Bytes per texel row.
const ROW_BYTES: usize = LAYER_WIDTH as usize;
/// Packed bytes per slot row (one row of 25 texels).
const SLOT_ROW_BYTES: usize = SLOT_SIDE as usize;

/// What the room instance says about a room's terrain: its slot, or one of these.
pub mod state {
    /// No terrain yet: drawn as a neutral placeholder.
    pub const PENDING: u32 = u32::MAX;
    /// The server has no terrain for the room: drawn as outside the world.
    pub const ABSENT: u32 = u32::MAX - 1;
}

/// Every room's packed terrain, in texture layout.
#[derive(Debug)]
pub struct TerrainStore {
    /// Most layers the device allows.
    max_layers: u32,
    /// Layers of the texture (and of `texels`).
    layers: u32,
    /// The texture's bytes, layer-major.
    texels: Vec<u8>,
    by_room: HashMap<RoomCoord, u32>,
    absent: HashSet<RoomCoord>,
    /// Slots given (the next slot).
    used: u32,
    /// Slots whose bytes the GPU has not taken yet.
    dirty: Dirty,
    /// Bumped whenever the texture must be recreated (more layers).
    generation: u64,
}

impl TerrainStore {
    /// An empty store whose texture may have up to `max_layers` layers.
    pub fn new(max_layers: u32) -> Self {
        let max_layers = max_layers.max(MIN_LAYERS);
        Self {
            max_layers,
            layers: MIN_LAYERS,
            texels: vec![0; LAYER_BYTES * MIN_LAYERS as usize],
            by_room: HashMap::new(),
            absent: HashSet::new(),
            used: 0,
            dirty: Dirty::default(),
            generation: 0,
        }
    }

    /// Layers of the texture.
    pub const fn layers(&self) -> u32 {
        self.layers
    }

    /// Changes whenever the texture must be recreated with [`Self::layers`] layers (and
    /// every slot uploaded again, which [`Self::flush`] then does).
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Marks every slot for upload (a new texture).
    pub fn mark_all(&mut self) {
        self.dirty.mark_all();
    }

    /// Rooms with terrain.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.by_room.len()
    }

    /// The slot of a room with terrain.
    pub fn slot(&self, room: RoomCoord) -> Option<u32> {
        self.by_room.get(&room).copied()
    }

    /// Whether a room's terrain is known.
    pub fn has_terrain(&self, room: RoomCoord) -> bool {
        self.by_room.contains_key(&room)
    }

    /// The room instance's terrain state: the slot, [`state::ABSENT`] or
    /// [`state::PENDING`].
    pub fn state(&self, room: RoomCoord) -> u32 {
        match self.slot(room) {
            Some(slot) => slot,
            None if self.absent.contains(&room) => state::ABSENT,
            None => state::PENDING,
        }
    }

    /// Grows the texture (if allowed) so that `more` further rooms fit without growing
    /// again.
    pub fn reserve(&mut self, more: usize) {
        let wanted = usize::try_from(self.used)
            .unwrap_or(usize::MAX)
            .saturating_add(more);
        let layers = wanted.div_ceil(SLOTS_PER_LAYER as usize);
        let layers = u32::try_from(layers)
            .unwrap_or(u32::MAX)
            .clamp(MIN_LAYERS, self.max_layers);
        if layers > self.layers {
            self.layers = layers;
            self.texels.resize(LAYER_BYTES * layers as usize, 0);
            self.generation += 1;
            self.dirty.mark_all();
        }
    }

    /// Stores a room's terrain. Returns its slot when it is new or changed, `None` when it
    /// is the same as before or the texture is full.
    pub fn set(&mut self, room: RoomCoord, terrain: &Terrain) -> Option<u32> {
        let packed = terrain.packed();
        let slot = if let Some(slot) = self.slot(room) {
            if self.block_equals(slot, packed) {
                return None;
            }
            slot
        } else {
            if self.used == self.layers * SLOTS_PER_LAYER {
                // Full: twice as many layers (fewer regrowths while a shard streams in).
                self.reserve(usize::try_from(self.used).unwrap_or(usize::MAX));
            }
            if self.used == self.layers * SLOTS_PER_LAYER {
                return None;
            }
            let slot = self.used;
            self.used += 1;
            self.by_room.insert(room, slot);
            self.absent.remove(&room);
            slot
        };
        for (row, bytes) in packed.as_chunks::<SLOT_ROW_BYTES>().0.iter().enumerate() {
            let start = block_start(slot) + row * ROW_BYTES;
            self.texels[start..start + SLOT_ROW_BYTES].copy_from_slice(bytes);
        }
        self.dirty.mark(slot);
        Some(slot)
    }

    /// Records that the server has no terrain for a room. Returns whether that changes its
    /// state (a room with terrain keeps it).
    pub fn mark_absent(&mut self, room: RoomCoord) -> bool {
        !self.has_terrain(room) && self.absent.insert(room)
    }

    fn block_equals(&self, slot: u32, packed: &[u8; Terrain::PACKED_LEN]) -> bool {
        packed
            .as_chunks::<SLOT_ROW_BYTES>()
            .0
            .iter()
            .enumerate()
            .all(|(row, bytes)| {
                let start = block_start(slot) + row * ROW_BYTES;
                self.texels[start..start + SLOT_ROW_BYTES] == *bytes
            })
    }

    /// The tile code (0 plain, 1 wall, 2 swamp) of tile `(x, y)` (`0..50`) in `slot`, read
    /// from the texture layout with the arithmetic of `tile_code` in `world.wgsl` (the tests
    /// hold both to [`Terrain::get`]).
    #[cfg(test)]
    pub fn tile(&self, slot: u32, x: u32, y: u32) -> u8 {
        let index = y * 50 + x;
        let byte = index >> 2;
        let block = slot % SLOTS_PER_LAYER;
        let layer = slot / SLOTS_PER_LAYER;
        let texel_x = (block % SLOT_COLUMNS) * SLOT_SIDE + byte % SLOT_SIDE;
        let texel_y = (block / SLOT_COLUMNS) * SLOT_SIDE + byte / SLOT_SIDE;
        let at = layer as usize * LAYER_BYTES + texel_y as usize * ROW_BYTES + texel_x as usize;
        (self.texels[at] >> ((index & 3) * 2)) & 3
    }

    /// The packed terrain in `slot`, as [`Terrain::packed`] gave it.
    pub fn packed(&self, slot: u32) -> [u8; Terrain::PACKED_LEN] {
        let mut packed = [0; Terrain::PACKED_LEN];
        for (row, bytes) in packed
            .as_chunks_mut::<SLOT_ROW_BYTES>()
            .0
            .iter_mut()
            .enumerate()
        {
            let start = block_start(slot) + row * ROW_BYTES;
            bytes.copy_from_slice(&self.texels[start..start + SLOT_ROW_BYTES]);
        }
        packed
    }

    /// Hands the bytes of the slots that changed since the last flush as texture
    /// rectangles (one per run of neighbouring slots in a slot row; whole slot rows of a
    /// layer merged) and clears the marks.
    pub fn flush(&mut self, mut upload: impl FnMut(Upload<'_>)) {
        let mut pending: Option<Region> = None;
        let texels = &self.texels;
        let mut emit = |region: Region| upload(region.upload(texels));
        let mut dirty = std::mem::take(&mut self.dirty);
        dirty.drain(self.used as usize, |run| {
            let (mut start, end) = (slot_index(run.start), slot_index(run.end));
            while start < end {
                let row_end = (start / SLOT_COLUMNS + 1) * SLOT_COLUMNS;
                let stop = end.min(row_end);
                let region = Region::of(start, stop);
                pending = match pending.take() {
                    Some(previous) => match previous.merge(region) {
                        Ok(merged) => Some(merged),
                        Err(previous) => {
                            emit(previous);
                            Some(region)
                        }
                    },
                    None => Some(region),
                };
                start = stop;
            }
        });
        if let Some(region) = pending {
            emit(region);
        }
        self.dirty = dirty;
    }
}

/// A rectangle of slots in one layer: slot columns and rows (end exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Region {
    layer: u32,
    columns: (u32, u32),
    rows: (u32, u32),
}

impl Region {
    /// The slots `start..end` of one slot row.
    const fn of(start: u32, end: u32) -> Self {
        let layer = start / SLOTS_PER_LAYER;
        let (first, last) = (start % SLOTS_PER_LAYER, (end - 1) % SLOTS_PER_LAYER);
        let row = first / SLOT_COLUMNS;
        Self {
            layer,
            columns: (first % SLOT_COLUMNS, last % SLOT_COLUMNS + 1),
            rows: (row, row + 1),
        }
    }

    /// `self` and the next region as one, when both are whole slot rows of a layer, one
    /// under the other; else `self` back.
    const fn merge(self, next: Self) -> Result<Self, Self> {
        if self.is_whole()
            && next.is_whole()
            && self.layer == next.layer
            && self.rows.1 == next.rows.0
        {
            Ok(Self {
                rows: (self.rows.0, next.rows.1),
                ..self
            })
        } else {
            Err(self)
        }
    }

    /// Whether the region spans whole slot rows.
    const fn is_whole(&self) -> bool {
        self.columns.0 == 0 && self.columns.1 == SLOT_COLUMNS
    }

    /// The texels of the region, as an upload.
    fn upload(self, texels: &[u8]) -> Upload<'_> {
        let (x, y) = (self.columns.0 * SLOT_SIDE, self.rows.0 * SLOT_SIDE);
        let width = (self.columns.1 - self.columns.0) * SLOT_SIDE;
        let height = (self.rows.1 - self.rows.0) * SLOT_SIDE;
        let start = self.layer as usize * LAYER_BYTES + y as usize * ROW_BYTES + x as usize;
        let len = (height as usize - 1) * ROW_BYTES + width as usize;
        Upload {
            level: 0,
            layer: self.layer,
            origin: (x, y),
            size: (width, height),
            bytes_per_row: LAYER_WIDTH,
            data: &texels[start..start + len],
        }
    }
}

/// Byte offset of a slot's first texel.
const fn block_start(slot: u32) -> usize {
    let block = slot % SLOTS_PER_LAYER;
    let layer = slot / SLOTS_PER_LAYER;
    layer as usize * LAYER_BYTES
        + (block / SLOT_COLUMNS * SLOT_SIDE) as usize * ROW_BYTES
        + (block % SLOT_COLUMNS * SLOT_SIDE) as usize
}

/// A slot index as `u32` (stores hold far fewer than `u32::MAX` slots).
fn slot_index(slot: usize) -> u32 {
    u32::try_from(slot).unwrap_or(u32::MAX)
}
