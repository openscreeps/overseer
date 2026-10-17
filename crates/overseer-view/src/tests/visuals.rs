//! Room visuals: conversion of every item kind with the game's defaults, CSS colours,
//! polygon fills, text layout through the glyph atlas and its distance fields.

use glam::Vec2;
use overseer::model::world::RoomVisualItem;
use serde_json::json;

use super::{ME, object, show, snapshot, terrain};
use crate::scene::Scene;
use crate::scene::color::parse;
use crate::scene::visuals::{Visuals, dash, kind, triangulate};
use crate::text::{Atlas, BASELINE, CELL, FONT_PX, PEN, Rasterizer, SPREAD};

/// Draws every glyph as a 20×28-pixel box on the baseline, 24 pixels wide.
#[derive(Debug, Default)]
pub struct Boxes {
    drawn: std::rc::Rc<std::cell::Cell<usize>>,
}

impl Rasterizer for Boxes {
    fn rasterize(&mut self, _glyph: char, cell: &mut [u8]) -> Option<f32> {
        self.drawn.set(self.drawn.get() + 1);
        let side = CELL as usize;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "cell pixels"
        )]
        let (left, baseline) = (PEN as usize, BASELINE as usize);
        for y in baseline - 28..baseline {
            for x in left..left + 20 {
                cell[y * side + x] = 255;
            }
        }
        Some(24.0)
    }
}

fn items(value: serde_json::Value) -> Vec<RoomVisualItem> {
    serde_json::from_value(value).unwrap()
}

fn build(value: serde_json::Value) -> Visuals {
    let mut visuals = Visuals::default();
    visuals.build(
        &items(value),
        &mut Atlas::new(Some(Box::new(Boxes::default()))),
    );
    visuals
}

#[test]
fn shapes_take_the_games_defaults() {
    let visuals = build(json!([
        {"t": "l", "x1": 1, "y1": 2, "x2": 3, "y2": 4},
        {"t": "c", "x": 5, "y": 5},
        {"t": "r", "x": 10, "y": 10, "w": 2, "h": 1},
        {"t": "p", "points": [[0, 0], [1, 0], [1, 1]]},
    ]));
    let [line, circle, rect] = [visuals.prims[0], visuals.prims[1], visuals.prims[2]];
    // Whole numbers are tile centres.
    assert_eq!((line.a, line.b), ([1.5, 2.5], [3.5, 4.5]));
    assert_eq!(line.kind, kind::LINE);
    assert!((line.size - 0.1).abs() < 1e-6);
    assert_eq!(line.fill, [255, 255, 255, 128], "white at opacity 0.5");
    assert_eq!((circle.a, circle.kind), ([5.5, 5.5], kind::CIRCLE));
    assert!((circle.size - 0.15).abs() < 1e-6);
    assert_eq!(
        (circle.stroke_width, circle.fill[3]),
        (0.0, 128),
        "filled, no outline"
    );
    assert_eq!(
        (rect.a, rect.b, rect.kind),
        ([10.5, 10.5], [12.5, 11.5], kind::RECT)
    );
    // A polygon without fill: its outline, open.
    assert!(visuals.fills.is_empty(), "no fill");
    assert_eq!(visuals.prims.len(), 3 + 2);
    assert!(
        visuals.prims[3..]
            .iter()
            .all(|prim| prim.kind == kind::LINE)
    );
    assert!(visuals.glyphs.is_empty(), "no text");
}

#[test]
fn styles_follow_colours_opacity_widths_and_dashes() {
    let visuals = build(json!([
        {"t": "l", "x1": 0, "y1": 0, "x2": 1, "y2": 0,
         "s": {"color": "#f00", "width": 0.3, "opacity": 1, "lineStyle": "dashed"}},
        {"t": "c", "x": 0, "y": 0,
         "s": {"fill": "transparent", "stroke": "rgba(0, 255, 0, 0.5)", "strokeWidth": 0.2,
               "radius": 2, "lineStyle": "dotted", "opacity": 1}},
        {"t": "r", "x": 0, "y": 0, "w": 1, "h": 1, "s": {"fill": "transparent"}},
        {"t": "p", "points": [[0, 0], [4, 0], [4, 4], [2, 4], [2, 2], [0, 2]],
         "s": {"fill": "navy", "stroke": "#ffffff80", "opacity": 0.5}},
    ]));
    let line = visuals.prims[0];
    assert_eq!(line.fill, [255, 0, 0, 255]);
    assert_eq!(line.kind >> 8, dash::DASHED);
    assert!((line.size - 0.3).abs() < 1e-6);
    let circle = visuals.prims[1];
    assert_eq!(circle.fill[3], 0, "transparent: no fill");
    assert_eq!(circle.stroke, [0, 255, 0, 128]);
    assert_eq!(circle.kind >> 8, dash::DOTTED);
    assert!((circle.size - 2.0).abs() < 1e-6 && (circle.stroke_width - 0.2).abs() < 1e-6);
    // The transparent rectangle draws nothing; the filled L-shaped polygon is closed.
    let outline = &visuals.prims[2..];
    assert_eq!(outline.len(), 6, "five sides and the closing one");
    assert_eq!(
        outline[0].fill,
        [255, 255, 255, 64],
        "#ffffff80 at opacity 0.5"
    );
    // Fill triangles cover the L exactly: 4 × 4 minus 2 × 2.
    let area: f32 = visuals
        .fills
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let [a, b, c] = [0, 1, 2].map(|i| Vec2::from(t[i].position));
            (b - a).perp_dot(c - a).abs() / 2.0
        })
        .sum();
    assert!((area - 12.0).abs() < 1e-4, "{area}");
    assert_eq!(visuals.fills[0].color, [0, 0, 128, 128]);
}

#[test]
fn css_colours_parse() {
    assert_eq!(parse("#abc"), Some([0xaa, 0xbb, 0xcc, 255]));
    assert_eq!(parse("#a1b2c3"), Some([0xa1, 0xb2, 0xc3, 255]));
    assert_eq!(parse("#a1b2c3d4"), Some([0xa1, 0xb2, 0xc3, 0xd4]));
    assert_eq!(parse(" RGB(10, 20, 30) "), Some([10, 20, 30, 255]));
    assert_eq!(parse("rgba(100%, 0%, 50%, 0.25)"), Some([255, 0, 128, 64]));
    assert_eq!(parse("rgb(1 2 3 / 50%)"), Some([1, 2, 3, 128]));
    assert_eq!(parse("RebeccaPurple"), Some([0x66, 0x33, 0x99, 255]));
    assert_eq!(parse("transparent"), None);
    assert_eq!(parse("#12"), None);
    assert_eq!(parse("not a colour"), None);
}

#[test]
fn concave_and_crossing_polygons_triangulate() {
    let arrow = [
        Vec2::new(0.0, 0.0),
        Vec2::new(4.0, 2.0),
        Vec2::new(0.0, 4.0),
        Vec2::new(1.0, 2.0),
    ];
    assert_eq!(triangulate(&arrow).len(), 2);
    // Clockwise and closed (first point repeated) alike.
    let mut square = vec![
        Vec2::new(0.0, 0.0),
        Vec2::new(0.0, 1.0),
        Vec2::new(1.0, 1.0),
        Vec2::new(1.0, 0.0),
    ];
    square.push(square[0]);
    assert_eq!(triangulate(&square).len(), 2);
    let bow = [
        Vec2::new(0.0, 0.0),
        Vec2::new(1.0, 1.0),
        Vec2::new(1.0, 0.0),
        Vec2::new(0.0, 1.0),
    ];
    assert_eq!(
        triangulate(&bow).len(),
        2,
        "a fan where the outline crosses itself"
    );
    assert!(triangulate(&arrow[..2]).is_empty(), "two points");
}

#[test]
fn text_is_laid_out_from_the_atlas() {
    let boxes = Boxes::default();
    let drawn = std::rc::Rc::clone(&boxes.drawn);
    let mut atlas = Atlas::new(Some(Box::new(boxes)));
    let mut visuals = Visuals::default();
    let text = items(json!([
        {"t": "t", "text": "aba a", "x": 10, "y": 20},
        {"t": "t", "text": "ab", "x": 10, "y": 30,
         "s": {"align": "left", "font": "0.7 monospace", "color": "#00ff00",
               "backgroundColor": "#000000", "stroke": "#ff0000", "strokeWidth": 0.1}},
    ]));
    visuals.build(&text, &mut atlas);
    assert_eq!(drawn.get(), 2, "each glyph rasterized once");
    assert_eq!(atlas.pending(), 2);
    // Centred: "aba a" is 4 × 0.6 + 0.28 ems at 0.5 tiles per em.
    let width = f32::midpoint(4.0 * 0.6, 0.28);
    let first = visuals.glyphs[0];
    let scale = 0.5 / FONT_PX;
    assert!(
        (first.rect[0] - (10.5 - width / 2.0 - PEN * scale)).abs() < 1e-5,
        "{first:?}"
    );
    assert!((first.rect[1] - (20.5 - BASELINE * scale)).abs() < 1e-5);
    assert_eq!(visuals.glyphs.len(), 4 + 2, "spaces take room, not glyphs");
    assert_eq!(
        visuals.glyphs[0].uv, visuals.glyphs[2].uv,
        "one cell per glyph"
    );
    assert_ne!(visuals.glyphs[0].uv, visuals.glyphs[1].uv);
    // Left-aligned, 0.7 tiles high, outlined, on a background.
    let left = visuals.glyphs[4];
    assert!((left.rect[0] - (10.5 - PEN * 0.7 / FONT_PX)).abs() < 1e-5);
    assert_eq!(left.color, [0, 255, 0, 255]);
    assert!(left.stroke_width > 0.0 && left.stroke[3] == 255);
    let background = visuals.prims[0];
    assert_eq!(
        (background.kind, background.fill),
        (kind::RECT, [0, 0, 0, 255])
    );
    assert!(background.a[0] < 10.5 && background.b[0] > 10.5 + 2.0 * 0.6 * 0.7);
    // Uploads: one cell each, then nothing.
    let mut origins = Vec::new();
    atlas.flush(|origin, field| {
        assert_eq!(field.len(), (CELL * CELL) as usize);
        origins.push(origin);
    });
    assert_eq!(origins, [(0, 0), (CELL, 0)]);
    assert_eq!(atlas.pending(), 0);
    // Without a rasterizer (native builds): laid out, not drawn.
    let mut bare = Visuals::default();
    bare.build(&text, &mut Atlas::default());
    assert!(bare.glyphs.is_empty(), "nothing rasterized");
    assert_eq!(bare.prims.len(), 1, "the background still shows");
}

#[test]
fn glyph_fields_hold_distances_to_the_outline() {
    let mut atlas = Atlas::new(Some(Box::new(Boxes::default())));
    let mut glyphs = Vec::new();
    let style = crate::text::TextStyle {
        size: 1.0,
        align: crate::text::Align::Left,
        color: [255; 4],
        stroke: [0; 4],
        stroke_width: 0.0,
    };
    atlas.layout("x", (0.0, 0.0), &style, &mut glyphs);
    let mut field = Vec::new();
    atlas.flush(|_, data| field = data.to_vec());
    let side = CELL as usize;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "cell pixels"
    )]
    let (left, baseline) = (PEN as usize, BASELINE as usize);
    let at = |x: usize, y: usize| field[y * side + x];
    // Deep inside the 20 × 28 box (10 pixels from its sides): fully in.
    assert_eq!(at(left + 10, baseline - 14), 255);
    // Far outside: fully out.
    assert_eq!(at(0, 0), 0);
    // On the outline: half way; a few pixels out: proportionally lower.
    let edge = at(left, baseline - 14);
    assert!((110..=145).contains(&edge), "{edge}");
    let out = f32::from(at(left - 5, baseline - 14)) / 255.0;
    let expected = 0.5 - 4.5 / (2.0 * SPREAD);
    assert!((out - expected).abs() < 0.06, "{out} vs {expected}");
}

#[test]
fn scenes_show_visuals_and_say_bubbles() {
    let terrain = terrain();
    let mut scene = Scene::default();
    let mut snapshot = snapshot(
        &terrain,
        vec![object(json!({
            "_id": "talker", "type": "creep", "room": "W1N1", "x": 5, "y": 5, "user": ME,
            "name": "talker", "hits": 100, "hitsMax": 100, "store": {},
            "body": [{"type": "move", "hits": 100}],
            "actionLog": {"say": {"message": "hello", "isPublic": true}},
        }))],
    );
    snapshot.visuals = items(json!([{"t": "c", "x": 1, "y": 1}]));
    show(&mut scene, &snapshot, 0.0);
    let visuals = scene.visuals();
    assert_eq!(visuals.prims.len(), 2, "the circle and the bubble's box");
    assert_eq!(visuals.prims[1].kind, kind::RECT);
    let version = visuals.version;
    scene.set_features(crate::scene::Features {
        visuals: false,
        ..crate::scene::Features::default()
    });
    assert!(scene.visuals().is_empty());
    assert!(scene.visuals().version > version, "the GPU copy follows");
}
