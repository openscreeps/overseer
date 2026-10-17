//! Pointer gestures: the wheel and pinch zoom, clicks that pick a tile, and pointers whose
//! release never arrived.

use glam::Vec2;
use overseer::map::RoomCoord;

use super::{ME, creep, energy, room, snapshot, structure, terrain, tile_centre, view};
use crate::view::View;
use crate::{PointerButton, ViewMode};

#[test]
fn wheel_and_pinch_zoom_around_the_pointer() {
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = view(mode);
        let anchor = Vec2::new(200.0, 150.0);
        let span = |view: &View| {
            let a = view
                .camera
                .ground_point(Vec2::new(350.0, 300.0), 0.0)
                .unwrap();
            let b = view
                .camera
                .ground_point(Vec2::new(450.0, 300.0), 0.0)
                .unwrap();
            a.distance(b)
        };
        let grabbed = view.camera.ground_point(anchor, 0.0).unwrap();
        let wide = span(&view);
        view.wheel(anchor, -400.0, 0.0);
        assert!(span(&view) < wide * 0.7, "{mode:?}: wheel up zooms in");
        let under = view.camera.ground_point(anchor, 0.0).unwrap();
        assert!(
            grabbed.distance(under) < 1e-2,
            "{mode:?}: zoom anchor moved"
        );

        let narrow = span(&view);
        view.pointer_down(1, Vec2::new(380.0, 300.0), PointerButton::Primary, true);
        view.pointer_down(2, Vec2::new(420.0, 300.0), PointerButton::Primary, false);
        view.pointer_move(1, Vec2::new(300.0, 300.0), 0.0);
        view.pointer_move(2, Vec2::new(500.0, 300.0), 0.0);
        assert!(
            span(&view) < narrow * 0.5,
            "{mode:?}: spreading fingers zooms in"
        );
        assert_eq!(view.pointer_up(1, Vec2::new(300.0, 300.0), 0.0, 0.0), None);
        assert_eq!(
            view.pointer_up(2, Vec2::new(500.0, 300.0), 0.0, 0.0),
            None,
            "a pinch is no click"
        );
    }
}

#[test]
fn a_click_returns_the_room_tile_and_its_objects_topmost_first() {
    let mut view = view(ViewMode::TopDown);
    let objects = vec![
        structure("road", "road", 20, 20),
        structure("spawn", "spawn", 20, 20),
        structure("rampart", "rampart", 20, 20),
        energy("energy", 20, 20),
        creep("creep-b", 20, 20, ME),
        creep("creep-a", 20, 20, "someone"),
        creep("elsewhere", 21, 20, ME),
    ];
    view.world.set_room(&snapshot(&terrain(), objects), 0.0);
    let w1n1 = room("W1N1");
    let point = view
        .camera
        .project(tile_centre(&view, w1n1, 20, 20))
        .unwrap();
    view.pointer_down(7, point, PointerButton::Primary, true);
    view.pointer_move(7, point + Vec2::new(2.0, -2.0), 0.0);
    let pick = view
        .pointer_up(7, point + Vec2::new(2.0, -2.0), 0.0, 0.0)
        .unwrap();
    assert_eq!((pick.room, pick.x, pick.y), (w1n1, 20, 20));
    assert_eq!(
        pick.objects,
        ["creep-a", "creep-b", "energy", "rampart", "spawn", "road"]
    );
    assert_eq!(
        view.pointer_up(7, point, 0.0, 0.0),
        None,
        "released pointers are forgotten"
    );
}

/// Presses and releases pointer `id` at `point` (`primary` as `isPrimary`); the tile picked.
fn click(view: &mut View, id: i32, point: Vec2, primary: bool) -> Option<(RoomCoord, u8, u8)> {
    view.pointer_down(id, point, PointerButton::Primary, primary);
    view.pointer_up(id, point, 0.0, 0.0)
        .map(|pick| (pick.room, pick.x, pick.y))
}

#[test]
fn lost_releases_do_not_turn_later_clicks_into_pinches() {
    let w1n1 = room("W1N1");
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = view(mode);
        let at = |view: &View, x: u8, y: u8| view.tile_point(w1n1, x, y).unwrap();

        // A touch whose release never arrived: every later mouse press is primary.
        view.pointer_down(3, at(&view, 10, 10), PointerButton::Primary, true);
        for (x, y) in [(20, 20), (30, 25), (12, 40)] {
            let point = at(&view, x, y);
            assert_eq!(
                click(&mut view, 1, point, true),
                Some((w1n1, x, y)),
                "{mode:?}"
            );
        }

        // Two pointers lost, then one of them pressed again: a gesture of its own.
        view.pointer_down(1, at(&view, 5, 5), PointerButton::Primary, true);
        view.pointer_down(2, at(&view, 6, 6), PointerButton::Primary, false);
        let point = at(&view, 33, 17);
        assert_eq!(
            click(&mut view, 1, point, false),
            Some((w1n1, 33, 17)),
            "{mode:?}"
        );

        // A cancelled pointer is forgotten without a click.
        view.pointer_down(5, at(&view, 8, 8), PointerButton::Primary, true);
        view.pointer_cancel(5);
        assert_eq!(
            view.pointer_up(5, at(&view, 8, 8), 0.0, 0.0),
            None,
            "{mode:?}"
        );
        let point = at(&view, 40, 9);
        assert_eq!(
            click(&mut view, 6, point, false),
            Some((w1n1, 40, 9)),
            "{mode:?}"
        );
    }
}
