//! The camera: the selection frame, the bounds of a room scope, the saved camera state,
//! flights to a room and the framing of the scope or of one room.

use glam::{Vec2, Vec3};

use super::{SIZE, drag, place, room, shard, view, world_view};
use crate::camera::{rect_extent, room_centre};
use crate::view::View;
use crate::world::{Limits, tile_origin};
use crate::{CameraState, Lod, PointerButton, Scope, ViewMode};

#[test]
fn the_selection_frame_sits_on_the_tile_and_stays_visible_when_zoomed_out() {
    let mut view = view(ViewMode::TopDown);
    let w1n1 = room("W1N1");
    assert_eq!(view.selection(), None);
    view.select(Some((w1n1, 3, 3)));
    let ring = view.selection().unwrap();
    let corner = tile_origin(w1n1, 3, 3);
    assert_eq!(
        ring.offset,
        [corner.x + 0.5, crate::mesh::WALL_HEIGHT, corner.y + 0.5]
    );
    let mut world = world_view();
    world.select(Some((w1n1, 3, 3)));
    let ring = world.selection().unwrap();
    let pixels = ring.scale[0] / world.camera.tiles_per_px();
    assert!((pixels - 14.0).abs() < 0.1, "{pixels} px wide at overview");
    world.select(Some((room("W70N1"), 3, 3)));
    assert_eq!(world.selection(), None, "outside the scope");
}

#[test]
fn a_room_scope_keeps_the_camera_over_its_room() {
    let w5n5 = room("W5N5");
    let mut view = View::new(Limits::WEBGL2);
    view.set_scope(Scope::Room(w5n5));
    view.resize(SIZE.x, SIZE.y);
    let (min, max) = rect_extent(Scope::Room(w5n5).bounds());
    let inside = |view: &View| {
        let target = view.camera.target();
        target.cmpge(min).all() && target.cmple(max).all()
    };
    assert_eq!(view.camera.target(), room_centre(w5n5), "framed");
    assert_eq!(view.lod(), Lod::Detail);
    assert_eq!(view.focused_room(), Some(w5n5));
    view.pan_by(Vec2::new(5000.0, 5000.0));
    assert_eq!(
        view.camera.target(),
        max,
        "panned into the south-east corner"
    );
    let fit = view.camera.half_height();
    view.zoom_by(0.0001);
    assert!(
        view.camera.half_height() <= 50.0,
        "zoomed out to {}",
        view.camera.half_height()
    );
    assert!(view.camera.half_height() > fit);
    assert!(inside(&view));
    // Neighbouring rooms show at the edge but do not pick.
    assert_eq!(view.pick(SIZE - Vec2::splat(2.0)), None);
    assert_eq!(view.focused_room(), Some(w5n5));
    view.set_camera_state(CameraState {
        centre: [1e6, -1e6],
        half_height: 1e9,
        mode: ViewMode::Perspective,
        yaw: 1.0,
        pitch: 9.0,
    });
    assert!(inside(&view));
    assert!(view.camera.half_height() <= 50.0);
    assert!(view.camera_state().pitch < 1.6);
    // A flight to another room stops at the edge of this one.
    view.look_at(room("W9N9"), Some((3, 3)), false, 0.0);
    assert!(inside(&view));
}

#[test]
fn camera_state_round_trips() {
    let mut view = world_view();
    view.set_mode(ViewMode::Perspective);
    place(&mut view, Vec2::new(-1234.5, 678.25), 140.0);
    drag(
        &mut view,
        PointerButton::Primary,
        Vec2::new(400.0, 300.0),
        Vec2::new(300.0, 320.0),
    );
    let state = view.camera_state();
    let json = serde_json::to_string(&state).unwrap();
    assert!(json.contains("\"mode\":\"Perspective\""), "{json}");
    let parsed: CameraState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, state);
    let mut other = world_view();
    other.set_camera_state(parsed);
    assert_eq!(other.camera_state(), state);
    assert_eq!(
        other.camera.view_projection(),
        view.camera.view_projection()
    );
    assert_eq!(other.interest(), view.interest());
    // Garbage is sanitized.
    other.set_camera_state(CameraState {
        centre: [f32::NAN, f32::INFINITY],
        half_height: f32::NAN,
        mode: ViewMode::TopDown,
        yaw: f32::NAN,
        pitch: f32::NAN,
    });
    let state = other.camera_state();
    assert!(state.centre.iter().all(|value| value.is_finite()));
    assert!(state.half_height.is_finite() && state.yaw.is_finite() && state.pitch.is_finite());
}

#[test]
fn look_at_flies_and_fit_frames_the_scope() {
    let mut view = world_view();
    let e3s5 = room("E3S5");
    view.look_at(e3s5, None, true, 1000.0);
    assert!(
        view.advance(1000.0 + crate::FLIGHT_MS / 2.0),
        "still flying"
    );
    let middle = view.camera.half_height();
    assert!(!view.advance(1000.0 + crate::FLIGHT_MS), "landed");
    assert_eq!(view.camera.target(), room_centre(e3s5));
    assert_eq!(view.focused_room(), Some(e3s5));
    assert_eq!(view.lod(), Lod::Detail);
    assert!(middle > view.camera.half_height());
    view.look_at(e3s5, Some((10, 40)), false, 0.0);
    assert_eq!(
        view.camera.target(),
        tile_origin(e3s5, 10, 40) + Vec2::splat(0.5)
    );
    view.fit(false, 0.0);
    assert_eq!(view.lod(), Lod::Overview);
    assert_eq!(view.interest().stats, Some(shard()));
    // A gesture interrupts a flight.
    view.look_at(e3s5, None, true, 0.0);
    view.pan_by(Vec2::new(10.0, 0.0));
    assert!(!view.advance(100.0));
}

#[test]
fn frame_room_shows_the_whole_room_from_any_zoom() {
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = world_view();
        view.set_mode(mode);
        let e3s5 = room("E3S5");
        // Zoomed in on a corner tile, then out to the whole shard: both land on the room.
        view.look_at(e3s5, Some((2, 3)), false, 0.0);
        view.zoom_by(100.0);
        for start in ["close", "far"] {
            view.frame_room(e3s5, false, 0.0);
            assert_eq!(view.camera.target(), room_centre(e3s5), "{mode:?} {start}");
            assert_eq!(view.camera.half_height(), view.camera.room_half_height());
            assert_eq!(view.focused_room(), Some(e3s5));
            let origin = tile_origin(e3s5, 0, 0);
            for corner in [(0.0, 0.0), (50.0, 0.0), (0.0, 50.0), (50.0, 50.0)] {
                let point = origin + Vec2::new(corner.0, corner.1);
                let pixel = view
                    .camera
                    .project(Vec3::new(point.x, 0.0, point.y))
                    .unwrap();
                assert!(
                    pixel.cmpge(Vec2::ZERO).all() && pixel.cmple(view.camera.size()).all(),
                    "{mode:?} {start}: corner {corner:?} at {pixel} off the canvas"
                );
            }
            view.fit(false, 0.0);
        }
        // Animated: lands on the same framing.
        view.frame_room(e3s5, true, 0.0);
        assert!(!view.advance(crate::FLIGHT_MS));
        assert_eq!(view.camera.half_height(), view.camera.room_half_height());
    }
}
