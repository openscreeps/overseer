//! Objects of a detail scene: glides, fades and spawning creeps, the wall mesh, heights on
//! raised walls, owner colours and the far-to-near draw order of tokens.

use std::sync::Arc;

use glam::{Vec2, Vec3};
use overseer::model::world::Terrain;
use serde_json::json;

use super::{ME, creep, object, show, snapshot, snapshot_of, structure, terrain};
use crate::MOVE_DURATION_MS;
use crate::scene::{Eye, Scene};

#[test]
fn moved_objects_glide_new_ones_fade_in_and_removed_ones_fade_out() {
    let terrain = terrain();
    let mut scene = Scene::default();
    show(
        &mut scene,
        &snapshot(
            &terrain,
            vec![creep("mover", 10, 10, ME), creep("gone", 1, 1, ME)],
        ),
        0.0,
    );
    assert_eq!(
        scene.drawn("mover", 0.0).unwrap().3,
        1.0,
        "the first snapshot snaps"
    );
    let start = 1000.0;
    show(
        &mut scene,
        &snapshot(
            &terrain,
            vec![creep("mover", 11, 9, ME), creep("new", 30, 30, ME)],
        ),
        start,
    );
    let at = |scene: &mut Scene, id: &str, time: f64| {
        let animating = scene.frame(time, &Eye::default()).animating;
        (scene.drawn(id, time), animating)
    };
    let (mover, animating) = at(&mut scene, "mover", start);
    assert_eq!(mover.unwrap().0, Vec2::new(10.0, 10.0));
    assert!(animating);
    let (new, _) = at(&mut scene, "new", start);
    assert_eq!(
        new.unwrap().0,
        Vec2::new(30.0, 30.0),
        "new objects appear in place"
    );
    assert_eq!(new.unwrap().3, 0.0, "and fade in");
    let (gone, _) = at(&mut scene, "gone", start);
    assert_eq!(
        gone.unwrap().3,
        1.0,
        "removed objects fade out from where they were"
    );
    assert_eq!(scene.ghost_count(), 1);
    let (mover, _) = at(&mut scene, "mover", start + MOVE_DURATION_MS / 2.0);
    assert_eq!(
        mover.unwrap().0,
        Vec2::new(10.5, 9.5),
        "halfway (smoothstep is symmetric)"
    );
    let (mover, _) = at(&mut scene, "mover", start + MOVE_DURATION_MS / 4.0);
    let x = mover.unwrap().0.x;
    assert!(x > 10.1 && x < 10.25, "eased in: {x}");
    let (mover, _) = at(&mut scene, "mover", start + MOVE_DURATION_MS);
    assert_eq!(mover.unwrap().0, Vec2::new(11.0, 9.0));
    let settled = start + crate::scene::FADE_MS;
    let (new, animating) = at(&mut scene, "new", settled);
    assert_eq!(new.unwrap().3, 1.0);
    assert!(!animating, "done after the glide and the fades");
    assert_eq!(scene.ghost_count(), 0, "faded ghosts are dropped");
    assert_eq!(scene.tokens().len(), 2);

    // A move during a move starts from where the object is drawn.
    let restart = settled + 100.0;
    show(
        &mut scene,
        &snapshot(&terrain, vec![creep("mover", 12, 9, ME)]),
        restart,
    );
    show(
        &mut scene,
        &snapshot(&terrain, vec![creep("mover", 13, 9, ME)]),
        restart + MOVE_DURATION_MS / 2.0,
    );
    let (mover, animating) = at(&mut scene, "mover", restart + MOVE_DURATION_MS / 2.0);
    assert_eq!(mover.unwrap().0, Vec2::new(11.5, 9.0));
    assert!(animating);
    // An unchanged snapshot keeps the movement going.
    show(
        &mut scene,
        &snapshot(&terrain, vec![creep("mover", 13, 9, ME)]),
        restart + MOVE_DURATION_MS * 0.75,
    );
    let (mover, animating) = at(&mut scene, "mover", restart + MOVE_DURATION_MS * 1.5);
    assert_eq!(mover.unwrap().0, Vec2::new(13.0, 9.0));
    assert!(!animating);
}

#[test]
fn creeps_inside_their_spawn_hide_and_fade_in_when_they_leave() {
    let terrain = terrain();
    let mut scene = Scene::default();
    let spawning = |spawning: bool| {
        object(json!({
            "_id": "baby", "type": "creep", "room": "W1N1", "x": 20, "y": 20, "user": ME,
            "name": "baby", "hits": 100, "hitsMax": 100, "store": {}, "spawning": spawning,
            "body": [{"type": "move", "hits": 100}],
        }))
    };
    show(&mut scene, &snapshot(&terrain, vec![spawning(true)]), 0.0);
    scene.frame(0.0, &Eye::default());
    assert!(scene.tokens().is_empty(), "hidden in the spawn");
    show(
        &mut scene,
        &snapshot(&terrain, vec![spawning(false)]),
        500.0,
    );
    scene.frame(500.0, &Eye::default());
    assert!(scene.tokens().is_empty(), "fades in as it leaves");
    scene.frame(500.0 + crate::scene::FADE_MS / 2.0, &Eye::default());
    let fade = scene.tokens()[0].fade;
    assert!(fade > 0.0 && fade < 1.0, "{fade}");
    scene.frame(500.0 + crate::scene::FADE_MS, &Eye::default());
    assert_eq!(scene.tokens()[0].fade, 1.0);
}

#[test]
fn motion_can_be_switched_off() {
    let terrain = terrain();
    let mut scene = Scene::default();
    scene.set_features(crate::scene::Features {
        motion: false,
        ..crate::scene::Features::default()
    });
    show(
        &mut scene,
        &snapshot(&terrain, vec![creep("c", 10, 10, ME)]),
        0.0,
    );
    show(
        &mut scene,
        &snapshot(&terrain, vec![creep("c", 11, 10, ME)]),
        100.0,
    );
    assert_eq!(scene.drawn("c", 100.0).unwrap().0, Vec2::new(11.0, 10.0));
}

#[test]
fn another_room_snaps_every_object() {
    let terrain = terrain();
    let mut scene = Scene::default();
    show(
        &mut scene,
        &snapshot(&terrain, vec![creep("c", 10, 10, ME)]),
        0.0,
    );
    let other = snapshot_of("W2N1", &terrain, vec![creep("c", 40, 40, ME)]);
    show(&mut scene, &other, 100.0);
    let frame = scene.frame(100.0, &Eye::default());
    assert!(!frame.animating);
    assert_eq!(scene.drawn("c", 100.0).unwrap().0, Vec2::new(40.0, 40.0));
    assert_eq!(scene.drawn("c", 100.0).unwrap().3, 1.0);
    assert_eq!(scene.ghost_count(), 0);
}

#[test]
fn the_wall_mesh_is_rebuilt_only_when_the_terrain_changes() {
    let terrain = terrain();
    let mut scene = Scene::default();
    assert_eq!(scene.terrain().1, 0);
    assert!(show(&mut scene, &snapshot(&terrain, Vec::new()), 0.0));
    let (mesh, version) = scene.terrain();
    assert_eq!(version, 1);
    assert!(!mesh.indices.is_empty(), "walls are raised");
    assert!(!show(
        &mut scene,
        &snapshot(&terrain, vec![creep("c", 9, 9, ME)]),
        1.0
    ));
    let equal = Arc::new((*terrain).clone());
    assert!(
        !show(&mut scene, &snapshot(&equal, Vec::new()), 2.0),
        "equal terrain"
    );
    assert_eq!(scene.terrain().1, 1);
    let plains = Arc::new(Terrain::decode(&"0".repeat(2500)).unwrap());
    assert!(show(&mut scene, &snapshot(&plains, Vec::new()), 3.0));
    assert_eq!(scene.terrain().1, 2);
    assert!(
        scene.terrain().0.indices.is_empty(),
        "no walls: nothing raised"
    );
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
    show(&mut scene, &snapshot(&terrain, objects), 0.0);
    let base = scene.drawn("tunnel", 0.0).unwrap().1;
    assert_eq!(base, crate::mesh::WALL_HEIGHT, "road on a wall");
    let colors = |scene: &Scene| {
        ["mine", "theirs", "invader", "keeper"].map(|id| scene.drawn(id, 0.0).unwrap().2)
    };
    let before = colors(&scene);
    assert_eq!(before[0], before[1], "no user signed in: all players alike");
    assert_ne!(before[1], before[2]);
    assert_ne!(before[2], before[3]);
    assert_eq!(
        scene.drawn("tunnel", 0.0).unwrap().2[3],
        0,
        "roads have no owner"
    );
    scene.set_user(Some(ME.to_owned()));
    let after = colors(&scene);
    assert_ne!(
        after[0], after[1],
        "own creep differs from another player's"
    );
    // Tokens sit on top of their prism; prisms only show in perspective.
    let frame = scene.frame(0.0, &Eye::default());
    assert!(frame.sides.iter().all(std::ops::Range::is_empty));
    assert!(scene.sides().is_empty(), "no prisms top-down");
    let eye = Eye {
        perspective: true,
        ..Eye::default()
    };
    let frame = scene.frame(0.0, &eye);
    let creeps = &frame.sides[0];
    assert_eq!(creeps.len(), 4, "four creep cylinders");
}

#[test]
fn tokens_are_drawn_far_to_near() {
    let terrain = terrain();
    let mut scene = Scene::default();
    let objects = vec![
        creep("creep", 20, 20, ME),
        structure("road", "road", 20, 20),
        structure("rampart", "rampart", 20, 20),
        structure("road", "far", 20, 40),
    ];
    show(&mut scene, &snapshot(&terrain, objects), 0.0);
    // Top-down: by height.
    scene.frame(0.0, &Eye::default());
    let heights: Vec<f32> = scene
        .tokens()
        .iter()
        .map(|token| token.position[1])
        .collect();
    assert!(
        heights.windows(2).all(|pair| pair[0] <= pair[1]),
        "{heights:?}"
    );
    // Looking north from the south: the far road first.
    let eye = Eye {
        position: Vec3::new(20.5, 10.0, 60.0),
        forward: Vec3::new(0.0, -0.5, -1.0).normalize(),
        perspective: true,
    };
    scene.frame(0.0, &eye);
    assert_eq!(scene.tokens().len(), 4);
    assert_eq!(scene.tokens()[3].position[2], 40.5, "the nearest last");
}
