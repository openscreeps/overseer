//! Room visuals of a bot (`RoomSnapshot::visuals`) as drawable primitives: lines, circles and
//! rectangles as signed-distance shapes, filled polygons as triangles, text as glyphs of the
//! atlas (and creeps' `say` bubbles the same way).
//!
//! Positions follow `RoomVisual`: whole numbers are tile centres. Defaults are the game's:
//! lines 0.1 wide, white, opacity 0.5; circles radius 0.15, white fill, opacity 0.5;
//! rectangles white fill, opacity 0.5; polygons a white 0.1 outline, opacity 0.5, closed
//! when filled; text white, 0.5 tiles high, centred, opacity 1, outline 0.15 when stroked,
//! background padding 0.3. Outlines default to 0.1 wide. `lineStyle` `dashed` and `dotted`
//! are followed for every outline. Shapes draw in the order given, then polygon fills
//! underneath them and text over them.

use glam::Vec2;
use overseer::core::socket::types::{
    RoomVisualItem, RoomVisualItemCircle, RoomVisualItemLine, RoomVisualItemPoly,
    RoomVisualItemRect, RoomVisualItemText, RoomVisualTextStyleFont,
};

use super::color::{self, Rgba};
use crate::mesh::{FillVertex, GlyphInstance, VisualInstance};
use crate::text::{Align, Atlas, TextStyle};

/// Kinds of [`VisualInstance`] (low byte of `kind`).
pub mod kind {
    /// A line segment.
    pub const LINE: u32 = 0;
    /// A circle.
    pub const CIRCLE: u32 = 1;
    /// A rectangle.
    pub const RECT: u32 = 2;
}

/// Dash styles of [`VisualInstance`] (second byte of `kind`).
pub mod dash {
    /// Solid.
    pub const SOLID: u32 = 0;
    /// Dashes.
    pub const DASHED: u32 = 1;
    /// Dots.
    pub const DOTTED: u32 = 2;
}

/// White (sRGB).
const WHITE: Rgba = [255, 255, 255, 255];
/// Colours of `say` bubbles (sRGB).
const BUBBLE: Rgba = [24, 26, 31, 215];
const BUBBLE_TEXT: Rgba = [236, 239, 244, 255];

/// The drawable room visuals of a snapshot.
#[derive(Debug, Default)]
pub struct Visuals {
    /// Shapes, in drawing order.
    pub prims: Vec<VisualInstance>,
    /// Polygon fills: a triangle list.
    pub fills: Vec<FillVertex>,
    /// Text glyphs.
    pub glyphs: Vec<GlyphInstance>,
    /// Bumped on every rebuild (the GPU uploads on change).
    pub version: u64,
}

/// A tile-centred visual coordinate as room-local tiles.
#[expect(
    clippy::cast_possible_truncation,
    reason = "room coordinates; f32 precision is plenty"
)]
fn at(x: f64, y: f64) -> Vec2 {
    Vec2::new(x as f32 + 0.5, y as f32 + 0.5)
}

/// A length or ratio as `f32`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "visual sizes; f32 precision is plenty"
)]
const fn float(value: f64) -> f32 {
    value as f32
}

/// `color` (or `fallback`) with its alpha scaled by `opacity`; `None` for no colour.
fn paint(color: Option<&str>, fallback: Option<Rgba>, opacity: Option<f64>) -> Option<Rgba> {
    let mut rgba = match color {
        Some(text) => color::parse(text)?,
        None => fallback?,
    };
    let opacity = opacity.map_or(1.0, float).clamp(0.0, 1.0);
    rgba[3] = color::unit(f32::from(rgba[3]) / 255.0 * opacity);
    (rgba[3] > 0).then_some(rgba)
}

fn dash_of(style: Option<&str>) -> u32 {
    match style {
        Some("dashed") => dash::DASHED,
        Some("dotted") => dash::DOTTED,
        _ => dash::SOLID,
    }
}

impl Visuals {
    /// Rebuilds the primitives from `items`; text is laid out with `atlas`.
    pub fn build(&mut self, items: &[RoomVisualItem], atlas: &mut Atlas) {
        self.prims.clear();
        self.fills.clear();
        self.glyphs.clear();
        self.version += 1;
        for item in items {
            match item {
                RoomVisualItem::Line(line) => self.line(line),
                RoomVisualItem::Circle(circle) => self.circle(circle),
                RoomVisualItem::Rect(rect) => self.rect(rect),
                RoomVisualItem::Poly(poly) => self.poly(poly),
                RoomVisualItem::Text(text) => self.text(text, atlas),
            }
        }
    }

    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.prims.is_empty() && self.fills.is_empty() && self.glyphs.is_empty()
    }

    fn segment(&mut self, a: Vec2, b: Vec2, width: f32, color: Rgba, dash: u32) {
        self.prims.push(VisualInstance {
            a: a.to_array(),
            b: b.to_array(),
            size: width.max(0.0),
            stroke_width: 0.0,
            fill: color,
            stroke: [0; 4],
            kind: kind::LINE | dash << 8,
        });
    }

    fn line(&mut self, line: &RoomVisualItemLine) {
        let style = line.s.as_ref();
        let Some(color) = paint(
            style.and_then(|s| s.color.as_deref()),
            Some(WHITE),
            Some(style.and_then(|s| s.opacity).unwrap_or(0.5)),
        ) else {
            return;
        };
        let width = float(style.and_then(|s| s.width).unwrap_or(0.1));
        let dash = dash_of(style.and_then(|s| s.line_style.as_deref()));
        self.segment(
            at(line.x1, line.y1),
            at(line.x2, line.y2),
            width,
            color,
            dash,
        );
    }

    fn circle(&mut self, circle: &RoomVisualItemCircle) {
        let style = circle.s.as_ref();
        let opacity = Some(style.and_then(|s| s.opacity).unwrap_or(0.5));
        let fill = paint(style.and_then(|s| s.fill.as_deref()), Some(WHITE), opacity);
        let stroke = paint(style.and_then(|s| s.stroke.as_deref()), None, opacity);
        if fill.is_none() && stroke.is_none() {
            return;
        }
        let centre = at(circle.x, circle.y);
        self.prims.push(VisualInstance {
            a: centre.to_array(),
            b: centre.to_array(),
            size: float(style.and_then(|s| s.radius).unwrap_or(0.15)).max(0.0),
            stroke_width: stroke.map_or(0.0, |_| {
                float(style.and_then(|s| s.stroke_width).unwrap_or(0.1))
            }),
            fill: fill.unwrap_or_default(),
            stroke: stroke.unwrap_or_default(),
            kind: kind::CIRCLE | dash_of(style.and_then(|s| s.line_style.as_deref())) << 8,
        });
    }

    fn rect(&mut self, rect: &RoomVisualItemRect) {
        let style = rect.s.as_ref();
        let opacity = Some(style.and_then(|s| s.opacity).unwrap_or(0.5));
        let fill = paint(style.and_then(|s| s.fill.as_deref()), Some(WHITE), opacity);
        let stroke = paint(style.and_then(|s| s.stroke.as_deref()), None, opacity);
        if fill.is_none() && stroke.is_none() {
            return;
        }
        let corner = at(rect.x, rect.y);
        let far = corner + Vec2::new(float(rect.w), float(rect.h));
        self.prims.push(VisualInstance {
            a: corner.min(far).to_array(),
            b: corner.max(far).to_array(),
            size: 0.0,
            stroke_width: stroke.map_or(0.0, |_| {
                float(style.and_then(|s| s.stroke_width).unwrap_or(0.1))
            }),
            fill: fill.unwrap_or_default(),
            stroke: stroke.unwrap_or_default(),
            kind: kind::RECT | dash_of(style.and_then(|s| s.line_style.as_deref())) << 8,
        });
    }

    fn poly(&mut self, poly: &RoomVisualItemPoly) {
        let style = poly.s.as_ref();
        let opacity = Some(style.and_then(|s| s.opacity).unwrap_or(0.5));
        let fill = paint(style.and_then(|s| s.fill.as_deref()), None, opacity);
        let stroke = paint(
            style.and_then(|s| s.stroke.as_deref()),
            Some(WHITE),
            opacity,
        );
        let points: Vec<Vec2> = poly
            .points
            .iter()
            .filter_map(|point| Some(at(*point.first()?, *point.get(1)?)))
            .collect();
        if points.len() < 2 {
            return;
        }
        if let Some(fill) = fill {
            for triangle in triangulate(&points) {
                for index in triangle {
                    self.fills.push(FillVertex {
                        position: points[index].to_array(),
                        color: fill,
                    });
                }
            }
        }
        if let Some(stroke) = stroke {
            let width = float(style.and_then(|s| s.stroke_width).unwrap_or(0.1));
            let dash = dash_of(style.and_then(|s| s.line_style.as_deref()));
            for pair in points.windows(2) {
                self.segment(pair[0], pair[1], width, stroke, dash);
            }
            if fill.is_some()
                && let (Some(first), Some(last)) = (points.first(), points.last())
                && first != last
            {
                self.segment(*last, *first, width, stroke, dash);
            }
        }
    }

    fn text(&mut self, text: &RoomVisualItemText, atlas: &mut Atlas) {
        let style = text.s.as_ref();
        let opacity = Some(style.and_then(|s| s.opacity).unwrap_or(1.0));
        let Some(color) = paint(style.and_then(|s| s.color.as_deref()), Some(WHITE), opacity)
        else {
            return;
        };
        let size = match style.and_then(|s| s.font.as_ref()) {
            Some(RoomVisualTextStyleFont::Number(size)) => float(*size),
            Some(RoomVisualTextStyleFont::String(font)) => font
                .split_whitespace()
                .find_map(|token| token.parse::<f32>().ok())
                .unwrap_or(0.5),
            None => style.and_then(|s| s.size).map_or(0.5, float),
        };
        if !(size.is_finite() && size > 0.0) {
            return;
        }
        let align = match style.and_then(|s| s.align.as_deref()) {
            Some("left") => Align::Left,
            Some("right") => Align::Right,
            _ => Align::Center,
        };
        let stroke = paint(style.and_then(|s| s.stroke.as_deref()), None, opacity);
        let text_style = TextStyle {
            size,
            align,
            color,
            stroke: stroke.unwrap_or_default(),
            stroke_width: float(style.and_then(|s| s.stroke_width).unwrap_or(0.15)),
        };
        let background = paint(
            style.and_then(|s| s.background_color.as_deref()),
            None,
            opacity,
        );
        let padding = float(style.and_then(|s| s.background_padding).unwrap_or(0.3));
        // Visual text has its baseline at `y`, a tile centre.
        let anchor = at(text.x, text.y);
        self.label(&text.text, anchor, &text_style, background, padding, atlas);
    }

    /// A line of text with an optional background box.
    fn label(
        &mut self,
        text: &str,
        anchor: Vec2,
        style: &TextStyle,
        background: Option<Rgba>,
        padding: f32,
        atlas: &mut Atlas,
    ) {
        let start = self.glyphs.len();
        let (left, right) = atlas.layout(text, (anchor.x, anchor.y), style, &mut self.glyphs);
        if let Some(background) = background {
            let pad = padding * style.size;
            self.prims.push(VisualInstance {
                a: [left - pad, anchor.y - 0.78 * style.size - pad],
                b: [right + pad, anchor.y + 0.22 * style.size + pad],
                size: 0.0,
                stroke_width: 0.0,
                fill: background,
                stroke: [0; 4],
                kind: kind::RECT,
            });
        }
        debug_assert!(self.glyphs.len() >= start);
    }

    /// A creep's `say` bubble over the tile `tile` (room-local, whole tiles).
    pub fn say(&mut self, message: &str, tile: Vec2, atlas: &mut Atlas) {
        let message: String = message.chars().take(10).collect();
        if message.trim().is_empty() {
            return;
        }
        let style = TextStyle {
            size: 0.42,
            align: Align::Center,
            color: BUBBLE_TEXT,
            stroke: [0; 4],
            stroke_width: 0.0,
        };
        let anchor = tile + Vec2::new(0.5, -0.32);
        self.label(&message, anchor, &style, Some(BUBBLE), 0.25, atlas);
    }
}

/// Triangles (index triples) covering a simple polygon, by ear clipping; a fan from the
/// first point when the outline crosses itself.
#[expect(
    clippy::many_single_char_names,
    reason = "the triangle's corners and the point tested"
)]
pub fn triangulate(points: &[Vec2]) -> Vec<[usize; 3]> {
    let mut indices: Vec<usize> = (0..points.len()).collect();
    if points.len() > 2 && points.first() == points.last() {
        indices.pop();
    }
    let n = indices.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| points[indices[i]].perp_dot(points[indices[(i + 1) % n]]))
        .sum();
    if area < 0.0 {
        indices.reverse();
    }
    let mut triangles = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while indices.len() > 3 && guard < n * n {
        guard += 1;
        let len = indices.len();
        let ear = (0..len).find(|&i| {
            let (a, b, c) = (
                points[indices[(i + len - 1) % len]],
                points[indices[i]],
                points[indices[(i + 1) % len]],
            );
            if (b - a).perp_dot(c - b) <= 0.0 {
                return false;
            }
            !indices.iter().any(|&other| {
                let p = points[other];
                p != a && p != b && p != c && inside_triangle(p, a, b, c)
            })
        });
        let Some(i) = ear else {
            break;
        };
        triangles.push([
            indices[(i + len - 1) % len],
            indices[i],
            indices[(i + 1) % len],
        ]);
        indices.remove(i);
    }
    if indices.len() == 3 {
        triangles.push([indices[0], indices[1], indices[2]]);
        return triangles;
    }
    // Not simple: a fan.
    (1..n - 1).map(|i| [0, i, i + 1]).collect()
}

/// Whether `p` lies inside the counter-clockwise triangle `a`, `b`, `c` (edges included).
fn inside_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    (b - a).perp_dot(p - a) >= 0.0
        && (c - b).perp_dot(p - b) >= 0.0
        && (a - c).perp_dot(p - c) >= 0.0
}
