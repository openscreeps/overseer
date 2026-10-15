//! Text of room visuals: a glyph atlas of signed distance fields, rasterized at runtime by
//! the browser's canvas text API (`OffscreenCanvas` `fillText`, wasm) and drawn with a
//! threshold on the distance, so that text stays sharp at every zoom.
//!
//! Each glyph is drawn once, white on transparent, at [`FONT_PX`] pixels per em into a
//! [`CELL`]-pixel cell (pen at [`PEN`], baseline at [`BASELINE`]); its coverage becomes a
//! distance field (the `TinySDF` approach: an exact Euclidean distance transform of the
//! inside and of the outside, seeded with sub-pixel distances from the coverage), stored
//! as `0.5 - distance / (2 × SPREAD)` (128 on the outline, distances up to [`SPREAD`]
//! pixels either way). The atlas is one `R8Unorm` texture of [`ATLAS_SIDE`]² texels: 1024
//! glyphs, uploaded cell by cell as they first appear. Glyphs beyond that, and every glyph
//! where no rasterizer exists (native builds), are not drawn but keep their place.
//!
//! The font is the browser's `sans-serif`; families and weights in a visual's `font` are not
//! followed (sizes are).

use std::collections::HashMap;

use crate::mesh::GlyphInstance;

/// Side of the atlas texture, in texels.
pub const ATLAS_SIDE: u32 = 2048;
/// Side of a glyph cell, in texels.
pub const CELL: u32 = 64;
/// Font size of the rasterized glyphs: pixels per em.
pub const FONT_PX: f32 = 40.0;
/// Pen position (left of the glyph's origin) in a cell, in pixels.
pub const PEN: f32 = 12.0;
/// Baseline in a cell, from its top, in pixels.
pub const BASELINE: f32 = 46.0;
/// Distance range of the field either side of the outline, in pixels.
pub const SPREAD: f32 = 10.0;
/// Advance of a glyph the atlas cannot draw, in ems.
const FALLBACK_ADVANCE: f32 = 0.55;

/// Draws one glyph's coverage.
pub trait Rasterizer: std::fmt::Debug {
    /// Draws `glyph` white on transparent into `cell` ([`CELL`]² coverage bytes, row
    /// major; it arrives cleared) with the pen at ([`PEN`], [`BASELINE`]) and [`FONT_PX`]
    /// pixels per em; returns its advance in pixels (`None`: cannot draw it).
    fn rasterize(&mut self, glyph: char, cell: &mut [u8]) -> Option<f32>;
}

/// A glyph of the atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    /// Cell index (`None`: measured, but not drawn).
    cell: Option<u32>,
    /// Advance, in ems.
    advance: f32,
}

/// Text alignment around the anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// The anchor is the left end.
    Left,
    /// The anchor is the middle.
    Center,
    /// The anchor is the right end.
    Right,
}

/// How a run of text is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// Font size: tiles per em.
    pub size: f32,
    /// Alignment around the anchor.
    pub align: Align,
    /// Colour (sRGB, alpha = opacity).
    pub color: [u8; 4],
    /// Outline colour (sRGB, alpha = opacity; alpha 0: none).
    pub stroke: [u8; 4],
    /// Outline width (tiles; the outline spreads half of it outside the glyph).
    pub stroke_width: f32,
}

/// The glyph atlas: cells in use and cells waiting for upload.
#[derive(Debug)]
pub struct Atlas {
    rasterizer: Option<Box<dyn Rasterizer>>,
    glyphs: HashMap<char, Entry>,
    next: u32,
    /// Cells drawn since the last [`Self::flush`]: index and distance field.
    pending: Vec<(u32, Vec<u8>)>,
    /// Scratch of the distance transform.
    scratch: Scratch,
}

impl Default for Atlas {
    fn default() -> Self {
        Self::new(None)
    }
}

/// Glyph cells per atlas row.
const COLUMNS: u32 = ATLAS_SIDE / CELL;

impl Atlas {
    /// An empty atlas drawing with `rasterizer` (`None`: no text is drawn).
    pub fn new(rasterizer: Option<Box<dyn Rasterizer>>) -> Self {
        Self {
            rasterizer,
            glyphs: HashMap::new(),
            next: 0,
            pending: Vec::new(),
            scratch: Scratch::default(),
        }
    }

    /// The browser's canvas rasterizer (wasm), or none.
    pub fn browser() -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            Self::new(web::CanvasRasterizer::new().map(|r| Box::new(r) as Box<dyn Rasterizer>))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::new(None)
        }
    }

    /// The glyph of `c`, rasterized on first use.
    fn entry(&mut self, c: char) -> Entry {
        if let Some(entry) = self.glyphs.get(&c) {
            return *entry;
        }
        let mut entry = Entry {
            cell: None,
            advance: FALLBACK_ADVANCE,
        };
        if c.is_whitespace() {
            entry.advance = if c == ' ' { 0.28 } else { FALLBACK_ADVANCE };
        } else if let Some(rasterizer) = &mut self.rasterizer
            && self.next < COLUMNS * COLUMNS
        {
            let side = CELL as usize;
            let mut coverage = vec![0; side * side];
            if let Some(advance) = rasterizer.rasterize(c, &mut coverage) {
                entry.advance = advance / FONT_PX;
                let field = self.scratch.field(&coverage, side);
                entry.cell = Some(self.next);
                self.pending.push((self.next, field));
                self.next += 1;
            }
        }
        self.glyphs.insert(c, entry);
        entry
    }

    /// Width of `text` in ems.
    pub fn measure(&mut self, text: &str) -> f32 {
        text.chars().map(|c| self.entry(c).advance).sum()
    }

    /// Lays out one line of `text` with its anchor (the baseline's alignment point) at
    /// `anchor` (room-local tiles); appends its glyphs to `out` and returns the line's left
    /// and right ends.
    pub fn layout(
        &mut self,
        text: &str,
        anchor: (f32, f32),
        style: &TextStyle,
        out: &mut Vec<GlyphInstance>,
    ) -> (f32, f32) {
        let width = self.measure(text) * style.size;
        let left = match style.align {
            Align::Left => anchor.0,
            Align::Center => anchor.0 - width / 2.0,
            Align::Right => anchor.0 - width,
        };
        let scale = style.size / FONT_PX;
        #[expect(clippy::cast_precision_loss, reason = "a cell is 64 texels")]
        let side = CELL as f32 * scale;
        // The outline in distance-field units: half its width outside the glyph.
        let stroke_width = if style.stroke[3] > 0 {
            (style.stroke_width / 2.0 / scale / (2.0 * SPREAD)).clamp(0.0, 0.5)
        } else {
            0.0
        };
        let mut pen = left;
        for c in text.chars() {
            let entry = self.entry(c);
            if let Some(cell) = entry.cell {
                out.push(GlyphInstance {
                    rect: [pen - PEN * scale, anchor.1 - BASELINE * scale, side, side],
                    uv: cell_uv(cell),
                    color: style.color,
                    stroke: style.stroke,
                    stroke_width,
                });
            }
            pen += entry.advance * style.size;
        }
        (left, left + width)
    }

    /// Hands the cells drawn since the last flush to `upload` (texel origin, field bytes of
    /// [`CELL`]² texels).
    pub fn flush(&mut self, mut upload: impl FnMut((u32, u32), &[u8])) {
        for (cell, field) in self.pending.drain(..) {
            upload(((cell % COLUMNS) * CELL, (cell / COLUMNS) * CELL), &field);
        }
    }

    /// Whether cells wait for upload.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// The atlas rectangle of a cell: corner and size in `0..1`.
#[expect(clippy::cast_precision_loss, reason = "texel counts far below 2^24")]
fn cell_uv(cell: u32) -> [f32; 4] {
    let side = ATLAS_SIDE as f32;
    [
        ((cell % COLUMNS) * CELL) as f32 / side,
        ((cell / COLUMNS) * CELL) as f32 / side,
        CELL as f32 / side,
        CELL as f32 / side,
    ]
}

/// Buffers of the distance transform, reused from glyph to glyph.
#[derive(Debug, Default)]
struct Scratch {
    outer: Vec<f32>,
    inner: Vec<f32>,
    f: Vec<f32>,
    d: Vec<f32>,
    v: Vec<usize>,
    z: Vec<f32>,
}

/// "Infinite" squared distance.
const FAR: f32 = 1e20;

impl Scratch {
    /// The distance field (bytes, see the module documentation) of `side`² coverage bytes.
    fn field(&mut self, coverage: &[u8], side: usize) -> Vec<u8> {
        let count = side * side;
        self.outer.clear();
        self.inner.clear();
        for &alpha in coverage.iter().take(count) {
            let a = f32::from(alpha) / 255.0;
            let (outer, inner) = if alpha == 255 {
                (0.0, FAR)
            } else if alpha == 0 {
                (FAR, 0.0)
            } else {
                let d = 0.5 - a;
                (d.max(0.0).powi(2), (-d).max(0.0).powi(2))
            };
            self.outer.push(outer);
            self.inner.push(inner);
        }
        let mut outer = std::mem::take(&mut self.outer);
        let mut inner = std::mem::take(&mut self.inner);
        self.transform(&mut outer, side);
        self.transform(&mut inner, side);
        let field = outer
            .iter()
            .zip(&inner)
            .map(|(outer, inner)| {
                let distance = outer.sqrt() - inner.sqrt();
                let value = (0.5 - distance / (2.0 * SPREAD)).clamp(0.0, 1.0);
                // A fraction of 0..=1 times 255, rounded: fits a byte.
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "0..=255 after the clamp"
                )]
                let byte = (value * 255.0).round() as u8;
                byte
            })
            .collect();
        self.outer = outer;
        self.inner = inner;
        field
    }

    /// Squared Euclidean distance transform of a `side`² grid in place (Felzenszwalb and
    /// Huttenlocher: columns, then rows).
    fn transform(&mut self, grid: &mut [f32], side: usize) {
        self.f.resize(side, 0.0);
        self.d.resize(side, 0.0);
        self.v.resize(side, 0);
        self.z.resize(side + 1, 0.0);
        for x in 0..side {
            for y in 0..side {
                self.f[y] = grid[y * side + x];
            }
            self.line(side);
            for y in 0..side {
                grid[y * side + x] = self.d[y];
            }
        }
        for y in 0..side {
            self.f[..side].copy_from_slice(&grid[y * side..(y + 1) * side]);
            self.line(side);
            grid[y * side..(y + 1) * side].copy_from_slice(&self.d[..side]);
        }
    }

    /// One-dimensional squared distance transform of `f` into `d` (the paper's names).
    #[expect(clippy::cast_precision_loss, reason = "cell coordinates are small")]
    #[expect(clippy::many_single_char_names, reason = "the names of the paper")]
    fn line(&mut self, n: usize) {
        let (f, d, v, z) = (&self.f, &mut self.d, &mut self.v, &mut self.z);
        v[0] = 0;
        z[0] = -FAR;
        z[1] = FAR;
        let mut k = 0;
        for q in 1..n {
            // `z[0]` is below every intersection: `k` never drops below 0.
            let s = loop {
                let r = v[k];
                let s = ((f[q] + (q * q) as f32) - (f[r] + (r * r) as f32))
                    / (2.0 * (q as f32 - r as f32));
                if s <= z[k] && k > 0 {
                    k -= 1;
                } else {
                    break s;
                }
            };
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = FAR;
        }
        k = 0;
        for (q, out) in d.iter_mut().enumerate().take(n) {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let r = v[k];
            let delta = q as f32 - r as f32;
            *out = delta * delta + f[r];
        }
    }
}

/// The browser's canvas rasterizer.
#[cfg(target_arch = "wasm32")]
mod web {
    use wasm_bindgen::JsCast;

    use super::{BASELINE, CELL, FONT_PX, PEN, Rasterizer};

    /// Draws glyphs with an `OffscreenCanvas` 2D context.
    #[derive(Debug)]
    pub struct CanvasRasterizer {
        context: web_sys::OffscreenCanvasRenderingContext2d,
    }

    impl CanvasRasterizer {
        /// A rasterizer, or `None` where the browser has no `OffscreenCanvas`.
        pub fn new() -> Option<Self> {
            let canvas = web_sys::OffscreenCanvas::new(CELL, CELL).ok()?;
            let context = canvas
                .get_context("2d")
                .ok()??
                .dyn_into::<web_sys::OffscreenCanvasRenderingContext2d>()
                .ok()?;
            context.set_font(&format!("{FONT_PX}px sans-serif"));
            context.set_text_baseline("alphabetic");
            context.set_text_align("left");
            context.set_fill_style_str("#ffffff");
            Some(Self { context })
        }
    }

    impl Rasterizer for CanvasRasterizer {
        fn rasterize(&mut self, glyph: char, cell: &mut [u8]) -> Option<f32> {
            let side = f64::from(CELL);
            let mut buffer = [0; 4];
            let text = glyph.encode_utf8(&mut buffer);
            self.context.clear_rect(0.0, 0.0, side, side);
            self.context
                .fill_text(text, f64::from(PEN), f64::from(BASELINE))
                .ok()?;
            let advance = self.context.measure_text(text).ok()?.width();
            let image = self
                .context
                .get_image_data(0.0, 0.0, side, side)
                .ok()?
                .data();
            for (coverage, pixel) in cell.iter_mut().zip(image.as_chunks::<4>().0) {
                *coverage = pixel[3];
            }
            #[expect(
                clippy::cast_possible_truncation,
                reason = "an advance of a few dozen pixels"
            )]
            Some(advance as f32)
        }
    }
}
