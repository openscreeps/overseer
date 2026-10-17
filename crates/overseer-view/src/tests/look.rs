//! How objects look: road and block links, gauges (stores, hits, progress, body parts),
//! resource colours, silhouettes and the packing of tokens.

use glam::Vec2;
use serde_json::json;

use super::{ME, object, show, snapshot, structure, terrain};
use crate::mesh::Shape;
use crate::scene::connect::Grid;
use crate::scene::style::{Gauges, Glyph, Join, Look, flag, resource_color};
use crate::scene::{Eye, Scene};

/// Link bits (`scene::connect`): N, NE, E, SE, S, SW, W, NW.
const N: u8 = 1;
const NE: u8 = 2;
const E: u8 = 4;
const SE: u8 = 8;
const S: u8 = 16;
const W: u8 = 64;

fn grid(tiles: &[(u8, u8)]) -> Grid {
    let mut grid = Grid::default();
    for &(x, y) in tiles {
        grid.insert(x, y);
    }
    grid
}

#[test]
fn roads_link_to_neighbouring_roads() {
    // A straight east-west road.
    let line = grid(&[(10, 10), (11, 10), (12, 10)]);
    assert_eq!(line.links(11, 10, Join::Link), E | W);
    assert_eq!(line.links(10, 10, Join::Link), E);
    // A diagonal road links diagonally.
    let diagonal = grid(&[(20, 20), (21, 21), (22, 22)]);
    assert_eq!(diagonal.links(21, 21, Join::Link), SE | 128);
    // A bend: no diagonal shortcut where the orthogonal path exists.
    let bend = grid(&[(30, 30), (31, 30), (31, 29)]);
    assert_eq!(
        bend.links(30, 30, Join::Link),
        E,
        "no NE link past (31, 30)"
    );
    assert_eq!(bend.links(31, 30, Join::Link), N | W);
    // Room edges: nothing beyond.
    let edge = grid(&[(0, 0), (49, 49)]);
    assert_eq!(edge.links(0, 0, Join::Link), 0);
    assert_eq!(edge.links(49, 49, Join::Link), 0);
    // Other kinds do not link.
    assert_eq!(line.links(11, 10, Join::None), 0);
}

#[test]
fn walls_and_ramparts_merge_and_fill_corners_between_three() {
    let block = grid(&[(5, 5), (6, 5), (5, 6), (6, 6)]);
    assert_eq!(block.links(5, 5, Join::Fill), E | SE | S);
    let ell = grid(&[(5, 5), (6, 5), (5, 6)]);
    assert_eq!(
        ell.links(5, 5, Join::Fill),
        E | S,
        "no corner without the diagonal"
    );
    let diagonal = grid(&[(5, 5), (6, 6)]);
    assert_eq!(
        diagonal.links(5, 5, Join::Fill),
        0,
        "diagonals alone stay apart"
    );
    assert_eq!(grid(&[(8, 8), (9, 7)]).links(8, 8, Join::Fill), 0);
    assert_eq!(grid(&[(8, 8), (9, 7)]).links(8, 8, Join::Link), NE);
}

#[test]
fn tokens_carry_their_links_kind_and_silhouette() {
    let terrain = terrain();
    let mut scene = Scene::default();
    let objects = vec![
        structure("road", "a", 10, 10),
        structure("road", "b", 11, 10),
        structure("rampart", "r1", 20, 20),
        structure("rampart", "r2", 20, 21),
        structure("spawn", "spawn", 30, 30),
    ];
    show(&mut scene, &snapshot(&terrain, objects), 0.0);
    scene.frame(0.0, &Eye::default());
    let token = |x: f32, y: f32, glyph: Glyph| {
        *scene
            .tokens()
            .iter()
            .find(|t| {
                Vec2::new(t.position[0], t.position[2]).distance(Vec2::new(x, y) + 0.5) < 1e-6
                    && t.kind & 255 == glyph as u32
            })
            .unwrap()
    };
    let road = token(10.0, 10.0, Glyph::Road);
    assert_eq!(road.kind >> 8 & 255, u32::from(E));
    assert_eq!(road.kind >> 16 & 255, 255, "roads have no prism");
    let rampart = token(20.0, 20.0, Glyph::Rampart);
    assert_eq!(rampart.kind >> 8 & 255, u32::from(S));
    assert_eq!(rampart.accent[3], 255, "owned: an owner colour");
    let spawn = token(30.0, 30.0, Glyph::Spawn);
    assert_eq!(spawn.kind >> 16 & 255, Shape::Circle.index());
    assert_eq!(
        spawn.position[1],
        Look::of(&structure("spawn", "s", 1, 1)).height
    );
}

#[test]
fn gauges_show_stores_hits_progress_and_body_parts() {
    let creep = object(json!({
        "_id": "c", "type": "creep", "room": "W1N1", "x": 1, "y": 1, "user": ME,
        "name": "c", "hits": 250, "hitsMax": 500, "store": {"energy": 25, "U": 50},
        "body": [
            {"type": "carry", "hits": 100}, {"type": "carry", "hits": 100},
            {"type": "move", "hits": 100}, {"type": "work", "hits": 0},
            {"type": "tough", "hits": 100},
        ],
    }));
    let gauges = Gauges::of(&creep, Some(1));
    let (fill, color) = gauges.store.unwrap();
    assert!((fill - 0.75).abs() < 1e-6, "75 of 2 carry × 50");
    assert_eq!(color, resource_color("U"), "the largest resource's colour");
    assert_eq!(gauges.hits, Some(0.5));
    assert_eq!(gauges.parts, [1, 1, 2, 0, 0, 0, 0, 1]);

    let spawn = object(json!({
        "_id": "s", "type": "spawn", "room": "W1N1", "x": 1, "y": 1, "user": ME,
        "hits": 5000, "hitsMax": 5000, "notifyWhenAttacked": true, "name": "Spawn1",
        "store": {"energy": 150}, "storeCapacityResource": {"energy": 300},
        "spawning": {"name": "x", "needTime": 30, "spawnTime": 110},
    }));
    let gauges = Gauges::of(&spawn, Some(100));
    assert_eq!(gauges.store.unwrap().0, 0.5);
    assert_eq!(gauges.hits, None, "undamaged: no bar");
    assert_eq!(gauges.flags & flag::SPAWNING, flag::SPAWNING);
    assert!(
        (gauges.progress.unwrap() - 2.0 / 3.0).abs() < 1e-6,
        "20 of 30 ticks"
    );

    let controller = object(json!({
        "_id": "k", "type": "controller", "room": "W1N1", "x": 1, "y": 1, "level": 0,
        "reservation": {"user": "other", "endTime": 5000},
    }));
    let gauges = Gauges::of(&controller, Some(1));
    assert_eq!(gauges.level, 0);
    assert_eq!(gauges.flags & flag::RESERVED, flag::RESERVED);
    assert_eq!(gauges.accent_user.as_deref(), Some("other"));
    let controller = object(json!({
        "_id": "k", "type": "controller", "room": "W1N1", "x": 1, "y": 1, "level": 4,
        "user": ME, "progress": 100_000, "progressTotal": 400_000,
    }));
    let gauges = Gauges::of(&controller, Some(1));
    assert_eq!((gauges.level, gauges.progress), (4, Some(0.25)));

    // A lab shows the compound it holds (of 3000), not the energy that powers it.
    let lab = |store: serde_json::Value| {
        object(json!({
            "_id": "l", "type": "lab", "room": "W1N1", "x": 1, "y": 1, "user": ME,
            "hits": 500, "hitsMax": 500, "notifyWhenAttacked": true, "cooldown": 0,
            "store": store, "storeCapacity": 5000, "storeCapacityResource": {"energy": 2000},
        }))
    };
    let gauges = Gauges::of(&lab(json!({"energy": 2000, "XGH2O": 1500})), None);
    assert_eq!(gauges.store, Some((0.5, resource_color("XGH2O"))));
    let gauges = Gauges::of(&lab(json!({"energy": 1000})), None);
    assert_eq!(
        gauges.store.unwrap().1,
        resource_color("energy"),
        "energy alone"
    );

    let small = Gauges::of(&super::energy("e", 1, 1), None).size.unwrap();
    let big = object(json!({
        "_id": "e", "type": "energy", "room": "W1N1", "x": 1, "y": 1, "energy": 5000,
    }));
    assert!(
        Gauges::of(&big, None).size.unwrap() > small,
        "piles grow with the amount"
    );
    assert_ne!(resource_color("energy"), resource_color("power"));
    assert_ne!(resource_color("U"), resource_color("L"));
    assert_eq!(
        resource_color("XUH2O"),
        resource_color("U"),
        "boosts take their mineral's"
    );
}

#[test]
fn every_structure_and_creep_kind_has_its_own_emblem() {
    let kinds = [
        "spawn",
        "extension",
        "tower",
        "storage",
        "terminal",
        "container",
        "link",
        "lab",
        "factory",
        "powerSpawn",
        "nuker",
        "observer",
        "extractor",
        "rampart",
        "road",
        "constructedWall",
    ];
    let mut glyphs = std::collections::HashSet::new();
    for kind in kinds {
        let look = Look::of(&object(json!({
            "_id": "x", "type": kind, "room": "W1N1", "x": 1, "y": 1, "user": ME,
            "hits": 100, "hitsMax": 100, "nextDecayTime": 1000, "notifyWhenAttacked": true,
            "store": {}, "storeCapacityResource": {}, "cooldown": 0, "cooldownTime": 0,
        })));
        assert!(glyphs.insert(look.glyph as u8), "{kind}");
        assert!(look.size > 0.0 && look.size <= 1.0);
    }
    assert!(glyphs.insert(Look::of(&super::creep("c", 1, 1, ME)).glyph as u8));
}

#[test]
fn silhouette_outlines_lie_on_their_distance_fields() {
    for shape in Shape::ALL {
        let Some(polygon) = shape.polygon() else {
            continue;
        };
        for (point, normal) in polygon.outline() {
            assert!(
                polygon.sd(point).abs() < 1e-4,
                "{shape:?} at {point}: {}",
                polygon.sd(point)
            );
            assert!(
                polygon.sd(point + normal * 0.02) > 0.0,
                "{shape:?}: outward {normal}"
            );
            assert!(
                point.abs().max_element() <= 0.55,
                "{shape:?} fits its footprint: {point}"
            );
        }
        assert!(
            polygon.sd(Vec2::ZERO) < 0.0,
            "{shape:?} contains its centre"
        );
    }
}
