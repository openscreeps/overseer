//! Camera smoothing: pan inertia after a released drag and the eased wheel zoom.

use glam::Vec2;

use super::{place, view, world_view};
use crate::view::View;
use crate::{MAX_PX_PER_TILE, PointerButton, ViewMode};

/// A world view zoomed in to rooms of 300 pixels, smoothing on.
fn smooth_view() -> View {
    let mut view = world_view();
    place(&mut view, Vec2::ZERO, 300.0);
    view.set_smooth(true);
    view
}

/// A drag of `distance` CSS pixels to the east over `ms` milliseconds, released at
/// `release`.
fn fling(view: &mut View, distance: f32, ms: f64, release: f64) {
    let from = Vec2::new(300.0, 300.0);
    view.pointer_down(1, from, PointerButton::Primary, true);
    for step in 1..=6_u8 {
        let share = f32::from(step) / 6.0;
        view.pointer_move(
            1,
            from + Vec2::new(distance * share, 0.0),
            ms * f64::from(share),
        );
    }
    assert_eq!(
        view.pointer_up(1, from + Vec2::new(distance, 0.0), release),
        None
    );
}

/// Advances until the camera rests; returns the steps taken.
fn settle(view: &mut View, from: f64) -> usize {
    let mut steps = 0;
    let mut now = from;
    while view.advance(now) {
        now += 16.0;
        steps += 1;
        assert!(steps < 1000, "never settles");
    }
    steps
}

#[test]
fn a_released_drag_glides_on_and_slows_down() {
    let mut view = smooth_view();
    fling(&mut view, 120.0, 60.0, 64.0);
    let released = view.camera.target();
    // Dragging east moves the view west (the ground follows the pointer).
    let mut last = released;
    let mut previous_step = f32::INFINITY;
    let mut now = 64.0;
    for _ in 0..10 {
        now += 16.0;
        assert!(view.advance(now));
        let target = view.camera.target();
        let step = last.x - target.x;
        assert!(
            step > 0.0 && step < previous_step,
            "slowing: {step} after {previous_step}"
        );
        previous_step = step;
        last = target;
    }
    assert!(settle(&mut view, now) > 0);
    // About velocity × decay time further than released: 2 px/ms × 325 ms.
    let tiles = (released.x - view.camera.target().x) / view.camera.tiles_per_px();
    assert!((450.0..800.0).contains(&tiles), "{tiles} px");
}

#[test]
fn slow_or_stopped_drags_and_new_presses_do_not_glide() {
    let mut view = smooth_view();
    fling(&mut view, 120.0, 60.0, 300.0);
    assert!(!view.advance(316.0), "stopped before the release");
    let mut view = smooth_view();
    fling(&mut view, 10.0, 600.0, 600.0);
    assert!(!view.advance(616.0), "too slow");
    let mut view = smooth_view();
    fling(&mut view, 120.0, 60.0, 64.0);
    view.pointer_down(2, Vec2::new(10.0, 10.0), PointerButton::Primary, true);
    assert!(!view.advance(80.0), "a press catches the view");
    let mut view = world_view();
    place(&mut view, Vec2::ZERO, 300.0);
    fling(&mut view, 120.0, 60.0, 64.0);
    assert!(!view.advance(80.0), "without smoothing");
}

#[test]
fn the_wheel_zoom_eases_in_around_the_pointer() {
    let mut view = view(ViewMode::TopDown);
    view.set_smooth(true);
    let anchor = Vec2::new(200.0, 150.0);
    let grabbed = view.camera.ground_point(anchor, 0.0).unwrap();
    let before = view.camera.half_height();
    view.wheel(anchor, -400.0, 0.0);
    assert_eq!(
        view.camera.half_height(),
        before,
        "nothing until the next frame"
    );
    assert!(view.advance(35.0));
    let partial = view.camera.half_height();
    let target = before * (-400.0 * 0.0015_f32).exp();
    assert!(
        partial < before && partial > target,
        "{partial} on the way to {target}"
    );
    // A second notch adds to the zoom still to come.
    view.wheel(anchor, -100.0, 40.0);
    settle(&mut view, 50.0);
    let expected = before * (-500.0 * 0.0015_f32).exp();
    assert!(
        (view.camera.half_height() - expected).abs() < 1e-3,
        "{}",
        view.camera.half_height()
    );
    let under = view.camera.ground_point(anchor, 0.0).unwrap();
    assert!(
        grabbed.distance(under) < 1e-2,
        "the anchor stays under the pointer"
    );
}

#[test]
fn zoom_stops_at_readable_tiles_and_eases_into_the_limit() {
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = smooth_view();
        view.set_mode(mode);
        let px_per_tile = |view: &View| view.camera.px_per_room() / 50.0;
        let (closest, _) = view.camera.half_height_range();
        assert_eq!(
            closest,
            view.camera.size().y / (2.0 * MAX_PX_PER_TILE),
            "{mode:?}: the cap follows the canvas"
        );
        // Buttons and an instant wheel stop at the cap.
        view.zoom_by(1000.0);
        assert!(
            (px_per_tile(&view) - MAX_PX_PER_TILE).abs() < 1e-3,
            "{mode:?}"
        );
        // Scrolling on at the limit queues nothing: scrolling back acts on the next frame.
        let anchor = Vec2::new(200.0, 150.0);
        for notch in 0..20_u8 {
            view.wheel(anchor, -400.0, f64::from(notch));
        }
        settle(&mut view, 30.0);
        assert!(
            (px_per_tile(&view) - MAX_PX_PER_TILE).abs() < 1e-3,
            "{mode:?}"
        );
        view.wheel(anchor, 100.0, 1000.0);
        assert!(view.advance(1016.0));
        assert!(
            px_per_tile(&view) < MAX_PX_PER_TILE - 1.0,
            "{mode:?}: zooms out at once"
        );
        // From further out, a big scroll eases into the cap: every frame nearer, no jump
        // past it and no stop short of it.
        view.zoom_by(0.1);
        view.wheel(anchor, -4000.0, 2000.0);
        let mut now = 2000.0;
        let mut last = px_per_tile(&view);
        while view.advance(now) {
            let px = px_per_tile(&view);
            assert!(
                px >= last && px <= MAX_PX_PER_TILE + 1e-3,
                "{mode:?}: {last} → {px}"
            );
            last = px;
            now += 16.0;
            assert!(now < 4000.0, "never settles");
        }
        assert!((last - MAX_PX_PER_TILE).abs() < 1e-2, "{mode:?}: {last}");
    }
}
