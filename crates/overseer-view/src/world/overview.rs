//! The overview layer: the terrain of the whole scope as one low-resolution picture, drawn
//! zoomed out (where the full-resolution terrain would only flicker) and cross-faded into
//! it while zooming in.
//!
//! An `Rgba8Unorm` texture covers the bounds, [`Overview::texels_per_room`] texels per room
//! side (10 when the bounds fit the device's texture size, else 5, 2 or 1). A texel is the
//! share of walls (red), swamps (green) and plains (blue) among the tiles it covers; a room
//! without terrain is all zero, which the shader draws as the placeholder. Every mip level
//! down to 1×1 is kept (2×2 averages, as the GPU would make them) so that the full shard
//! filters smoothly at any zoom. A room whose terrain arrives rewrites its own texels and
//! the texels of every level that cover them; [`Overview::flush`] hands those as rectangles
//! (one per room row and level).

use overseer::map::{RoomCoord, RoomRect};

use super::terrain::TerrainStore;
use super::{Dirty, Upload};

/// Bytes per texel.
const TEXEL: usize = 4;
/// Texels per room side, best first: divisors of 50 so that texels cover whole tiles.
const RESOLUTIONS: [u32; 4] = [10, 5, 2, 1];

/// One mip level: size and texels (row-major RGBA).
#[derive(Debug)]
struct Level {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Level {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * TEXEL],
        }
    }

    const fn offset(&self, x: u32, y: u32) -> usize {
        (y as usize * self.width as usize + x as usize) * TEXEL
    }
}

/// A rectangle of texels: `x0..x1` × `y0..y1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl Rect {
    /// The rectangle of the next mip level covering this one (within `width × height`).
    fn halved(self, width: u32, height: u32) -> Self {
        Self {
            x0: (self.x0 / 2).min(width - 1),
            y0: (self.y0 / 2).min(height - 1),
            x1: self.x1.div_ceil(2).clamp(1, width),
            y1: self.y1.div_ceil(2).clamp(1, height),
        }
    }
}

/// The overview texture of the bounds and its mip chain.
#[derive(Debug)]
pub struct Overview {
    bounds: RoomRect,
    texels_per_room: u32,
    levels: Vec<Level>,
    /// Rooms (row-major over the bounds) whose texels the GPU has not taken yet.
    dirty: Dirty,
    /// Scratch of [`Self::flush`]: changed rectangles of the current level.
    rects: Vec<Rect>,
    /// Bumped whenever the texture must be recreated (new bounds).
    generation: u64,
}

impl Overview {
    /// The overview of `bounds` for textures of at most `max_side` texels, rooms without
    /// terrain until [`Self::set_room`].
    pub fn new(bounds: RoomRect, max_side: u32) -> Self {
        let mut overview = Self {
            bounds,
            texels_per_room: 1,
            levels: Vec::new(),
            dirty: Dirty::default(),
            rects: Vec::new(),
            generation: 0,
        };
        overview.lay_out(bounds, max_side);
        overview
    }

    /// Lays the overview over new bounds and draws every room with terrain.
    pub fn set_bounds(&mut self, bounds: RoomRect, max_side: u32, store: &TerrainStore) {
        self.lay_out(bounds, max_side);
        for room in bounds.iter() {
            if let Some(slot) = store.slot(room) {
                self.draw_room(room, slot, store);
            }
        }
    }

    fn lay_out(&mut self, bounds: RoomRect, max_side: u32) {
        let (width, height) = (bounds.width(), bounds.height());
        self.texels_per_room = RESOLUTIONS
            .into_iter()
            .find(|&texels| width * texels <= max_side && height * texels <= max_side)
            .unwrap_or(1);
        self.bounds = bounds;
        let (mut width, mut height) = (
            (width * self.texels_per_room).clamp(1, max_side.max(1)),
            (height * self.texels_per_room).clamp(1, max_side.max(1)),
        );
        self.levels.clear();
        loop {
            self.levels.push(Level::new(width, height));
            if width == 1 && height == 1 {
                break;
            }
            (width, height) = ((width / 2).max(1), (height / 2).max(1));
        }
        self.generation += 1;
        self.dirty.mark_all();
    }

    /// Marks every room for upload (a new texture).
    pub fn mark_all(&mut self) {
        self.dirty.mark_all();
    }

    /// The texel `(x, y)` of mip level `level`.
    #[cfg(test)]
    pub fn texel(&self, level: usize, x: u32, y: u32) -> [u8; 4] {
        let level = &self.levels[level];
        let at = level.offset(x, y);
        [0, 1, 2, 3].map(|channel| level.data[at + channel])
    }

    /// Texels per room side.
    #[cfg(test)]
    pub const fn texels_per_room(&self) -> u32 {
        self.texels_per_room
    }

    /// The size of mip level 0 in texels.
    pub fn size(&self) -> (u32, u32) {
        self.levels
            .first()
            .map_or((1, 1), |level| (level.width, level.height))
    }

    /// Mip levels.
    pub fn level_count(&self) -> u32 {
        u32::try_from(self.levels.len()).unwrap_or(1)
    }

    /// Changes whenever the texture must be recreated (with [`Self::size`] and
    /// [`Self::level_count`]); every texel is then uploaded again by [`Self::flush`].
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The world rectangle (X, Z in tiles) the texture covers: the bounds' north-west
    /// corner and the extent of the texture.
    #[expect(
        clippy::cast_precision_loss,
        reason = "room coordinates and texture sizes are far below 2^24"
    )]
    pub fn placement(&self) -> [f32; 4] {
        let (width, height) = self.size();
        let tiles = 50.0 / self.texels_per_room as f32;
        [
            self.bounds.min.x as f32 * 50.0,
            self.bounds.min.y as f32 * 50.0,
            width as f32 * tiles,
            height as f32 * tiles,
        ]
    }

    /// The index of a room in the bounds.
    fn index(&self, room: RoomCoord) -> Option<u32> {
        if !self.bounds.contains(room) {
            return None;
        }
        let column = u32::try_from(room.x - self.bounds.min.x).ok()?;
        let row = u32::try_from(room.y - self.bounds.min.y).ok()?;
        Some(row * self.bounds.width() + column)
    }

    /// Draws a room whose terrain is in `slot` of `store` (rooms outside the bounds are
    /// ignored).
    pub fn set_room(&mut self, room: RoomCoord, slot: u32, store: &TerrainStore) {
        if self.index(room).is_some() {
            self.draw_room(room, slot, store);
        }
    }

    /// Writes a room's level-0 texels and marks it.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "room offsets inside the bounds are small and positive"
    )]
    fn draw_room(&mut self, room: RoomCoord, slot: u32, store: &TerrainStore) {
        let Some(index) = self.index(room) else {
            return;
        };
        let texels = self.texels_per_room as usize;
        let tiles = 50 / texels;
        let count = u32::try_from(tiles * tiles).unwrap_or(1);
        // Tile counts (wall, swamp) per texel of the room, from its packed bytes in order.
        let mut counts = [[0_u32; 2]; 100];
        let packed = store.packed(slot);
        for (byte_index, &byte) in packed.iter().enumerate() {
            for within in 0..4 {
                let tile = byte_index * 4 + within;
                let code = (byte >> (within * 2)) & 3;
                if code == 1 || code == 2 {
                    let texel = (tile / 50 / tiles) * texels + (tile % 50) / tiles;
                    counts[texel][usize::from(code - 1)] += 1;
                }
            }
        }
        let level = &mut self.levels[0];
        let origin = (
            (room.x - self.bounds.min.x) as u32 * self.texels_per_room,
            (room.y - self.bounds.min.y) as u32 * self.texels_per_room,
        );
        let share = |part: u32| u8::try_from((part * 255 + count / 2) / count).unwrap_or(u8::MAX);
        for (texel, [walls, swamps]) in counts.iter().take(texels * texels).enumerate() {
            let x = origin.0 + (texel % texels) as u32;
            let y = origin.1 + (texel / texels) as u32;
            if x >= level.width || y >= level.height {
                continue;
            }
            let (wall, swamp) = (share(*walls), share(*swamps));
            let plain = 255_u8.saturating_sub(wall).saturating_sub(swamp);
            let at = level.offset(x, y);
            level.data[at..at + TEXEL].copy_from_slice(&[wall, swamp, plain, 255]);
        }
        self.dirty.mark(index);
    }

    /// Recomputes the mip levels over the rooms that changed and hands the changed texels of
    /// every level as rectangles, then clears the marks.
    pub fn flush(&mut self, mut upload: impl FnMut(Upload<'_>)) {
        let mut rects = std::mem::take(&mut self.rects);
        rects.clear();
        let width = self.bounds.width();
        let texels = self.texels_per_room;
        let mut dirty = std::mem::take(&mut self.dirty);
        let (level_width, level_height) = self.size();
        dirty.drain(self.bounds.len(), |run| {
            let (start, end) = (index_u32(run.start), index_u32(run.end));
            // Split the run into room rows; merge with the previous rectangle of the same row.
            let mut room = start;
            while room < end {
                let row = room / width;
                let stop = end.min((row + 1) * width);
                let rect = Rect {
                    x0: (room % width) * texels,
                    y0: row * texels,
                    x1: ((stop - 1) % width + 1) * texels,
                    y1: (row + 1) * texels,
                };
                let rect = Rect {
                    x0: rect.x0.min(level_width),
                    y0: rect.y0.min(level_height),
                    x1: rect.x1.min(level_width),
                    y1: rect.y1.min(level_height),
                };
                match rects.last_mut() {
                    Some(last) if last.y0 == rect.y0 => {
                        last.x0 = last.x0.min(rect.x0);
                        last.x1 = last.x1.max(rect.x1);
                    }
                    _ => rects.push(rect),
                }
                room = stop;
            }
        });
        self.dirty = dirty;
        rects.retain(|rect| rect.x0 < rect.x1 && rect.y0 < rect.y1);
        for level in 0..self.levels.len() {
            if level > 0 {
                let (source, target) = self.levels.split_at_mut(level);
                let (source, target) = (&source[level - 1], &mut target[0]);
                for rect in &mut rects {
                    *rect = rect.halved(target.width, target.height);
                    downsample(source, target, *rect);
                }
            }
            merge(&mut rects);
            let data = &self.levels[level];
            for rect in &rects {
                upload(level_upload(data, level, *rect));
            }
        }
        self.rects = rects;
    }
}

/// Averages 2×2 texels of `source` into `rect` of `target` (edge texels of odd sizes clamp).
fn downsample(source: &Level, target: &mut Level, rect: Rect) {
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let (sx, sy) = (x * 2, y * 2);
            let xs = [sx.min(source.width - 1), (sx + 1).min(source.width - 1)];
            let ys = [sy.min(source.height - 1), (sy + 1).min(source.height - 1)];
            let at = target.offset(x, y);
            for channel in 0..TEXEL {
                let mut sum = 0_u16;
                for &y in &ys {
                    for &x in &xs {
                        sum += u16::from(source.data[source.offset(x, y) + channel]);
                    }
                }
                target.data[at + channel] = u8::try_from((sum + 2) / 4).unwrap_or(u8::MAX);
            }
        }
    }
}

/// Merges rectangles (in row order) whose rows overlap or touch with the same columns.
fn merge(rects: &mut Vec<Rect>) {
    let mut merged = 0;
    for index in 0..rects.len() {
        let rect = rects[index];
        if merged > 0 {
            let last = &mut rects[merged - 1];
            let overlaps = rect.y0 < last.y1;
            let stacks = rect.y0 == last.y1 && rect.x0 == last.x0 && rect.x1 == last.x1;
            if overlaps || stacks {
                last.x0 = last.x0.min(rect.x0);
                last.x1 = last.x1.max(rect.x1);
                last.y1 = last.y1.max(rect.y1);
                continue;
            }
        }
        rects[merged] = rect;
        merged += 1;
    }
    rects.truncate(merged);
}

/// The texels of `rect` in `level` as an upload.
fn level_upload(data: &Level, level: usize, rect: Rect) -> Upload<'_> {
    let start = data.offset(rect.x0, rect.y0);
    let end = data.offset(rect.x1 - 1, rect.y1 - 1) + TEXEL;
    Upload {
        level: u32::try_from(level).unwrap_or(0),
        layer: 0,
        origin: (rect.x0, rect.y0),
        size: (rect.x1 - rect.x0, rect.y1 - rect.y0),
        bytes_per_row: data.width * u32::try_from(TEXEL).unwrap_or(4),
        data: &data.data[start..end],
    }
}

/// A room index as `u32` (bounds hold far fewer than `u32::MAX` rooms).
fn index_u32(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}
