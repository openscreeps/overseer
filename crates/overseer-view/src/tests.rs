//! Scene, camera, levels of detail, interest, picking, gestures, interpolation and the
//! world's change tracking without a GPU, plus frames drawn through wgpu's no-op backend
//! (pipelines, shaders and buffer validation).

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{Vec2, Vec3};
use overseer::RoomSnapshot;
use overseer::map::{RoomCoord, RoomMap, RoomOwner, RoomRect, RoomStat, RoomStatus};
use overseer::model::world::{GameRoomObject, Terrain};
use serde_json::{Value, json};

use crate::scene::{Scene, Timing};
use crate::text::Atlas;
use crate::view::View;
use crate::world::{Limits, tile_origin};
use crate::{PointerButton, Scope, ViewMode};

mod accent;
mod camera;
mod camera_motion;
mod effects;
mod gestures;
mod kinds;
mod lod;
mod look;
mod motion;
mod objects;
mod overlay;
mod picking;
mod render;
mod terrain_shape;
mod visuals;
mod world;

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
            badge: None,
        }),
        reservation: None,
        safe_mode: false,
        mineral: None,
        mineral_density: None,
        fetched_at: 0,
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
    let mut view = View::new(Limits::WEBGL2);
    view.set_smooth(false);
    view.set_scope(Scope::Room(room("W1N1")));
    view.resize(SIZE.x, SIZE.y);
    view.set_mode(mode);
    view.world.set_room(&snapshot(&terrain(), Vec::new()), 0.0);
    view
}

/// A world scope over the shard, fitted.
fn world_view() -> View {
    let mut view = View::new(Limits::WEBGL2);
    view.set_smooth(false);
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
    view.pointer_down(1, from, button, true);
    for step in 1..=4 {
        #[expect(clippy::cast_precision_loss, reason = "small numbers")]
        view.pointer_move(1, from.lerp(to, step as f32 / 4.0), 0.0);
    }
    assert_eq!(
        view.pointer_up(1, to, 0.0, 0.0),
        None,
        "a drag is not a click"
    );
}

/// A scene showing `snapshot` at `now` (no text rasterizer).
fn show(scene: &mut Scene, snapshot: &RoomSnapshot, now: f64) -> bool {
    scene.set_room(snapshot, Timing::at(now), &mut Atlas::default())
}
