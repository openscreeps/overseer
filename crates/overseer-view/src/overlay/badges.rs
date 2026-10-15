//! The badge atlas: every user's badge given to `Viewer::set_badge`, resampled to a
//! [`BADGE`]-texel square with its mip levels (box-filtered down to 4 texels), in an
//! `Rgba8UnormSrgb` 2D array texture of premultiplied colour. A layer holds [`COLUMNS`] ×
//! [`ROWS`] cells (never square, at least two layers: see `world/terrain.rs` for why);
//! when every cell is taken the layers double (up to the device's limit), the texture is
//! created again and every badge uploaded again from the copy kept here. A user's badge
//! keeps its cell; giving it again rewrites the cell.

use std::collections::HashMap;

use crate::world::{Dirty, Upload};

/// Side of a badge cell at mip level 0, in texels.
pub const BADGE: u32 = 64;
/// Cells per row of a layer.
pub const COLUMNS: u32 = 8;
/// Rows of cells per layer (one more than columns: layers are never square).
pub const ROWS: u32 = 9;
/// Texels per row of a layer.
pub const LAYER_WIDTH: u32 = BADGE * COLUMNS;
/// Rows of texels per layer.
pub const LAYER_HEIGHT: u32 = BADGE * ROWS;
/// Cells per layer.
pub const CELLS_PER_LAYER: u32 = COLUMNS * ROWS;
/// Mip levels: a badge is 64, 32, 16, 8 and 4 texels wide.
pub const LEVELS: u32 = 5;
/// Fewest layers of the texture.
const MIN_LAYERS: u32 = 2;
/// Most layers ever used (18 432 badges).
const MAX_LAYERS: u32 = 256;

/// Bytes of a level of a cell.
const fn level_bytes(level: u32) -> usize {
    let side = (BADGE >> level) as usize;
    side * side * 4
}

/// Bytes of a cell with all its levels.
const CELL_BYTES: usize = {
    let mut bytes = 0;
    let mut level = 0;
    while level < LEVELS {
        bytes += level_bytes(level);
        level += 1;
    }
    bytes
};

/// The badges.
#[derive(Debug)]
pub struct Badges {
    max_layers: u32,
    layers: u32,
    by_user: HashMap<String, u32>,
    /// Each cell's texels, level 0 first (premultiplied sRGB RGBA, row major).
    cells: Vec<Vec<u8>>,
    /// Cells changed since the GPU took them.
    dirty: Dirty,
    /// Bumped whenever the texture must be created again (more layers).
    generation: u64,
}

impl Badges {
    /// No badges; the texture may have up to `max_layers` layers.
    pub fn new(max_layers: u32) -> Self {
        Self {
            max_layers: max_layers.clamp(MIN_LAYERS, MAX_LAYERS),
            layers: MIN_LAYERS,
            by_user: HashMap::new(),
            cells: Vec::new(),
            dirty: Dirty::default(),
            generation: 0,
        }
    }

    /// Layers of the texture.
    pub const fn layers(&self) -> u32 {
        self.layers
    }

    /// Changes whenever the texture must be created again with [`Self::layers`] layers
    /// (and every cell uploaded again, which [`Self::flush`] then does).
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Marks every cell for upload (a new texture).
    pub fn mark_all(&mut self) {
        self.dirty.mark_all();
    }

    /// Badges held.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Stores a user's badge: `rgba` is a `size` × `size` square of premultiplied RGBA
    /// bytes, row major. Returns whether the user had no badge before (rooms may now show
    /// it); `false` too when `rgba` does not match `size` or the atlas is full.
    pub fn set(&mut self, user: &str, rgba: &[u8], size: u32) -> bool {
        let width = size as usize;
        if width == 0 || width.checked_mul(width).and_then(|n| n.checked_mul(4)) != Some(rgba.len())
        {
            return false;
        }
        let texels = pyramid(rgba, width);
        if let Some(&cell) = self.by_user.get(user) {
            if let Some(stored) = self.cells.get_mut(cell as usize)
                && *stored != texels
            {
                *stored = texels;
                self.dirty.mark(cell);
            }
            return false;
        }
        let capacity = self.layers * CELLS_PER_LAYER;
        let next = u32::try_from(self.cells.len()).unwrap_or(u32::MAX);
        if next >= capacity {
            if self.layers >= self.max_layers {
                return false;
            }
            self.layers = (self.layers * 2).min(self.max_layers);
            self.generation += 1;
            self.dirty.mark_all();
        }
        self.by_user.insert(user.to_owned(), next);
        self.cells.push(texels);
        self.dirty.mark(next);
        true
    }

    /// The atlas rectangle of a user's badge: corner and size (`0..1`), and the layer.
    pub fn cell(&self, user: &str) -> Option<([f32; 4], u32)> {
        let cell = *self.by_user.get(user)?;
        let (column, row, layer) = place(cell);
        #[expect(clippy::cast_precision_loss, reason = "texel counts far below 2^24")]
        let rect = [
            (column * BADGE) as f32 / LAYER_WIDTH as f32,
            (row * BADGE) as f32 / LAYER_HEIGHT as f32,
            BADGE as f32 / LAYER_WIDTH as f32,
            BADGE as f32 / LAYER_HEIGHT as f32,
        ];
        Some((rect, layer))
    }

    /// Hands the levels of every cell changed since the last flush as uploads and clears
    /// the marks.
    pub fn flush(&mut self, mut upload: impl FnMut(Upload<'_>)) {
        let cells = &self.cells;
        let mut dirty = std::mem::take(&mut self.dirty);
        dirty.drain(cells.len(), |run| {
            for index in run {
                let (column, row, layer) = place(u32::try_from(index).unwrap_or(u32::MAX));
                let mut start = 0;
                for level in 0..LEVELS {
                    let side = BADGE >> level;
                    let bytes = level_bytes(level);
                    upload(Upload {
                        level,
                        layer,
                        origin: (column * side, row * side),
                        size: (side, side),
                        bytes_per_row: side * 4,
                        data: &cells[index][start..start + bytes],
                    });
                    start += bytes;
                }
            }
        });
        self.dirty = dirty;
    }
}

/// Column, row and layer of a cell.
const fn place(cell: u32) -> (u32, u32, u32) {
    let block = cell % CELLS_PER_LAYER;
    (block % COLUMNS, block / COLUMNS, cell / CELLS_PER_LAYER)
}

/// A `side`² RGBA square resampled to [`BADGE`]² texels (the area of each texel averaged,
/// or the nearest texel when enlarging), followed by its smaller levels.
fn pyramid(rgba: &[u8], side: usize) -> Vec<u8> {
    let badge = BADGE as usize;
    let mut texels = Vec::with_capacity(CELL_BYTES);
    // The source texels a texel of the badge covers along one axis.
    let span = |index: usize| {
        let start = index * side / badge;
        let end = ((index + 1) * side).div_ceil(badge).max(start + 1);
        start..end.min(side)
    };
    for y in 0..badge {
        for x in 0..badge {
            let mut sum = [0_u32; 4];
            let mut count = 0;
            for sy in span(y) {
                for sx in span(x) {
                    let at = (sy * side + sx) * 4;
                    for (total, &byte) in sum.iter_mut().zip(&rgba[at..at + 4]) {
                        *total += u32::from(byte);
                    }
                    count += 1;
                }
            }
            texels.extend(sum.map(|total| average(total, count)));
        }
    }
    let mut start = 0;
    for level in 1..LEVELS {
        let (above, side) = ((BADGE >> (level - 1)) as usize, (BADGE >> level) as usize);
        for y in 0..side {
            for x in 0..side {
                let mut sum = [0_u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let at = start + ((2 * y + dy) * above + 2 * x + dx) * 4;
                    for (total, &byte) in sum.iter_mut().zip(&texels[at..at + 4]) {
                        *total += u32::from(byte);
                    }
                }
                texels.extend(sum.map(|total| average(total, 4)));
            }
        }
        start += above * above * 4;
    }
    texels
}

/// The rounded mean of `count` bytes summing to `total`.
fn average(total: u32, count: u32) -> u8 {
    u8::try_from((total + count / 2) / count.max(1)).unwrap_or(u8::MAX)
}
