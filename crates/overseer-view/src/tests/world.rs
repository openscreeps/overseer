//! The world's change tracking without a GPU: dirty marks and uploads, packed terrain and
//! its overview, a whole shard without eviction, placeholders and detail scenes.

use overseer::map::{RoomCoord, RoomRect, RoomStatus};
use overseer::model::world::Terrain;

use super::{ME, minimap, room, snapshot_of, stat, terrain};
use crate::MINIMAP_ROOMS;
use crate::camera::room_centre;
use crate::world::{Limits, World};

/// What a flush hands out: `(level, layer, origin, size)` per rectangle.
type Rects = Vec<(u32, u32, (u32, u32), (u32, u32))>;

/// Flushes the terrain and the overview, as the GPU does.
fn flush(world: &mut World) -> (Rects, Rects) {
    let (mut terrain, mut overview) = (Vec::new(), Vec::new());
    world
        .terrain
        .flush(|upload| terrain.push((upload.level, upload.layer, upload.origin, upload.size)));
    world
        .overview
        .flush(|upload| overview.push((upload.level, upload.layer, upload.origin, upload.size)));
    (terrain, overview)
}

#[test]
fn updates_touch_only_the_rooms_that_changed() {
    let bounds = RoomRect::new(room("W4N4"), room("E4S4"));
    let mut world = World::new(bounds, Limits::WEBGL2);
    world.set_focus(room_centre(room("W0N0")));
    // What a fresh world must upload in full: every instance, the overview (one rectangle
    // per mip level: 100, 50, 25, 12, 6, 3 and 1 texels); no terrain yet.
    assert_eq!(world.cells.dirty.marks(), (true, Vec::new()));
    world.cells.dirty.drain(100, |_| {});
    let (terrain_rects, overview_rects) = flush(&mut world);
    assert_eq!(terrain_rects, []);
    let sizes: Vec<_> = overview_rects.iter().map(|rect| rect.3).collect();
    assert_eq!(
        sizes,
        [
            (100, 100),
            (50, 50),
            (25, 25),
            (12, 12),
            (6, 6),
            (3, 3),
            (1, 1)
        ]
    );

    // An overview: one instance.
    let e1s2 = room("E1S2");
    world.set_stat(e1s2, Some(&stat(RoomStatus::Normal, Some(("other", 3)))));
    let index = (2 + 5) * 10 + (1 + 5);
    assert_eq!(world.cells.dirty.marks(), (false, vec![index]));
    let instance = world.cells.instances()[index as usize];
    assert_ne!(instance.overlay, world.cells.instances()[0].overlay);
    assert!(
        instance.overlay[3] > instance.tint[3],
        "stronger zoomed out"
    );
    world.set_stat(e1s2, Some(&stat(RoomStatus::Normal, Some(("other", 3)))));
    assert_eq!(
        world.cells.dirty.marks().1.len(),
        1,
        "unchanged: no new mark"
    );
    world.cells.dirty.drain(100, |_| {});
    assert_eq!(world.cells.dirty.marks(), (false, Vec::new()));
    assert_eq!(flush(&mut world), (Vec::new(), Vec::new()), "no texels");

    // A terrain: its instance, its slot's 25×25 texels and its overview texels (10×10 at
    // level 0, what covers them below).
    world.set_terrain(e1s2, &terrain());
    let slot = world.terrain.slot(e1s2).unwrap();
    assert_eq!(world.cells.instances()[index as usize].terrain, slot);
    assert_eq!(world.cells.dirty.marks(), (false, vec![index]));
    world.cells.dirty.drain(100, |_| {});
    let (terrain_rects, overview_rects) = flush(&mut world);
    assert_eq!(terrain_rects, [(0, 0, (0, 0), (25, 25))]);
    assert_eq!(overview_rects.len(), 7, "one rectangle per level");
    assert_eq!(overview_rects[0], (0, 0, (60, 70), (10, 10)));
    for (level, rect) in overview_rects.iter().enumerate() {
        assert_eq!(rect.0 as usize, level);
        assert!(rect.3.0 <= (10 >> level) + 2 && rect.3.1 <= (10 >> level) + 2);
    }
    // The same terrain again: nothing.
    world.set_terrain(e1s2, &terrain());
    assert_eq!(world.cells.dirty.marks(), (false, Vec::new()));
    assert_eq!(flush(&mut world), (Vec::new(), Vec::new()));
    // A new overview of a room with terrain: its instance only.
    world.set_stat(e1s2, Some(&stat(RoomStatus::Novice, None)));
    assert_eq!(world.cells.dirty.marks(), (false, vec![index]));
    assert_eq!(flush(&mut world), (Vec::new(), Vec::new()));
    // A room without terrain on the server: its instance only.
    world.cells.dirty.drain(100, |_| {});
    world.mark_no_terrain(room("W4N4"));
    assert_eq!(world.cells.dirty.marks(), (false, vec![0]));
    assert_eq!(flush(&mut world), (Vec::new(), Vec::new()));

    // A minimap: its slot only.
    world.set_minimap(e1s2, Some(&minimap("other")));
    world.set_minimap(room("W2S0"), Some(&minimap(ME)));
    assert_eq!(world.minimaps.dirty.marks(), (false, vec![0, 1]));
    world.minimaps.dirty.drain(MINIMAP_ROOMS, |_| {});
    world.set_minimap(room("W2S0"), Some(&minimap("another")));
    assert_eq!(world.minimaps.dirty.marks(), (false, vec![1]));
    assert_eq!(world.minimaps.points(1).len(), 7);
    // Signing in recolours (every instance, once) but uploads no texels.
    world.set_user(Some(ME.to_owned()));
    assert!(world.cells.dirty.marks().0);
    assert_eq!(flush(&mut world), (Vec::new(), Vec::new()));
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

/// Three different terrains: walls, swamps and plains in changing places.
fn terrains() -> [Terrain; 3] {
    [0, 1, 2].map(|shift| {
        let digits: String = (0..2500)
            .map(|index: usize| match (index * 7 + index / 50 + shift) % 5 {
                0 | 3 => '1',
                1 => '2',
                2 => '3',
                _ => '0',
            })
            .collect();
        Terrain::decode(&digits).unwrap()
    })
}

/// The code of a tile as `Terrain::packed` has it.
fn code(tile: overseer::model::world::Tile) -> u8 {
    match tile {
        overseer::model::world::Tile::Plain => 0,
        overseer::model::world::Tile::Wall => 1,
        overseer::model::world::Tile::Swamp => 2,
    }
}

/// Every tile of `slot` reads back as `terrain` through the shader's addressing.
fn assert_decodes(world: &World, slot: u32, terrain: &Terrain) {
    assert_eq!(&world.terrain.packed(slot), terrain.packed(), "slot {slot}");
    for y in 0..50_u8 {
        for x in 0..50_u8 {
            assert_eq!(
                world.terrain.tile(slot, u32::from(x), u32::from(y)),
                code(terrain.get(x, y).unwrap()),
                "slot {slot} tile ({x}, {y})"
            );
        }
    }
}

#[test]
fn packed_terrain_decodes_like_the_shader_for_every_tile_kind() {
    let terrains = terrains();
    let kinds: std::collections::BTreeSet<u8> = (0..50_u8)
        .flat_map(|y| (0..50_u8).map(move |x| (x, y)))
        .map(|(x, y)| code(terrains[0].get(x, y).unwrap()))
        .collect();
    assert_eq!(kinds, [0, 1, 2].into(), "plains, walls and swamps");
    // Enough rooms to fill a layer and start the next: slots across slot rows and layers.
    let bounds = RoomRect::new(room("E0S0"), room("E49S39"));
    assert_eq!(bounds.len(), 2000);
    let mut world = World::new(bounds, Limits::WEBGL2);
    for (index, room) in bounds.iter().enumerate() {
        world.set_terrain(room, &terrains[index % 3]);
    }
    for slot in [0, 1, 39, 40, 41, 1639, 1640, 1999] {
        assert_decodes(&world, slot, &terrains[slot as usize % 3]);
    }

    // The overview averages 5×5 tiles per texel: walls (red), swamps (green), plains (blue).
    let simple = terrain();
    let e0s0 = room("E0S0");
    let mut world = World::new(RoomRect::new(e0s0, e0s0), Limits::WEBGL2);
    assert_eq!(world.overview.texels_per_room(), 10);
    assert_eq!(world.overview.texel(0, 0, 0), [0; 4], "the placeholder");
    world.set_terrain(e0s0, &simple);
    assert_eq!(
        world.overview.texel(0, 0, 0),
        [255, 0, 0, 255],
        "x < 5: walls"
    );
    assert_eq!(
        world.overview.texel(0, 6, 6),
        [0, 255, 0, 255],
        "30..35: swamp"
    );
    assert_eq!(world.overview.texel(0, 3, 3), [0, 0, 255, 255], "plains");
    flush(&mut world);
    // Level 1 (5×5): the first column is half walls, half plains.
    assert_eq!(world.overview.texel(1, 0, 0), [128, 0, 128, 255]);
}

#[test]
fn a_whole_shard_stays_resident_without_eviction() {
    // screeps.com shard0: 182 × 182 rooms.
    let bounds = RoomRect::new(room("W90N90"), room("E90S90"));
    assert_eq!(bounds.len(), 33_124);
    let mut world = World::new(bounds, Limits::WEBGL2);
    assert_eq!(
        world.terrain.layers(),
        21,
        "reserved for the whole shard up front"
    );
    assert_eq!(world.overview.size(), (1820, 1820), "10 texels per room");
    let generation = (world.terrain.generation(), world.overview.generation());
    flush(&mut world);
    let terrains = terrains();
    for (index, room) in bounds.iter().enumerate() {
        world.set_terrain(room, &terrains[index % 3]);
    }
    assert_eq!(world.terrain.len(), 33_124, "every room kept");
    assert_eq!(
        (world.terrain.generation(), world.overview.generation()),
        generation,
        "no texture recreated"
    );
    let mut slots = std::collections::HashSet::new();
    for (index, room) in bounds.iter().enumerate() {
        let slot = world.terrain.slot(room).unwrap();
        assert!(slots.insert(slot), "{room}: slot {slot} shared");
        assert_eq!(world.cells.instances()[index].terrain, slot);
    }
    for slot in [0, 1639, 1640, 33_123] {
        assert_decodes(&world, slot, &terrains[slot as usize % 3]);
    }
    // One flush uploads each room's texels once, in few rectangles.
    let (terrain_rects, overview_rects) = flush(&mut world);
    let texels: u32 = terrain_rects.iter().map(|rect| rect.3.0 * rect.3.1).sum();
    assert_eq!(texels, 33_124 * 625);
    assert!(
        terrain_rects.len() <= 2 * 21,
        "{} rectangles",
        terrain_rects.len()
    );
    assert_eq!(
        overview_rects.len(),
        world.overview.level_count() as usize,
        "one rectangle per level"
    );

    // Rooms beyond the reservation (outside the bounds) grow the texture and keep the rest.
    let first = bounds.min;
    for x in 0..1317 {
        world.set_terrain(RoomCoord { x: 1000 + x, y: 0 }, &terrains[0]);
    }
    assert_eq!(world.terrain.layers(), 42, "twice the layers once full");
    assert_ne!(world.terrain.generation(), generation.0);
    assert_decodes(&world, world.terrain.slot(first).unwrap(), &terrains[0]);
    assert_eq!(
        world.terrain.set(first, &terrains[0]),
        None,
        "known: unchanged"
    );
}

#[test]
fn rooms_without_terrain_are_placeholders_and_absent_rooms_are_outside() {
    use crate::world::terrain::state::{ABSENT, PENDING};
    let bounds = RoomRect::new(room("W1N1"), room("E0S0"));
    let mut world = World::new(bounds, Limits::WEBGL2);
    let state = |world: &World, name: &str| {
        world.cells.instances()[world.cells.index(room(name)).unwrap()].terrain
    };
    assert!(
        world
            .cells
            .instances()
            .iter()
            .all(|instance| instance.terrain == PENDING)
    );
    world.mark_no_terrain(room("W1N1"));
    assert_eq!(state(&world, "W1N1"), ABSENT);
    world.set_terrain(room("W0N1"), &terrain());
    assert_eq!(state(&world, "W0N1"), 0);
    world.mark_no_terrain(room("W0N1"));
    assert_eq!(state(&world, "W0N1"), 0, "known terrain wins");
    assert_eq!(state(&world, "E0S0"), PENDING);
    // The overview: loaded rooms' shares add up to one, the others' to nothing (drawn as
    // the placeholder; absent rooms are not drawn at all).
    let [r, g, b, _] = world.overview.texel(0, 10, 0);
    assert_eq!(u16::from(r) + u16::from(g) + u16::from(b), 255);
    assert_eq!(world.overview.texel(0, 0, 0), [0; 4]);
    assert_eq!(world.overview.texel(0, 15, 15), [0; 4]);

    // A wider scope keeps every state; terrain arriving late replaces an absent marker.
    world.set_bounds(RoomRect::new(room("W2N2"), room("E1S1")));
    assert_eq!(state(&world, "W1N1"), ABSENT);
    assert_eq!(state(&world, "W0N1"), 0);
    assert_eq!(state(&world, "W2N2"), PENDING);
    let [r, g, b, _] = world.overview.texel(0, 20, 10);
    assert_eq!(
        u16::from(r) + u16::from(g) + u16::from(b),
        255,
        "W0N1 redrawn"
    );
    world.set_terrain(room("W1N1"), &terrain());
    assert_eq!(state(&world, "W1N1"), 1);
}

#[test]
fn detail_scenes_fade_in_and_are_replaced_farthest_first() {
    let mut world = World::new(RoomRect::new(room("E0S0"), room("E9S9")), Limits::WEBGL2);
    world.set_focus(room_centre(room("E0S0")));
    let terrain = terrain();
    for (index, name) in ["E0S0", "E1S0", "E2S0", "E9S9"].into_iter().enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "small numbers")]
        world.set_room(&snapshot_of(name, &terrain, Vec::new()), index as f64);
    }
    let (_, animating) = world.frame(10.0, &crate::scene::Eye::default());
    assert!(animating, "fading in");
    assert!(world.detail(room("E0S0")).unwrap().arrival(310.0) >= 1.0);
    let (_, animating) = world.frame(400.0, &crate::scene::Eye::default());
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
        world.terrain.has_terrain(room("E2S0")),
        "snapshots bring terrain"
    );
}
