//! Projection and picking: rays through projected tiles, the tile under the pointer in
//! one room and across room borders, and drags that pan or orbit around the ground point.

use glam::Vec2;
use overseer::map::RoomCoord;

use super::{SIZE, drag, place, room, snapshot_of, terrain, tile_centre, view, world_view};
use crate::view::View;
use crate::world::tile_origin;
use crate::{PointerButton, ViewMode};

/// Every probe tile whose top centre is on screen picks itself, and the ray through the
/// projected point passes through the world point.
fn assert_round_trips(view: &View, probes: &[(RoomCoord, u8, u8)]) {
    let mut checked = 0;
    for &(room, x, y) in probes {
        let world = tile_centre(view, room, x, y);
        let Some(point) = view.camera.project(world) else {
            continue;
        };
        if !(0.0..SIZE.x).contains(&point.x) || !(0.0..SIZE.y).contains(&point.y) {
            continue;
        }
        let (origin, direction) = view.camera.ray(point);
        let distance = (world - origin).cross(direction).length();
        assert!(
            distance < 2e-2,
            "ray misses {room} ({x}, {y}) by {distance}"
        );
        let pick = view.pick(point);
        let picked = pick.as_ref().map(|pick| (pick.room, pick.x, pick.y));
        assert_eq!(
            picked,
            Some((room, x, y)),
            "{:?} at {point}, ray {origin} {direction}",
            view.camera.mode
        );
        checked += 1;
    }
    assert!(checked >= 3, "only {checked} probe tiles on screen");
}

const W1N1_PROBES: [(u8, u8); 7] = [
    (25, 25),
    (2, 20),
    (32, 32),
    (10, 40),
    (40, 8),
    (12, 12),
    (47, 45),
];

fn w1n1_probes() -> Vec<(RoomCoord, u8, u8)> {
    W1N1_PROBES
        .iter()
        .map(|&(x, y)| (room("W1N1"), x, y))
        .collect()
}

#[test]
fn projection_and_picking_round_trip_in_both_modes() {
    let probes = w1n1_probes();
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = view(mode);
        assert_round_trips(&view, &probes);
        drag(
            &mut view,
            PointerButton::Secondary,
            Vec2::new(400.0, 300.0),
            Vec2::new(470.0, 260.0),
        );
        assert_round_trips(&view, &probes);
        view.wheel(Vec2::new(300.0, 200.0), -300.0, 0.0);
        assert_round_trips(&view, &probes);
        if mode == ViewMode::Perspective {
            drag(
                &mut view,
                PointerButton::Primary,
                Vec2::new(400.0, 300.0),
                Vec2::new(250.0, 340.0),
            );
            assert_round_trips(&view, &probes);
        }
    }
}

#[test]
fn top_down_picks_the_tile_under_the_pointer() {
    let view = view(ViewMode::TopDown);
    let w1n1 = room("W1N1");
    let origin = tile_origin(w1n1, 0, 0);
    let target = view.camera.target();
    let scale = 1.0 / view.camera.tiles_per_px();
    let point = |x: f32, y: f32| SIZE / 2.0 + (origin + Vec2::new(x, y) - target) * scale;
    let pick = view.pick(point(10.5, 20.5)).unwrap();
    assert_eq!((pick.room, pick.x, pick.y), (w1n1, 10, 20));
    let pick = view.pick(point(49.9, 0.1)).unwrap();
    assert_eq!((pick.room, pick.x, pick.y), (w1n1, 49, 0));
    assert_eq!(view.pick(point(50.2, 25.0)), None, "outside the room scope");
    assert_eq!(view.pick(point(25.0, -0.2)), None, "outside the room scope");
}

#[test]
fn pan_keeps_the_dragged_ground_point_under_the_pointer() {
    for (mode, button) in [
        (ViewMode::TopDown, PointerButton::Primary),
        (ViewMode::Perspective, PointerButton::Secondary),
        (ViewMode::Perspective, PointerButton::Middle),
    ] {
        let mut view = view(mode);
        let (from, to) = (Vec2::new(400.0, 300.0), Vec2::new(480.0, 350.0));
        let grabbed = view.camera.ground_point(from, 0.0).unwrap();
        drag(&mut view, button, from, to);
        let under = view.camera.ground_point(to, 0.0).unwrap();
        assert!(
            grabbed.distance(under) < 1e-2,
            "{mode:?}: {grabbed} → {under}"
        );
    }
}

#[test]
fn perspective_primary_drag_orbits_around_the_target() {
    let mut view = view(ViewMode::Perspective);
    let centre = SIZE / 2.0;
    let target = view.camera.ground_point(centre, 0.0).unwrap();
    let before = view.camera.ray(centre).1;
    drag(
        &mut view,
        PointerButton::Primary,
        centre,
        Vec2::new(300.0, 330.0),
    );
    let after = view.camera.ray(centre).1;
    assert!(before.angle_between(after) > 0.2, "the camera turned");
    let still = view.camera.ground_point(centre, 0.0).unwrap();
    assert!(
        target.distance(still) < 1e-2,
        "the target moved: {target} → {still}"
    );
}

#[test]
fn picking_crosses_room_borders_in_both_modes() {
    let (west, east) = (room("W0S0"), room("E0S0"));
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = world_view();
        view.set_mode(mode);
        let terrain = terrain();
        view.world
            .set_room(&snapshot_of("W0S0", &terrain, Vec::new()), 0.0);
        view.world
            .set_room(&snapshot_of("E0S0", &terrain, Vec::new()), 0.0);
        // The border between the two rooms is the world line x = 0.
        place(&mut view, Vec2::new(0.0, 25.0), 600.0);
        let probes = [
            (west, 49, 25),
            (west, 47, 20),
            (west, 45, 30),
            (east, 0, 25),
            (east, 2, 21),
            (east, 6, 30),
            (east, 8, 24),
        ];
        assert_round_trips(&view, &probes);
        // Rooms without a detail scene pick on the ground, across borders.
        place(&mut view, Vec2::new(-50.0, -50.0), 30.0);
        for (room, x, y) in [
            (room("W1N0"), 10, 40),
            (room("W0N0"), 1, 1),
            (room("W1N1"), 49, 49),
        ] {
            let point = view.camera.project(tile_centre(&view, room, x, y)).unwrap();
            let pick = view.pick(point).unwrap();
            assert_eq!((pick.room, pick.x, pick.y), (room, x, y), "{mode:?}");
            assert_eq!(pick.objects, [] as [String; 0]);
        }
    }
}
