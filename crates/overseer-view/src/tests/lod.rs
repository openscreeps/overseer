//! Levels of detail: zoom that stays continuous across the thresholds, and the rooms that
//! each level asks for (stats, terrain, minimaps and detail).

use glam::Vec2;
use overseer::map::{RoomCoord, RoomRect};

use super::{SIZE, place, room, shard, terrain, world_view};
use crate::camera::{rect_extent, room_centre};
use crate::view::Fades;
use crate::{
    DETAIL_PX_PER_ROOM, Lod, MINIMAP_PX_PER_ROOM, MINIMAP_ROOMS, Scope, TERRAIN_PX_PER_ROOM,
    ViewMode,
};

#[test]
fn zoom_is_continuous_across_the_lod_thresholds() {
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = world_view();
        view.set_mode(mode);
        assert_eq!(view.lod(), Lod::Overview, "the fitted shard");
        assert_eq!(view.fades().terrain, 0.0);
        let anchor = Vec2::new(430.0, 280.0);
        let grabbed = view.camera.ground_point(anchor, 0.0).unwrap();
        let step = (-10.0 * 0.0015_f32).exp();
        let mut lods = vec![view.lod()];
        let (mut size, mut fades) = (view.camera.px_per_room(), view.fades());
        for _ in 0..2000 {
            let half_height = view.camera.half_height();
            view.wheel(anchor, -10.0, 0.0);
            if view.camera.half_height() >= half_height {
                break;
            }
            let now = view.camera.px_per_room();
            if view.camera.half_height() > view.camera.half_height_range().0 {
                assert!(
                    (size / now - step).abs() < 1e-3,
                    "{mode:?}: a wheel step zooms by its factor ({size} → {now})"
                );
            }
            let under = view.camera.ground_point(anchor, 0.0).unwrap();
            assert!(
                grabbed.distance(under) < 0.05,
                "{mode:?}: the anchor drifted to {under} at {now} px/room"
            );
            let next = view.fades();
            for (before, after) in [
                (fades.terrain, next.terrain),
                (fades.minimap, next.minimap),
                (fades.detail, next.detail),
                (fades.borders, next.borders),
            ] {
                assert!(
                    after >= before && after - before < 0.12,
                    "{mode:?}: fade jumps {before} → {after} at {now} px/room"
                );
            }
            if lods.last() != Some(&view.lod()) {
                lods.push(view.lod());
            }
            (size, fades) = (now, next);
        }
        assert_eq!(
            lods,
            [Lod::Overview, Lod::Terrain, Lod::Minimap, Lod::Detail],
            "{mode:?}"
        );
        assert_eq!(fades, Fades::at(size));
        assert_eq!(
            fades.detail, 1.0,
            "{mode:?}: fully detailed at the closest zoom"
        );
    }
    // The thresholds themselves.
    assert_eq!(Lod::of(TERRAIN_PX_PER_ROOM - 0.01), Lod::Overview);
    assert_eq!(Lod::of(TERRAIN_PX_PER_ROOM), Lod::Terrain);
    assert_eq!(Lod::of(MINIMAP_PX_PER_ROOM), Lod::Minimap);
    assert_eq!(Lod::of(DETAIL_PX_PER_ROOM), Lod::Detail);
    let at = Fades::at(DETAIL_PX_PER_ROOM);
    assert_eq!((at.terrain, at.minimap, at.detail), (1.0, 1.0, 0.0));
}

/// Rooms of an interest list are distinct, inside `visible` and nearest to `centre` first.
fn assert_nearest_first(rooms: &[RoomCoord], visible: RoomRect, centre: Vec2) {
    let mut seen = std::collections::HashSet::new();
    let mut distances = Vec::new();
    for room in rooms {
        assert!(visible.contains(*room), "{room} not in view");
        assert!(seen.insert(*room), "{room} twice");
        distances.push(room_centre(*room).distance(centre));
    }
    assert!(
        distances.windows(2).all(|pair| pair[0] <= pair[1] + 1e-3),
        "not nearest first"
    );
}

#[test]
fn interest_follows_the_level_of_detail() {
    let mut view = world_view();
    // Overview: the whole shard's stats and terrain of every room, nearest first.
    assert_eq!(view.lod(), Lod::Overview);
    let interest = view.interest();
    assert_eq!(interest.stats, Some(shard()));
    assert_eq!(interest.terrain.len(), shard().len(), "every room in view");
    assert_nearest_first(&interest.terrain, shard(), view.camera.target());
    assert!(interest.minimaps.is_empty() && interest.detail.is_empty());
    // Rooms with terrain or known absent are still listed (the world map skips them).
    view.world.set_terrain(room("W0N0"), &terrain());
    view.world.mark_no_terrain(room("W1N0"));
    assert_eq!(view.interest().terrain.len(), shard().len());
    // Zoomed out on a corner, only the rooms in view.
    place(&mut view, rect_extent(shard()).0, 8.0);
    assert_eq!(view.lod(), Lod::Overview);
    let interest = view.interest();
    let visible = view.visible_rooms();
    assert!(visible.len() < shard().len());
    assert_eq!(interest.terrain.len(), visible.len());
    assert_eq!(Some(interest.terrain[0]), view.focused_room());
    assert_nearest_first(&interest.terrain, visible, view.camera.target());

    // Terrain: the rooms in view, nearest first.
    let e3s5 = room("E3S5");
    let centre = room_centre(e3s5) + Vec2::new(10.0, -5.0);
    place(&mut view, centre, 50.0);
    assert_eq!(view.lod(), Lod::Terrain);
    let interest = view.interest();
    let visible = view.visible_rooms();
    assert_eq!(interest.stats, Some(visible));
    assert_eq!((visible.width(), visible.height()), (17, 13));
    assert_eq!(interest.terrain.len(), visible.len());
    assert_eq!(interest.terrain[0], e3s5);
    assert_nearest_first(&interest.terrain, visible, centre);
    assert!(interest.minimaps.is_empty() && interest.detail.is_empty());

    // Minimap: also minimaps, at most `MINIMAP_ROOMS`.
    view.resize(1600.0, 1200.0);
    place(&mut view, centre, 105.0);
    assert_eq!(view.lod(), Lod::Minimap);
    let interest = view.interest();
    let visible = view.visible_rooms();
    assert!(
        visible.len() > MINIMAP_ROOMS,
        "{} rooms in view",
        visible.len()
    );
    assert_eq!(interest.minimaps.len(), MINIMAP_ROOMS);
    assert_eq!(interest.minimaps[0], e3s5);
    assert_nearest_first(&interest.minimaps, visible, centre);
    assert_eq!(interest.detail, [] as [RoomCoord; 0]);

    // Detail: the focused room and the nearest other (east of the centre here).
    view.resize(SIZE.x, SIZE.y);
    place(&mut view, room_centre(e3s5) + Vec2::new(15.0, 3.0), 400.0);
    assert_eq!(view.lod(), Lod::Detail);
    assert_eq!(view.focused_room(), Some(e3s5));
    let interest = view.interest();
    assert_eq!(interest.detail, [e3s5, room("E4S5")]);
    assert_eq!(interest.minimaps[..2], [e3s5, room("E4S5")]);
    assert_eq!(interest.terrain.len(), view.visible_rooms().len());

    // At the shard's edge the lists stay inside the bounds.
    place(&mut view, rect_extent(shard()).0, 200.0);
    let interest = view.interest();
    assert!(interest.minimaps.iter().all(|room| shard().contains(*room)));
    assert_eq!(interest.minimaps[0], room("W59N59"));

    // A room scope wants its room, without stats.
    let w1n1 = room("W1N1");
    view.set_scope(Scope::Room(w1n1));
    let interest = view.interest();
    assert_eq!(interest.stats, None);
    assert_eq!(
        (interest.terrain, interest.minimaps, interest.detail),
        (vec![w1n1], vec![w1n1], vec![w1n1])
    );
}
