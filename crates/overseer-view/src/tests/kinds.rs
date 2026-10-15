//! Every kind of object as a detail scene draws it, and how the scene replaces its room's
//! minimap: the minimap's coarse squares (two tiles wide for controllers, sources, minerals,
//! keeper lairs and power banks) must not show under the scene's tokens.

use glam::Vec2;
use overseer::core::api::types::GameRoomObject;
use overseer::map::RoomMap;
use serde_json::{Value, json};

use super::{ME, object, room, snapshot_of, terrain, world_view};
use crate::scene::style::{Glyph, Look, flag};

/// The tile of the `index`th object of [`every_kind`]: row-major from (5, 2), three tiles
/// apart.
fn kind_tile(index: u8) -> (u8, u8) {
    (5 + 3 * (index % 14), 2 + 3 * (index / 14))
}

/// One object of every kind the server sends, each on its own tile ([`kind_tile`]), with the
/// fields the server always sends.
fn every_kind() -> Vec<GameRoomObject> {
    let kinds = [
        (
            "creep",
            json!({"user": ME, "name": "c", "hits": 100, "hitsMax": 100, "store": {"energy": 20}, "body": [{"type": "work", "hits": 100}, {"type": "move", "hits": 100}]}),
        ),
        (
            "powerCreep",
            json!({"user": ME, "name": "p", "hits": 1000, "hitsMax": 1000, "store": {}, "className": "operator", "level": 1, "powers": {}, "actionLog": {}, "ageTime": 5000, "notifyWhenAttacked": true, "shard": "shard0", "spawnCooldownTime": 0, "deleteTime": 0}),
        ),
        (
            "spawn",
            json!({"user": ME, "name": "Spawn1", "hits": 5000, "hitsMax": 5000, "store": {"energy": 200}, "storeCapacityResource": {"energy": 300}, "notifyWhenAttacked": true}),
        ),
        (
            "extension",
            json!({"user": ME, "hits": 1000, "hitsMax": 1000, "store": {}, "storeCapacityResource": {"energy": 50}, "notifyWhenAttacked": true}),
        ),
        (
            "tower",
            json!({"user": ME, "hits": 3000, "hitsMax": 3000, "store": {}, "storeCapacityResource": {"energy": 1000}, "notifyWhenAttacked": true}),
        ),
        (
            "storage",
            json!({"user": ME, "hits": 10000, "hitsMax": 10000, "store": {"energy": 5000}, "storeCapacity": 1_000_000, "notifyWhenAttacked": true}),
        ),
        (
            "terminal",
            json!({"user": ME, "hits": 3000, "hitsMax": 3000, "store": {}, "storeCapacity": 300_000, "notifyWhenAttacked": true}),
        ),
        (
            "container",
            json!({"hits": 250_000, "hitsMax": 250_000, "store": {"energy": 500}, "storeCapacity": 2000, "nextDecayTime": 100, "notifyWhenAttacked": true}),
        ),
        (
            "link",
            json!({"user": ME, "hits": 1000, "hitsMax": 1000, "store": {}, "storeCapacityResource": {"energy": 800}, "cooldown": 0, "notifyWhenAttacked": true}),
        ),
        (
            "lab",
            json!({"user": ME, "hits": 500, "hitsMax": 500, "store": {}, "storeCapacity": 5000, "storeCapacityResource": {"energy": 2000}, "cooldown": 0, "notifyWhenAttacked": true}),
        ),
        (
            "factory",
            json!({"user": ME, "hits": 1000, "hitsMax": 1000, "store": {}, "storeCapacity": 50000, "cooldown": 0, "level": 0, "notifyWhenAttacked": true}),
        ),
        (
            "powerSpawn",
            json!({"user": ME, "hits": 5000, "hitsMax": 5000, "store": {}, "storeCapacityResource": {"energy": 5000, "power": 100}, "notifyWhenAttacked": true}),
        ),
        (
            "nuker",
            json!({"user": ME, "hits": 1000, "hitsMax": 1000, "store": {}, "storeCapacityResource": {"energy": 300_000, "G": 5000}, "cooldownTime": 0, "notifyWhenAttacked": true}),
        ),
        (
            "observer",
            json!({"user": ME, "hits": 500, "hitsMax": 500, "notifyWhenAttacked": true}),
        ),
        (
            "extractor",
            json!({"user": ME, "hits": 500, "hitsMax": 500, "cooldown": 0, "notifyWhenAttacked": true}),
        ),
        (
            "controller",
            json!({"user": ME, "level": 4, "progress": 100_000, "progressTotal": 405_000, "downgradeTime": 20000}),
        ),
        (
            "source",
            json!({"energy": 2000, "energyCapacity": 3000, "ticksToRegeneration": 100}),
        ),
        (
            "mineral",
            json!({"mineralType": "O", "mineralAmount": 50000, "density": 3}),
        ),
        (
            "deposit",
            json!({"depositType": "metal", "harvested": 0, "cooldownTime": 0, "decayTime": 50000}),
        ),
        (
            "powerBank",
            json!({"hits": 2_000_000, "hitsMax": 2_000_000, "store": {"power": 3000}, "decayTime": 5000}),
        ),
        (
            "constructedWall",
            json!({"hits": 10000, "hitsMax": 300_000_000, "notifyWhenAttacked": true}),
        ),
        (
            "rampart",
            json!({"user": ME, "hits": 10000, "hitsMax": 300_000, "isPublic": false, "nextDecayTime": 100, "notifyWhenAttacked": true}),
        ),
        (
            "road",
            json!({"hits": 5000, "hitsMax": 5000, "nextDecayTime": 100, "notifyWhenAttacked": true}),
        ),
        (
            "constructionSite",
            json!({"user": ME, "structureType": "extension", "progress": 1000, "progressTotal": 3000, "name": ""}),
        ),
        ("energy", json!({"energy": 300, "resourceType": "energy"})),
        (
            "tombstone",
            json!({"creepId": "x", "creepName": "x", "creepTicksToLive": 0, "creepBody": ["work"], "store": {"energy": 50}, "deathTime": 1, "decayTime": 100, "user": ME}),
        ),
        (
            "ruin",
            json!({"structureType": "spawn", "store": {}, "destroyTime": 1, "decayTime": 100, "structure": {"id": "y", "type": "spawn", "hits": 0, "hitsMax": 5000, "user": ME}, "user": ME}),
        ),
        ("nuke", json!({"launchRoomName": "W5N5", "landTime": 5000})),
        (
            "portal",
            json!({"destination": {"room": "W5N5", "x": 25, "y": 25}}),
        ),
        ("keeperLair", json!({"user": "3", "nextSpawnTime": 100})),
        (
            "invaderCore",
            json!({"user": "2", "hits": 100_000, "hitsMax": 100_000, "level": 1, "store": {}, "strongholdId": "s", "notifyWhenAttacked": true, "depositType": "metal", "nextExpandTime": 1, "effects": []}),
        ),
    ];
    kinds
        .into_iter()
        .zip(0_u8..)
        .map(|((kind, fields), index)| {
            let (x, y) = kind_tile(index);
            let mut value: Value = json!({
                "_id": format!("o{index}"), "type": kind, "room": "W1N1", "x": x, "y": y,
            });
            for (key, field) in fields.as_object().unwrap() {
                value[key] = field.clone();
            }
            object(value)
        })
        .collect()
}

#[test]
fn every_kind_draws_one_token_within_its_tile() {
    let objects = every_kind();
    let mut view = world_view();
    let w1n1 = room("W1N1");
    view.world
        .set_room(&snapshot_of("W1N1", &terrain(), objects.clone()), 0.0);
    view.world.frame(1000.0, &view.eye());
    let detail = view.world.detail(w1n1).unwrap();
    let tokens = detail.scene.tokens();
    assert_eq!(tokens.len(), objects.len(), "one token per object");
    let mut glyphs = Vec::new();
    for (object, index) in objects.iter().zip(0_u8..) {
        let look = Look::of(object);
        let (x, y) = kind_tile(index);
        // Centred on its tile.
        let centre = Vec2::new(f32::from(x), f32::from(y)) + 0.5;
        let token = *tokens
            .iter()
            .find(|token| Vec2::new(token.position[0], token.position[2]).distance(centre) < 1e-5)
            .unwrap_or_else(|| panic!("{:?}: no token centred on its tile", look.glyph));
        let glyph = token.kind & 0xff;
        assert_eq!(glyph, u32::from(look.glyph as u8), "{object:?}");
        assert_ne!(look.glyph, Glyph::Other, "{object:?}");
        glyphs.push(glyph);
        // Within its tile: the token's silhouette never covers a neighbour.
        assert!(
            token.size > 0.0 && token.size <= 1.0,
            "{:?}: footprint {}",
            look.glyph,
            token.size
        );
        // Flat kinds (no prism) cast no shadow and draw no prism sides.
        let shape = token.kind >> 16 & 0xff;
        assert_eq!(shape == 0xff, look.shape.is_none(), "{:?}", look.glyph);
        // Live objects are opaque and not ghosts.
        assert_eq!(
            token.kind >> 24 & u32::from(flag::GHOST),
            0,
            "{:?}",
            look.glyph
        );
        assert_eq!(token.fade, 1.0, "{:?}", look.glyph);
    }
    glyphs.sort_unstable();
    glyphs.dedup();
    assert_eq!(glyphs.len(), objects.len(), "every kind its own emblem");
}

#[test]
fn a_detail_scene_replaces_its_rooms_minimap() {
    let mut view = world_view();
    let (w1n1, w2n1) = (room("W1N1"), room("W2N1"));
    // The minimap's squares: two tiles for controllers, sources, minerals, keeper lairs
    // and power banks, wider than any token.
    let map: RoomMap = serde_json::from_value(json!({
        "c": [[5, 2]], "s": [[8, 2]], "m": [[11, 2]], "k": [[14, 2]], "pb": [[17, 2]],
        "p": [[20, 2]], ME: [[23, 2]],
    }))
    .unwrap();
    for coord in [w1n1, w2n1] {
        view.world.set_minimap(coord, Some(&map));
    }
    assert_eq!(
        view.world.detail_slot(w1n1),
        None,
        "a minimap only: drawn as is"
    );
    view.world
        .set_room(&snapshot_of("W1N1", &terrain(), every_kind()), 1000.0);
    let slot = view.world.detail_slot(w1n1).unwrap();
    let detail = view.world.details()[slot].as_ref().unwrap();
    assert_eq!(detail.room, w1n1);
    assert_eq!(
        view.world.detail_slot(w2n1),
        None,
        "other rooms keep their minimap"
    );
    // The points are drawn at `1 - opacity` (`point_vertex` in `shader.wgsl`): fully
    // shown before the scene arrives or below the detail level, gone once it shows.
    assert_eq!(detail.opacity(1.0, 1000.0), 0.0);
    assert_eq!(detail.opacity(0.0, 5000.0), 0.0);
    assert!((detail.opacity(1.0, 1150.0) - 0.5).abs() < 1e-6, "arriving");
    assert!(
        (detail.opacity(0.5, 5000.0) - 0.5).abs() < 1e-6,
        "zooming in"
    );
    assert_eq!(detail.opacity(1.0, 5000.0), 1.0);
    view.world.clear_room(w1n1);
    assert_eq!(
        view.world.detail_slot(w1n1),
        None,
        "the minimap shows again"
    );
}
