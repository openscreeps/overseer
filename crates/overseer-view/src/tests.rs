//! Scene, camera, picking, gestures and interpolation without a GPU, plus a frame drawn
//! through wgpu's no-op backend (pipelines, shader and buffer validation).

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::Vec2;
use overseer::RoomSnapshot;
use overseer::core::api::Terrain;
use overseer::core::api::types::GameRoomObject;
use serde_json::{Value, json};

use crate::gpu::Gpu;
use crate::scene::Scene;
use crate::view::View;
use crate::{MOVE_DURATION_MS, PointerButton, ViewMode};

const ME: &str = "5a0f0e0e0e0e0e0e0e0e0e01";

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

fn snapshot(terrain: &Arc<Terrain>, objects: Vec<GameRoomObject>) -> RoomSnapshot {
    RoomSnapshot {
        room: "W1N1".to_owned(),
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

fn view(mode: ViewMode) -> View {
    let mut view = View::default();
    view.resize(800.0, 600.0);
    view.set_mode(mode);
    view.scene.set_room(&snapshot(&terrain(), Vec::new()), 0.0);
    view
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
fn assert_round_trips(view: &View) {
    let mut checked = 0;
    for (x, y) in [
        (25, 25),
        (2, 20),
        (32, 32),
        (10, 40),
        (40, 8),
        (12, 12),
        (47, 45),
    ] {
        let world = view.scene.tile_centre(x, y);
        let Some(point) = view.camera.project(world) else {
            continue;
        };
        if !(0.0..800.0).contains(&point.x) || !(0.0..600.0).contains(&point.y) {
            continue;
        }
        let (origin, direction) = view.camera.ray(point);
        let distance = (world - origin).cross(direction).length();
        assert!(distance < 1e-2, "ray misses ({x}, {y}) by {distance}");
        let pick = view.pick(point);
        let picked = pick.as_ref().map(|pick| (pick.x, pick.y));
        assert_eq!(
            picked,
            Some((x, y)),
            "{:?} at {point}, ray {origin} {direction}",
            view.camera.mode
        );
        checked += 1;
    }
    assert!(checked >= 3, "only {checked} probe tiles on screen");
}

#[test]
fn projection_and_picking_round_trip_in_both_modes() {
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = view(mode);
        assert_round_trips(&view);
        drag(
            &mut view,
            PointerButton::Secondary,
            Vec2::new(400.0, 300.0),
            Vec2::new(470.0, 260.0),
        );
        assert_round_trips(&view);
        view.wheel(Vec2::new(300.0, 200.0), -300.0);
        assert_round_trips(&view);
        if mode == ViewMode::Perspective {
            drag(
                &mut view,
                PointerButton::Primary,
                Vec2::new(400.0, 300.0),
                Vec2::new(250.0, 340.0),
            );
            assert_round_trips(&view);
        }
    }
}

#[test]
fn top_down_picks_the_tile_under_the_pointer() {
    let view = view(ViewMode::TopDown);
    // The fitted room is 52 tiles tall on 600 pixels, centred.
    let scale = 600.0 / 52.0;
    let point = |x: f32, y: f32| Vec2::new(400.0 + (x - 25.0) * scale, 300.0 + (y - 25.0) * scale);
    let pick = view.pick(point(10.5, 20.5)).unwrap();
    assert_eq!((pick.x, pick.y), (10, 20));
    let pick = view.pick(point(49.9, 0.1)).unwrap();
    assert_eq!((pick.x, pick.y), (49, 0));
    assert_eq!(view.pick(point(50.2, 25.0)), None, "outside the room");
    assert_eq!(view.pick(point(25.0, -0.2)), None, "outside the room");
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
    let centre = Vec2::new(400.0, 300.0);
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
fn a_click_returns_the_tile_and_its_objects_topmost_first() {
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
    view.scene.set_room(&snapshot(&terrain(), objects), 0.0);
    let point = view.camera.project(view.scene.tile_centre(20, 20)).unwrap();
    view.pointer_down(7, point, PointerButton::Primary);
    view.pointer_move(7, point + Vec2::new(2.0, -2.0));
    let pick = view.pointer_up(7, point + Vec2::new(2.0, -2.0)).unwrap();
    assert_eq!((pick.x, pick.y), (20, 20));
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
    let mut other = snapshot(&terrain, vec![creep("c", 40, 40, ME)]);
    other.room = "W2N1".to_owned();
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
fn selection_is_an_extra_ring_instance() {
    let mut scene = Scene::default();
    scene.set_room(&snapshot(&terrain(), vec![creep("c", 3, 3, ME)]), 0.0);
    let frame = scene.frame(0.0);
    assert_eq!(frame.selection, None);
    assert_eq!(frame.ranges, [0..0, 0..1, 1..1]);
    scene.select(Some((3, 3)));
    let frame = scene.frame(0.0);
    assert_eq!(frame.selection, Some(1));
    let ring = scene.instances()[1];
    assert_eq!(ring.offset, [3.5, crate::mesh::WALL_HEIGHT, 3.5]);
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
    for samples in [1, 4] {
        let mut gpu = Gpu::new(device.clone(), queue.clone(), format, samples);
        gpu.resize(320, 200);
        let mut view = view(ViewMode::Perspective);
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
            .collect();
        view.scene.set_room(&snapshot(&terrain(), objects), 0.0);
        view.scene.select(Some((7, 7)));
        gpu.sync(&view.scene);
        for mode in [ViewMode::Perspective, ViewMode::TopDown] {
            view.set_mode(mode);
            let frame = view.scene.frame(0.0);
            gpu.draw(&output, view.camera.view_projection(), &view.scene, &frame);
        }
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
}
