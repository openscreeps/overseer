//! Scene, camera, levels of detail, interest, picking, gestures, interpolation and the
//! world's change tracking without a GPU, plus frames drawn through wgpu's no-op backend
//! (pipelines, shaders and buffer validation).

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{Vec2, Vec3};
use overseer::RoomSnapshot;
use overseer::core::api::Terrain;
use overseer::core::api::types::GameRoomObject;
use overseer::map::{RoomCoord, RoomMap, RoomOwner, RoomRect, RoomStat, RoomStatus};
use serde_json::{Value, json};

use crate::camera::{rect_extent, room_centre};
use crate::gpu::{Draw, Gpu};
use crate::scene::Scene;
use crate::view::{Fades, View};
use crate::world::{World, tile_origin};
use crate::{
    CameraState, DETAIL_PX_PER_ROOM, Lod, MINIMAP_PX_PER_ROOM, MINIMAP_ROOMS, MOVE_DURATION_MS,
    PointerButton, Scope, TERRAIN_PX_PER_ROOM, ViewMode,
};

const ME: &str = "5a0f0e0e0e0e0e0e0e0e0e01";
const SIZE: Vec2 = Vec2::new(800.0, 600.0);

fn room(name: &str) -> RoomCoord {
    name.parse().unwrap()
}

/// A 120×120-room shard: `W59N59` to `E59S59`.
fn shard() -> RoomRect {
    RoomRect::new(room("W59N59"), room("E59S59"))
}

/// Plains with a wall block at `x < 5` and a swamp at `30 ≤ x < 35, 30 ≤ y < 35`.
fn terrain() -> Arc<Terrain> {
    let mut digits = String::with_capacity(2500);
    for y in 0..50 {
        for x in 0..50 {
            digits.push(if x < 5 {
                '1'
            } else if (30..35).contains(&x) && (30..35).contains(&y) {
                '2'
            } else {
                '0'
            });
        }
    }
    Arc::new(Terrain::decode(&digits).unwrap())
}

fn object(value: Value) -> GameRoomObject {
    let object: GameRoomObject = serde_json::from_value(value).unwrap();
    assert!(!matches!(object, GameRoomObject::Other(_)), "{object:?}");
    object
}

fn creep(id: &str, x: u8, y: u8, user: &str) -> GameRoomObject {
    object(json!({
        "_id": id, "type": "creep", "room": "W1N1", "x": x, "y": y, "user": user,
        "name": id, "hits": 100, "hitsMax": 100, "store": {},
        "body": [{"type": "move", "hits": 100}],
    }))
}

fn structure(kind: &str, id: &str, x: u8, y: u8) -> GameRoomObject {
    object(json!({
        "_id": id, "type": kind, "room": "W1N1", "x": x, "y": y, "user": ME,
        "hits": 100, "hitsMax": 100, "nextDecayTime": 1000, "notifyWhenAttacked": true,
        "store": {}, "storeCapacityResource": {},
    }))
}

fn energy(id: &str, x: u8, y: u8) -> GameRoomObject {
    object(json!({
        "_id": id, "type": "energy", "room": "W1N1", "x": x, "y": y, "energy": 50,
    }))
}

fn snapshot_of(name: &str, terrain: &Arc<Terrain>, objects: Vec<GameRoomObject>) -> RoomSnapshot {
    RoomSnapshot {
        room: name.to_owned(),
        shard: Some("shard0".to_owned()),
        game_time: Some(1),
        terrain: Arc::clone(terrain),
        objects,
        undecoded: Vec::new(),
        users: BTreeMap::new(),
        flags: Vec::new(),
        visuals: Vec::new(),
        subscription_error: None,
    }
}

fn snapshot(terrain: &Arc<Terrain>, objects: Vec<GameRoomObject>) -> RoomSnapshot {
    snapshot_of("W1N1", terrain, objects)
}

fn stat(status: RoomStatus, owner: Option<(&str, u8)>) -> RoomStat {
    RoomStat {
        status,
        status_until: None,
        owner: owner.map(|(id, level)| RoomOwner {
            id: id.to_owned(),
            username: None,
            level,
        }),
        reservation: None,
        safe_mode: false,
        mineral: None,
        mineral_density: None,
    }
}

fn minimap(user: &str) -> RoomMap {
    serde_json::from_value(json!({
        "w": [[10, 10]], "r": [[11, 10], [12, 10]], "s": [[20, 20]], "c": [[25, 25]],
        user: [[30, 30], [31, 30]],
    }))
    .unwrap()
}

/// A room scope on `W1N1` showing an empty snapshot.
fn view(mode: ViewMode) -> View {
    let mut view = View::new(4);
    view.set_scope(Scope::Room(room("W1N1")));
    view.resize(SIZE.x, SIZE.y);
    view.set_mode(mode);
    view.world.set_room(&snapshot(&terrain(), Vec::new()), 0.0);
    view
}

/// A world scope over the shard, fitted.
fn world_view(atlas_per_row: u32) -> View {
    let mut view = View::new(atlas_per_row);
    view.set_scope(Scope::World(shard()));
    view.resize(SIZE.x, SIZE.y);
    view
}

/// Places the camera over `centre` (world tiles) with rooms `px_per_room` CSS pixels wide.
fn place(view: &mut View, centre: Vec2, px_per_room: f32) {
    let mut state = view.camera_state();
    state.centre = centre.to_array();
    state.half_height = view.camera.size().y * 50.0 / (2.0 * px_per_room);
    view.set_camera_state(state);
}

/// The world position of a tile's centre on top of its terrain.
fn tile_centre(view: &View, room: RoomCoord, x: u8, y: u8) -> Vec3 {
    let corner = tile_origin(room, x, y);
    let height = view
        .world
        .detail(room)
        .map_or(0.0, |detail| detail.scene.tile_height(x, y));
    Vec3::new(corner.x + 0.5, height, corner.y + 0.5)
}

/// A primary-button drag from `from` to `to` in a few steps.
fn drag(view: &mut View, button: PointerButton, from: Vec2, to: Vec2) {
    view.pointer_down(1, from, button);
    for step in 1..=4 {
        #[expect(clippy::cast_precision_loss, reason = "small numbers")]
        view.pointer_move(1, from.lerp(to, step as f32 / 4.0));
    }
    assert_eq!(view.pointer_up(1, to), None, "a drag is not a click");
}

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
        view.wheel(Vec2::new(300.0, 200.0), -300.0);
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
        view.wheel(anchor, -400.0);
        assert!(span(&view) < wide * 0.7, "{mode:?}: wheel up zooms in");
        let under = view.camera.ground_point(anchor, 0.0).unwrap();
        assert!(
            grabbed.distance(under) < 1e-2,
            "{mode:?}: zoom anchor moved"
        );

        let narrow = span(&view);
        view.pointer_down(1, Vec2::new(380.0, 300.0), PointerButton::Primary);
        view.pointer_down(2, Vec2::new(420.0, 300.0), PointerButton::Primary);
        view.pointer_move(1, Vec2::new(300.0, 300.0));
        view.pointer_move(2, Vec2::new(500.0, 300.0));
        assert!(
            span(&view) < narrow * 0.5,
            "{mode:?}: spreading fingers zooms in"
        );
        assert_eq!(view.pointer_up(1, Vec2::new(300.0, 300.0)), None);
        assert_eq!(
            view.pointer_up(2, Vec2::new(500.0, 300.0)),
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
    view.pointer_down(7, point, PointerButton::Primary);
    view.pointer_move(7, point + Vec2::new(2.0, -2.0));
    let pick = view.pointer_up(7, point + Vec2::new(2.0, -2.0)).unwrap();
    assert_eq!((pick.room, pick.x, pick.y), (w1n1, 20, 20));
    assert_eq!(
        pick.objects,
        ["creep-a", "creep-b", "energy", "rampart", "spawn", "road"]
    );
    assert_eq!(
        view.pointer_up(7, point),
        None,
        "released pointers are forgotten"
    );
}

#[test]
fn moved_objects_glide_and_new_or_removed_objects_snap() {
    let terrain = terrain();
    let mut scene = Scene::default();
    scene.set_room(
        &snapshot(
            &terrain,
            vec![creep("mover", 10, 10, ME), creep("gone", 1, 1, ME)],
        ),
        0.0,
    );
    let start = 1000.0;
    scene.set_room(
        &snapshot(
            &terrain,
            vec![creep("mover", 11, 9, ME), creep("new", 30, 30, ME)],
        ),
        start,
    );
    let at = |scene: &mut Scene, time: f64| {
        let frame = scene.frame(time);
        let offsets: Vec<[f32; 3]> = scene.instances().iter().map(|i| i.offset).collect();
        (offsets, frame.animating)
    };
    // Instances follow snapshot order; the removed object is gone, the new one in place.
    let (offsets, animating) = at(&mut scene, start);
    assert_eq!(offsets, [[10.5, 0.0, 10.5], [30.5, 0.0, 30.5]]);
    assert!(animating);
    let (offsets, animating) = at(&mut scene, start + MOVE_DURATION_MS / 2.0);
    assert_eq!(
        offsets[0],
        [11.0, 0.0, 10.0],
        "halfway (smoothstep is symmetric)"
    );
    assert!(animating);
    let (offsets, _) = at(&mut scene, start + MOVE_DURATION_MS / 4.0);
    assert!(
        offsets[0][0] > 10.5 && offsets[0][0] < 10.75,
        "eased in: {offsets:?}"
    );
    let (offsets, animating) = at(&mut scene, start + MOVE_DURATION_MS);
    assert_eq!(offsets[0], [11.5, 0.0, 9.5]);
    assert!(!animating, "done after the duration");

    // A move during a move starts from where the object is drawn.
    let restart = start + MOVE_DURATION_MS + 100.0;
    scene.set_room(
        &snapshot(&terrain, vec![creep("mover", 12, 9, ME)]),
        restart,
    );
    scene.set_room(
        &snapshot(&terrain, vec![creep("mover", 13, 9, ME)]),
        restart + MOVE_DURATION_MS / 2.0,
    );
    let (offsets, animating) = at(&mut scene, restart + MOVE_DURATION_MS / 2.0);
    assert_eq!(offsets, [[12.0, 0.0, 9.5]]);
    assert!(animating);
    // An unchanged snapshot keeps the movement going.
    scene.set_room(
        &snapshot(&terrain, vec![creep("mover", 13, 9, ME)]),
        restart + MOVE_DURATION_MS * 0.75,
    );
    let (offsets, animating) = at(&mut scene, restart + MOVE_DURATION_MS * 1.5);
    assert_eq!(offsets, [[13.5, 0.0, 9.5]]);
    assert!(!animating);
}

#[test]
fn another_room_snaps_every_object() {
    let terrain = terrain();
    let mut scene = Scene::default();
    scene.set_room(&snapshot(&terrain, vec![creep("c", 10, 10, ME)]), 0.0);
    let other = snapshot_of("W2N1", &terrain, vec![creep("c", 40, 40, ME)]);
    scene.set_room(&other, 100.0);
    let frame = scene.frame(100.0);
    assert!(!frame.animating);
    assert_eq!(scene.instances()[0].offset, [40.5, 0.0, 40.5]);
}

#[test]
fn terrain_is_rebuilt_only_when_it_changes() {
    let terrain = terrain();
    let mut scene = Scene::default();
    assert_eq!(scene.terrain().1, 0);
    assert!(scene.set_room(&snapshot(&terrain, Vec::new()), 0.0));
    let (mesh, version) = scene.terrain();
    assert_eq!(version, 1);
    // 2500 tops; wall sides: 50 facing the plains, 50 + 2 × 5 at the room edge.
    assert_eq!(mesh.indices.len(), (2500 + 50 + 50 + 10) * 6);
    assert!(!scene.set_room(&snapshot(&terrain, vec![creep("c", 9, 9, ME)]), 1.0));
    let equal = Arc::new((*terrain).clone());
    assert!(
        !scene.set_room(&snapshot(&equal, Vec::new()), 2.0),
        "equal terrain"
    );
    assert_eq!(scene.terrain().1, 1);
    let plains = Arc::new(Terrain::decode(&"0".repeat(2500)).unwrap());
    assert!(scene.set_room(&snapshot(&plains, Vec::new()), 3.0));
    assert_eq!(scene.terrain().1, 2);
    assert_eq!(scene.terrain().0.indices.len(), 2500 * 6);
}

#[test]
fn objects_stand_on_raised_walls_and_take_owner_colours() {
    let terrain = terrain();
    let mut scene = Scene::default();
    let objects = vec![
        structure("road", "tunnel", 2, 2),
        creep("mine", 10, 10, ME),
        creep("theirs", 11, 10, "other"),
        creep("invader", 12, 10, "2"),
        creep("keeper", 13, 10, "3"),
    ];
    scene.set_room(&snapshot(&terrain, objects), 0.0);
    scene.frame(0.0);
    let instances = scene.instances().to_vec();
    assert_eq!(
        instances[0].offset[1],
        crate::mesh::WALL_HEIGHT,
        "road on a wall"
    );
    let colors: Vec<[f32; 4]> = instances[1..].iter().map(|i| i.color).collect();
    assert_eq!(colors[0], colors[1], "no user signed in: all players alike");
    assert_ne!(colors[1], colors[2]);
    assert_ne!(colors[2], colors[3]);
    scene.set_user(Some(ME.to_owned()));
    scene.frame(0.0);
    let colors: Vec<[f32; 4]> = scene.instances()[1..].iter().map(|i| i.color).collect();
    assert_ne!(
        colors[0], colors[1],
        "own creep differs from another player's"
    );
}

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
    let mut world = world_view(4);
    world.select(Some((w1n1, 3, 3)));
    let ring = world.selection().unwrap();
    let pixels = ring.scale[0] / world.camera.tiles_per_px();
    assert!((pixels - 14.0).abs() < 0.1, "{pixels} px wide at overview");
    world.select(Some((room("W70N1"), 3, 3)));
    assert_eq!(world.selection(), None, "outside the scope");
}

#[test]
fn zoom_is_continuous_across_the_lod_thresholds() {
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = world_view(4);
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
            view.wheel(anchor, -10.0);
            if view.camera.half_height() >= half_height {
                break;
            }
            let now = view.camera.px_per_room();
            if view.camera.half_height() > 2.0 {
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
    let mut distances = Vec::new();
    for (index, room) in rooms.iter().enumerate() {
        assert!(visible.contains(*room), "{room} not in view");
        assert!(!rooms[..index].contains(room), "{room} twice");
        distances.push(room_centre(*room).distance(centre));
    }
    assert!(
        distances.windows(2).all(|pair| pair[0] <= pair[1] + 1e-3),
        "{distances:?}"
    );
}

#[test]
fn interest_follows_the_level_of_detail() {
    let mut view = world_view(4);
    let atlas = view.world.atlas.capacity();
    assert_eq!(atlas, 16);
    // Overview: the whole shard's stats, nothing else.
    let interest = view.interest();
    assert_eq!(interest.stats, Some(shard()));
    assert!(
        interest.terrain.is_empty() && interest.minimaps.is_empty() && interest.detail.is_empty()
    );

    // Terrain: the rooms in view, nearest first, at most an atlas of them.
    let e3s5 = room("E3S5");
    let centre = room_centre(e3s5) + Vec2::new(10.0, -5.0);
    place(&mut view, centre, 50.0);
    assert_eq!(view.lod(), Lod::Terrain);
    let interest = view.interest();
    let visible = view.visible_rooms();
    assert_eq!(interest.stats, Some(visible));
    assert_eq!((visible.width(), visible.height()), (17, 13));
    assert_eq!(interest.terrain.len(), atlas);
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
    assert!(interest.terrain.len() <= atlas);

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

#[test]
fn picking_crosses_room_borders_in_both_modes() {
    let (west, east) = (room("W0S0"), room("E0S0"));
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = world_view(4);
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

#[test]
fn a_room_scope_keeps_the_camera_over_its_room() {
    let w5n5 = room("W5N5");
    let mut view = View::new(4);
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
    let mut view = world_view(4);
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
    let mut other = world_view(4);
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
    let mut view = world_view(4);
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
fn updates_touch_only_the_rooms_that_changed() {
    let bounds = RoomRect::new(room("W4N4"), room("E4S4"));
    let mut world = World::new(bounds, 4);
    world.set_focus(bounds, room_centre(room("W0N0")));
    // What a fresh world must upload in full.
    assert_eq!(world.cells.dirty.marks(), (true, Vec::new()));
    assert_eq!(world.atlas.dirty.marks(), (true, Vec::new()));
    world.cells.dirty.drain(100, |_| {});
    world.atlas.dirty.drain(16, |_| {});

    // An overview: one cell.
    let e1s2 = room("E1S2");
    world.set_stat(e1s2, Some(&stat(RoomStatus::Normal, Some(("other", 3)))));
    let index = (2 + 5) * 10 + (1 + 5);
    assert_eq!(world.cells.dirty.marks(), (false, vec![index]));
    assert_ne!(
        world.cells.instances()[index as usize].color,
        world.cells.instances()[0].color
    );
    world.set_stat(e1s2, Some(&stat(RoomStatus::Normal, Some(("other", 3)))));
    assert_eq!(
        world.cells.dirty.marks().1.len(),
        1,
        "unchanged: no new mark"
    );
    world.cells.dirty.drain(100, |_| {});
    assert_eq!(world.cells.dirty.marks(), (false, Vec::new()));

    // A terrain: one atlas slot (texels and instance), tinted by the known overview.
    world.set_terrain(e1s2, &terrain());
    let slot = world.atlas.slot(e1s2).unwrap();
    assert_eq!(world.atlas.uploads, [slot]);
    assert_eq!(world.atlas.dirty.marks(), (false, vec![slot]));
    assert_ne!(world.atlas.instances()[slot as usize].tint[3], 0);
    world.atlas.uploads.clear();
    world.atlas.dirty.drain(16, |_| {});
    world.set_terrain(e1s2, &terrain());
    assert!(world.atlas.uploads.is_empty(), "the same terrain again");
    // A new overview of a room with terrain: its cell and its atlas tint.
    world.set_stat(e1s2, Some(&stat(RoomStatus::Novice, None)));
    assert_eq!(world.cells.dirty.marks(), (false, vec![index]));
    assert_eq!(world.atlas.dirty.marks(), (false, vec![slot]));
    assert_eq!(world.atlas.uploads, [] as [u32; 0]);

    // A minimap: its slot only.
    world.set_minimap(e1s2, Some(&minimap("other")));
    world.set_minimap(room("W2S0"), Some(&minimap(ME)));
    assert_eq!(world.minimaps.dirty.marks(), (false, vec![0, 1]));
    world.minimaps.dirty.drain(MINIMAP_ROOMS, |_| {});
    world.set_minimap(room("W2S0"), Some(&minimap("another")));
    assert_eq!(world.minimaps.dirty.marks(), (false, vec![1]));
    assert_eq!(world.minimaps.points(1).len(), 7);
    // Signing in recolours (everything, once).
    world.set_user(Some(ME.to_owned()));
    assert!(world.cells.dirty.marks().0);
}

#[test]
fn dirty_marks_drain_as_merged_runs() {
    let mut dirty = crate::world::Dirty::default();
    for index in [7, 3, 4, 9, 4, 5, 12] {
        dirty.mark(index);
    }
    let mut runs = Vec::new();
    dirty.drain(10, |run| runs.push(run));
    assert_eq!(runs, [3..6, 7..8, 9..10], "12 is out of range");
    dirty.mark_all();
    dirty.mark(2);
    runs.clear();
    dirty.drain(4, |run| runs.push(run));
    assert_eq!(runs, [std::ops::Range { start: 0, end: 4 }]);
    runs.clear();
    dirty.drain(4, |run| runs.push(run));
    assert_eq!(runs, [] as [std::ops::Range<usize>; 0]);
}

#[test]
fn the_atlas_keeps_the_rooms_nearest_to_the_camera() {
    let bounds = RoomRect::new(room("E0S0"), room("E9S9"));
    let mut world = World::new(bounds, 2);
    let near = RoomRect::new(room("E0S0"), room("E1S1"));
    world.set_focus(near, room_centre(room("E0S0")));
    let rooms = ["E0S0", "E1S0", "E0S1", "E1S1", "E5S5", "E9S9"].map(room);
    for room in rooms {
        world.set_terrain(room, &terrain());
    }
    for room in &rooms[..4] {
        assert!(world.atlas.slot(*room).is_some(), "{room} near");
    }
    assert_eq!(
        world.atlas.slot(rooms[4]),
        None,
        "the atlas is full of nearer rooms"
    );
    // Rooms outside the bounds are stored but never placed.
    world.set_terrain(room("W1N1"), &terrain());
    assert!(world.atlas.has_terrain(room("W1N1")));
    assert_eq!(world.atlas.slot(room("W1N1")), None);

    // Looking elsewhere brings the stored terrain back, replacing the farthest rooms.
    world.atlas.uploads.clear();
    world.set_focus(
        RoomRect::new(room("E5S5"), room("E9S9")),
        room_centre(room("E9S9")),
    );
    assert!(!world.refresh(), "everything placed");
    assert!(world.atlas.slot(rooms[5]).is_some());
    assert!(world.atlas.slot(rooms[4]).is_some());
    assert_eq!(
        world.atlas.slot(rooms[0]),
        None,
        "the farthest room gave its slot"
    );
    assert_eq!(world.atlas.uploads.len(), 2);
    assert_eq!(world.focus().centre, room_centre(room("E9S9")));
}

#[test]
fn detail_scenes_fade_in_and_are_replaced_farthest_first() {
    let mut world = World::new(RoomRect::new(room("E0S0"), room("E9S9")), 2);
    world.set_focus(world.bounds(), room_centre(room("E0S0")));
    let terrain = terrain();
    for (index, name) in ["E0S0", "E1S0", "E2S0", "E9S9"].into_iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "small numbers")]
        world.set_room(&snapshot_of(name, &terrain, Vec::new()), index as f64);
    }
    let (_, animating) = world.frame(10.0);
    assert!(animating, "fading in");
    assert!(world.detail(room("E0S0")).unwrap().arrival(310.0) >= 1.0);
    let (_, animating) = world.frame(400.0);
    assert!(!animating);
    world.set_room(&snapshot_of("E0S1", &terrain, Vec::new()), 500.0);
    assert!(
        world.detail(room("E9S9")).is_none(),
        "the farthest scene was replaced"
    );
    assert!(world.detail(room("E0S1")).is_some());
    world.clear_room(room("E0S1"));
    assert!(world.detail(room("E0S1")).is_none());
    assert!(!world.set_room(&snapshot_of("sim", &terrain, Vec::new()), 600.0));
    assert!(
        world.atlas.has_terrain(room("E2S0")),
        "snapshots bring terrain"
    );
}

/// Runs a future that completes without waiting (the no-op backend's).
fn ready<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
}

#[test]
fn frames_render_through_the_noop_backend() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::NOOP,
        backend_options: wgpu::BackendOptions {
            noop: wgpu::NoopBackendOptions {
                enable: true,
                ..wgpu::NoopBackendOptions::default()
            },
            ..wgpu::BackendOptions::default()
        },
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = ready(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = ready(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_webgl2_defaults(),
        ..wgpu::DeviceDescriptor::default()
    }))
    .unwrap();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let output = device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 320,
                height: 200,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    let per_row = crate::atlas_per_row(device.limits().max_texture_dimension_2d);
    assert_eq!(per_row, 40, "2048 texels: 40 × 40 rooms");
    for samples in [1, 4] {
        let mut gpu = Gpu::new(device.clone(), queue.clone(), format, samples, per_row);
        gpu.resize(320, 200);
        let mut view = View::new(per_row);
        view.set_scope(Scope::World(shard()));
        view.resize(320.0, 200.0);
        view.world.set_user(Some(ME.to_owned()));
        let objects = (0..300_u16)
            .map(|index| {
                let (x, y) = (
                    u8::try_from(index % 40).unwrap() + 5,
                    u8::try_from(index / 40).unwrap(),
                );
                match index % 4 {
                    0 => creep(&format!("c{index}"), x, y, ME),
                    1 => structure("road", &format!("r{index}"), x, y),
                    2 => structure("rampart", &format!("p{index}"), x, y),
                    _ => energy(&format!("e{index}"), x, y),
                }
            })
            .collect::<Vec<_>>();
        let terrain = terrain();
        for name in ["W1N1", "W0N1", "W1N0", "W0N0"] {
            let room = room(name);
            view.world.set_terrain(room, &terrain);
            view.world
                .set_stat(room, Some(&stat(RoomStatus::Normal, Some((ME, 8)))));
            view.world.set_minimap(room, Some(&minimap(ME)));
        }
        view.world
            .set_room(&snapshot_of("W1N1", &terrain, objects.clone()), 0.0);
        view.world
            .set_room(&snapshot_of("W0N1", &terrain, objects), 0.0);
        view.select(Some((room("W1N1"), 7, 7)));
        for (mode, px_per_room) in [
            (ViewMode::Perspective, 2.0),
            (ViewMode::TopDown, 2.0),
            (ViewMode::TopDown, 120.0),
            (ViewMode::Perspective, 500.0),
            (ViewMode::TopDown, 500.0),
        ] {
            view.set_mode(mode);
            place(&mut view, Vec2::new(-50.0, -50.0), px_per_room);
            view.world.refresh();
            view.world.frame(1000.0);
            gpu.sync(&mut view.world);
            gpu.draw(&output, &Draw::of(&view, 1000.0), &view.world);
        }
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
}
