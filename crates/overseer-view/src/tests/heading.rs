//! What tokens carry besides their kind: the heading of creeps (where they went or what they
//! act on) and of towers (what they shot at), and the owner's badge on creeps.

use glam::Vec2;
use serde_json::json;

use super::{ME, creep, object, room, show, snapshot, snapshot_of, structure, terrain, world_view};
use crate::mesh::Token;
use crate::scene::motion::TURN_MS;
use crate::scene::{Eye, Features, Scene};

/// The token drawn on tile `(x, y)` of the latest frame.
fn token_at(tokens: &[Token], x: u8, y: u8) -> Token {
    let centre = Vec2::new(f32::from(x), f32::from(y)) + 0.5;
    *tokens
        .iter()
        .find(|token| Vec2::new(token.position[0], token.position[2]).distance(centre) < 1e-5)
        .unwrap_or_else(|| panic!("no token on {x},{y}"))
}

/// A tower of the signed-in user on `(x, y)` that attacked tile `target` (if any) last tick.
fn tower(x: u8, y: u8, target: Option<(u8, u8)>) -> overseer::model::world::GameRoomObject {
    let log = target.map_or_else(|| json!({}), |(x, y)| json!({"attack": {"x": x, "y": y}}));
    object(json!({
        "_id": "tower", "type": "tower", "room": "W1N1", "x": x, "y": y, "user": ME,
        "hits": 3000, "hitsMax": 3000, "store": {}, "storeCapacityResource": {"energy": 1000},
        "notifyWhenAttacked": true, "actionLog": log,
    }))
}

/// A creep of the signed-in user on `(x, y)` that harvested tile `target` last tick.
fn harvester(x: u8, y: u8, target: (u8, u8)) -> overseer::model::world::GameRoomObject {
    object(json!({
        "_id": "creep", "type": "creep", "room": "W1N1", "x": x, "y": y, "user": ME,
        "name": "creep", "hits": 100, "hitsMax": 100, "store": {},
        "body": [{"type": "work", "hits": 100}, {"type": "move", "hits": 100}],
        "actionLog": {"harvest": {"x": target.0, "y": target.1}},
    }))
}

/// Headings in 256ths of a turn clockwise from north.
const EAST: u8 = 64;
const SOUTH: u8 = 128;
const WEST: u8 = 192;

#[test]
fn creeps_face_where_they_go_or_what_they_work_on_and_towers_their_target() {
    let terrain = terrain();
    let mut scene = Scene::default();
    // No action effects: `animating` then tells whether a turn goes on.
    scene.set_features(Features {
        effects: false,
        ..Features::default()
    });
    let heading = |scene: &mut Scene, now: f64, (x, y): (u8, u8)| {
        scene.frame(now, &Eye::default());
        token_at(scene.tokens(), x, y).gauges[3]
    };
    show(
        &mut scene,
        &snapshot(
            &terrain,
            vec![creep("creep", 10, 10, ME), tower(20, 20, None)],
        ),
        0.0,
    );
    assert_eq!(
        heading(&mut scene, 0.0, (10, 10)),
        SOUTH,
        "first seen: south"
    );
    assert_eq!(heading(&mut scene, 0.0, (20, 20)), SOUTH);

    // The creep goes east, the tower shoots west: both turn the short way.
    let start = 1000.0;
    show(
        &mut scene,
        &snapshot(
            &terrain,
            vec![creep("creep", 11, 10, ME), tower(20, 20, Some((15, 20)))],
        ),
        start,
    );
    let turning = heading(&mut scene, start + TURN_MS / 2.0, (20, 20));
    assert!(SOUTH < turning && turning < WEST, "turning: {turning}");
    let done = start + 10_000.0;
    assert_eq!(heading(&mut scene, done, (11, 10)), EAST);
    assert_eq!(heading(&mut scene, done, (20, 20)), WEST);
    assert!(!scene.frame(done, &Eye::default()).animating);

    // Standing still, the creep faces what it works on; the idle tower keeps its aim, and an
    // unchanged target starts no turn.
    let later = 20_000.0;
    show(
        &mut scene,
        &snapshot(
            &terrain,
            vec![harvester(11, 10, (11, 11)), tower(20, 20, None)],
        ),
        later,
    );
    show(
        &mut scene,
        &snapshot(
            &terrain,
            vec![harvester(11, 10, (11, 11)), tower(20, 20, None)],
        ),
        later + 5000.0,
    );
    assert!(!scene.frame(later + 5000.0, &Eye::default()).animating);
    assert_eq!(heading(&mut scene, later + 5000.0, (11, 10)), SOUTH);
    assert_eq!(heading(&mut scene, later + 5000.0, (20, 20)), WEST);
}

#[test]
fn creeps_carry_their_owners_badge_once_it_is_there() {
    const FOE: &str = "f0e0f0e0f0e0f0e0f0e0f0e0";
    let mut view = world_view();
    let w1n1 = room("W1N1");
    let objects = vec![
        creep("mine", 10, 10, ME),
        creep("theirs", 12, 10, FOE),
        structure("spawn", "spawn", 14, 10),
    ];
    let badges = |view: &mut crate::view::View| {
        view.world.frame(1000.0, &view.eye());
        let tokens = view.world.detail(w1n1).unwrap().scene.tokens();
        [(10, 10), (12, 10), (14, 10)].map(|(x, y)| token_at(tokens, x, y).badge)
    };
    let badge = [200; 64 * 64 * 4];
    // A badge known before the room arrives.
    view.world.set_badge(FOE, &badge, 64);
    view.world
        .set_room(&snapshot_of("W1N1", &terrain(), objects.clone()), 0.0);
    let theirs = view.world.overlay.badges.index(FOE).unwrap() + 1;
    assert_eq!(
        badges(&mut view),
        [0, theirs, 0],
        "none yet for the user's creeps"
    );
    // One that arrives while the room is shown; structures never carry one.
    view.world.set_badge(ME, &badge, 64);
    let mine = view.world.overlay.badges.index(ME).unwrap() + 1;
    assert_ne!(mine, theirs);
    assert_eq!(badges(&mut view), [mine, theirs, 0]);
    // The next snapshot keeps them.
    view.world
        .set_room(&snapshot_of("W1N1", &terrain(), objects), 500.0);
    assert_eq!(badges(&mut view), [mine, theirs, 0]);
}
