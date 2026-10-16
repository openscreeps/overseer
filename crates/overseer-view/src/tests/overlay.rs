//! The overlay: what is under the pointer, labels (placement, skip rules, following the
//! view), the badge atlas and badge marks, portal marks, arcs and shard names.

use glam::{Vec2, Vec3};
use overseer::map::{Portal, PortalDestination, RoomCoord, RoomRect, RoomReservation, RoomStatus};

use super::{ME, room, shard, stat, terrain, world_view};
use crate::camera::{room_centre, room_origin};
use crate::mesh::{MarkInstance, arc, mark};
use crate::overlay::badges::{BADGE, Badges, CELLS_PER_LAYER, LEVELS};
use crate::overlay::labels::{GAP, Label, ROOM_TIERS, class};
use crate::overlay::{self, Fades};
use crate::view::View;
use crate::world::{Limits, World};
use crate::{BADGE_PX_PER_ROOM, Scope, TERRAIN_PX_PER_ROOM, ViewMode};

/// Places the camera over `centre` (world tiles) with rooms `px_per_room` CSS pixels wide.
fn place(view: &mut View, centre: Vec2, px_per_room: f32) {
    super::place(view, centre, px_per_room);
    view.update_overlay();
}

#[test]
fn room_at_names_the_room_and_tile_under_the_pointer_in_both_modes() {
    for mode in [ViewMode::TopDown, ViewMode::Perspective] {
        let mut view = world_view();
        view.set_mode(mode);
        // E0S0's north-west corner: four rooms meet there.
        place(&mut view, Vec2::ZERO, 400.0);
        for (point, expected) in [
            // Just inside each room around the corner, and tile centres.
            (Vec2::new(0.2, 0.2), ("E0S0", 0, 0)),
            (Vec2::new(-0.2, 0.2), ("W0S0", 49, 0)),
            (Vec2::new(0.2, -0.2), ("E0N0", 0, 49)),
            (Vec2::new(-0.2, -0.2), ("W0N0", 49, 49)),
            (Vec2::new(3.5, 2.5), ("E0S0", 3, 2)),
            (Vec2::new(-3.5, -2.5), ("W0N0", 46, 47)),
        ] {
            let css = view
                .camera
                .project(Vec3::new(point.x, 0.0, point.y))
                .unwrap();
            let (name, x, y) = expected;
            assert_eq!(
                view.room_at(css),
                Some((room(name), x, y)),
                "{mode:?} at {point}"
            );
            assert_eq!(
                view.pick(css).map(|pick| (pick.room, pick.x, pick.y)),
                view.room_at(css)
            );
        }
    }
    // Outside the scope: nothing.
    let mut view = world_view();
    view.set_scope(Scope::World(RoomRect::new(room("W1N1"), room("E0S0"))));
    view.resize(800.0, 600.0);
    place(&mut view, Vec2::ZERO, 60.0);
    assert_eq!(view.room_at(Vec2::new(1.0, 1.0)), None);
    assert!(view.room_at(Vec2::new(400.0, 300.0)).is_some());
}

/// Whether two labels shown at `px_per_room` come closer than [`GAP`].
fn crowded(a: &Label, b: &Label, px_per_room: f32) -> bool {
    let at = |label: &Label| {
        let shift = label.anchor / 50.0 * px_per_room;
        let [left, top, right, bottom] = label.bounds();
        (
            shift.x + left,
            shift.y + top,
            shift.x + right,
            shift.y + bottom,
        )
    };
    let (a, b) = (at(a), at(b));
    let slack = GAP - 0.01;
    a.0 < b.2 + slack && b.0 < a.2 + slack && a.1 < b.3 + slack && b.1 < a.3 + slack
}

/// Checks that no two of `labels` shown at `px_per_room` crowd each other; returns how many
/// show.
fn assert_apart(labels: &[Label], px_per_room: f32) -> usize {
    let shown: Vec<&Label> = labels
        .iter()
        .filter(|label| label.appear <= px_per_room)
        .collect();
    for (index, a) in shown.iter().enumerate() {
        for b in &shown[index + 1..] {
            if (a.anchor - b.anchor).abs().max_element() > 50.0 * 12.0 {
                continue;
            }
            assert!(
                !crowded(a, b, px_per_room),
                "{} and {} at {px_per_room}",
                a.text,
                b.text
            );
        }
    }
    shown.len()
}

#[test]
fn sector_labels_never_overlap_and_coarser_grids_show_first() {
    let mut view = world_view();
    place(&mut view, Vec2::ZERO, 3.0);
    let labels = view.world.overlay.overview_labels().to_vec();
    // Crossroads W50..W0 and E0..E50 each way.
    assert_eq!(labels.len(), 12 * 12);
    assert!(labels.iter().all(|label| label.class == class::SECTOR));
    let appear = |name: &str| {
        labels
            .iter()
            .find(|label| label.text == name)
            .unwrap()
            .appear
    };
    assert!(appear("E0S0") <= 0.0, "the centre always shows");
    assert!(appear("E40S40") < appear("E10S10"), "coarser grids first");
    assert!(
        appear("W0S0") > TERRAIN_PX_PER_ROOM,
        "next to E0S0: shown only once rooms fit a label"
    );
    let mut previous = 0;
    for px_per_room in [1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 18.0, 24.0] {
        let shown = assert_apart(&labels, px_per_room);
        assert!(shown >= previous, "more labels as rooms grow");
        previous = shown;
    }
    assert!(previous > 100, "{previous} of 144 show at sector size");
}

#[test]
fn room_labels_skip_small_rooms_and_follow_the_view() {
    let mut view = world_view();
    place(&mut view, Vec2::new(100.0, 100.0), 12.0);
    assert!(
        view.world.overlay.room_labels().is_empty(),
        "zoomed out: none"
    );
    place(&mut view, Vec2::new(100.0, 100.0), 26.0);
    let labels = view.world.overlay.room_labels().to_vec();
    assert!(labels.iter().all(|label| label.class == class::ROOM));
    let appear = |name: &str| {
        labels
            .iter()
            .find(|label| label.text == name)
            .map(|label| label.appear)
    };
    // Every second room fits at 26 pixels, odd ones do not.
    assert!(appear("E2S2").unwrap() <= 26.0);
    assert!(appear("E1S2").unwrap() > 26.0);
    assert!(appear("E8S8").unwrap() * 8.0 <= appear("E1S1").unwrap() * 1.01);
    let shown = assert_apart(&labels, 26.0);
    assert!(shown > 50, "{shown}");
    // In its room's north-west corner.
    let label = labels.iter().find(|label| label.text == "E2S2").unwrap();
    assert_eq!(label.anchor, room_origin(room("E2S2")));
    assert!(label.bounds()[0] > 0.0 && label.bounds()[1] > 0.0);
    // Zoomed in, every room in view is named.
    place(&mut view, Vec2::new(100.0, 100.0), 120.0);
    let labels = view.world.overlay.room_labels().to_vec();
    let visible = view.visible_rooms();
    for name in visible.iter() {
        let label = labels
            .iter()
            .find(|label| label.anchor == room_origin(name));
        assert!(
            label.is_some_and(|label| label.appear <= 120.0),
            "{name} named"
        );
    }
    assert_apart(&labels, 120.0);
    // Panning within the margin keeps them; leaving it makes new ones.
    let version = view.world.overlay.room_glyphs.version;
    place(&mut view, Vec2::new(150.0, 100.0), 120.0);
    assert_eq!(view.world.overlay.room_glyphs.version, version);
    place(&mut view, Vec2::new(2000.0, 100.0), 120.0);
    assert_ne!(view.world.overlay.room_glyphs.version, version);
    assert!(
        view.world
            .overlay
            .room_labels()
            .iter()
            .any(|label| label.text == "E40S2")
    );
}

#[test]
fn room_label_grids_coarsen_with_the_zoom_and_count() {
    assert_eq!(overlay::labels::room_tier(200.0, 40.0, 100, 6000), 0);
    assert_eq!(overlay::labels::room_tier(20.0, 40.0, 100, 6000), 1);
    assert_eq!(overlay::labels::room_tier(1.0, 40.0, 100, 6000), ROOM_TIERS);
    assert_eq!(overlay::labels::room_tier(200.0, 40.0, 30_000, 6000), 2);
}

#[test]
fn overlay_fades_follow_the_zoom() {
    let fades = |px_per_room| Fades::at(px_per_room, 0.0);
    assert!(fades(3.0).labels[class::SECTOR as usize] > 0.99);
    assert!(fades(3.0).labels[class::ROOM as usize] <= 0.0);
    assert!(fades(40.0).labels[class::SECTOR as usize] <= 0.0);
    assert!(fades(40.0).labels[class::ROOM as usize] > 0.99);
    assert!(
        fades(BADGE_PX_PER_ROOM * 0.9).marks[0] <= 0.0,
        "badges skip tiny rooms"
    );
    assert!(fades(30.0).marks[0] > 0.99);
    assert!(
        Fades::at(30.0, 1.0).marks[0] <= 0.0,
        "detail scenes show the controller"
    );
    assert!(
        fades(5.0).marks[1] > 0.99,
        "portal runs and arcs zoomed out"
    );
    assert!(fades(150.0).marks[1] <= 0.0, "no runs nor arcs zoomed in");
    assert!(fades(5.0).marks[2] <= 0.0, "no portal tiles zoomed out");
    assert!(
        fades(150.0).marks[2] > 0.99 && Fades::at(1000.0, 1.0).marks[2] > 0.0,
        "portal tiles zoomed in, over detail scenes too"
    );
    let walls = |px_per_room| fades(px_per_room).links[0];
    assert!(
        (walls(3.0) - 0.5).abs() < 1e-3 && walls(40.0) > 0.7 && walls(150.0) <= 0.0,
        "walls calm zoomed out, stronger zoomed in, their tiles take over"
    );
    assert!(
        fades(3.0).links[1] > 0.99 && fades(100.0).links[1] > 0.99 && fades(400.0).links[1] <= 0.0,
        "walls' links (while few are in view) until detail scenes"
    );
    assert!(fades(3.0).links[2] > 0.99 && Fades::at(1000.0, 1.0).links[2] > 0.99);
    let mut view = world_view();
    place(&mut view, Vec2::ZERO, 30.0);
    assert!(view.overlay_fades().marks[0] > 0.0);
    assert_eq!(view.interest().portals, Some(view.visible_rooms()));
    view.set_scope(Scope::Room(room("W1N1")));
    assert_eq!(view.overlay_fades(), Fades::ROOM);
    assert_eq!(
        view.interest().portals,
        Some(RoomRect::new(room("W1N1"), room("W1N1")))
    );
}

/// A `side`² badge of one colour.
fn badge(side: u32, rgba: [u8; 4]) -> Vec<u8> {
    rgba.repeat((side * side) as usize)
}

#[test]
fn badge_atlas_grows_and_keeps_every_cell() {
    let mut badges = Badges::new(Limits::WEBGL2.array_layers);
    assert_eq!((badges.layers(), badges.generation()), (2, 0));
    assert!(
        !badges.set("bad", &[0; 10], 64),
        "bytes not matching the size"
    );
    let capacity = CELLS_PER_LAYER * 2;
    for user in 0..capacity {
        assert!(badges.set(&format!("u{user}"), &badge(64, [9, 9, 9, 255]), 64));
    }
    let first = badges.cell("u0").unwrap();
    let mut uploads = 0;
    badges.flush(|upload| {
        assert_eq!(upload.size.0, BADGE >> upload.level);
        assert_eq!(
            upload.data.len(),
            (upload.size.0 * upload.size.1 * 4) as usize
        );
        uploads += 1;
    });
    assert_eq!(uploads, capacity * LEVELS);
    assert_eq!(badges.generation(), 0);
    // Full: twice the layers, a new texture, everything uploaded again; cells stay.
    assert!(badges.set("more", &badge(32, [1, 2, 3, 255]), 32));
    assert_eq!((badges.layers(), badges.generation()), (4, 1));
    assert_eq!(badges.cell("u0"), Some(first));
    assert_eq!(badges.cell("more").unwrap().1, 2, "in the third layer");
    let mut uploads = 0;
    badges.flush(|_| uploads += 1);
    assert_eq!(uploads, (capacity + 1) * LEVELS);
    // The same badge again uploads nothing; a new one for a user rewrites their cell.
    assert!(!badges.set("u0", &badge(64, [9, 9, 9, 255]), 64));
    badges.flush(|_| panic!("unchanged"));
    assert!(!badges.set("u0", &badge(128, [200, 100, 50, 255]), 128));
    let mut texels = Vec::new();
    badges.flush(|upload| texels.push((upload.level, upload.data.to_vec())));
    assert_eq!(texels.len(), LEVELS as usize);
    for (_, data) in &texels {
        assert!(
            data.chunks(4).all(|texel| texel == [200, 100, 50, 255]),
            "resampled"
        );
    }
    assert_eq!(badges.len(), (capacity + 1) as usize);
    // At the device's limit the atlas stops growing.
    let mut small = Badges::new(2);
    for user in 0..capacity {
        small.set(&format!("u{user}"), &badge(4, [0; 4]), 4);
    }
    assert!(!small.set("late", &badge(4, [0; 4]), 4));
    assert_eq!(small.cell("late"), None);
}

#[test]
fn owners_and_reservers_show_their_badges() {
    let mut world = World::new(shard(), Limits::WEBGL2);
    let frame = overlay::Frame {
        bounds: shard(),
        visible: shard(),
        px_per_room: 10.0,
        world: true,
        selected: None,
    };
    let (owned, reserved, other) = (room("W3N3"), room("W4N3"), room("W5N3"));
    world.set_stat(owned, Some(&stat(RoomStatus::Normal, Some((ME, 4)))));
    let mut reservation = stat(RoomStatus::Normal, None);
    reservation.reservation = Some(RoomReservation {
        id: ME.to_owned(),
        username: None,
        badge: None,
    });
    world.set_stat(reserved, Some(&reservation));
    world.set_stat(other, Some(&stat(RoomStatus::Normal, Some(("other", 2)))));
    world.update_overlay(&frame);
    assert!(world.overlay.marks.items.is_empty(), "no badges yet");
    world
        .overlay
        .set_badge(ME, &badge(64, [0, 128, 0, 255]), 64);
    world.update_overlay(&frame);
    let marks = &world.overlay.marks.items;
    assert_eq!(marks.len(), 2);
    let kind_at = |room: RoomCoord| {
        marks
            .iter()
            .find(|mark| Vec2::from(mark.centre).distance(room_centre(room)) < 1e-3)
            .map(|mark| mark.kind & 255)
    };
    assert_eq!(kind_at(owned), Some(mark::BADGE));
    assert_eq!(kind_at(reserved), Some(mark::RESERVED));
    assert_eq!(kind_at(other), None);
    // A status change keeps the marks; a new owner rebuilds them.
    let version = world.overlay.marks.version;
    world.set_stat(owned, Some(&stat(RoomStatus::Novice, Some((ME, 4)))));
    world.update_overlay(&frame);
    assert_eq!(world.overlay.marks.version, version);
    world.set_stat(owned, Some(&stat(RoomStatus::Normal, Some(("other", 4)))));
    world.update_overlay(&frame);
    assert_eq!(world.overlay.marks.items.len(), 1);
}

fn portal(at: (&str, u8, u8), to: (&str, Option<&str>, Option<(u8, u8)>)) -> Portal {
    Portal {
        room: room(at.0),
        x: at.1,
        y: at.2,
        destination: Some(PortalDestination {
            room: room(to.0),
            shard: to.1.map(str::to_owned),
            x: to.2.map(|tile| tile.0),
            y: to.2.map(|tile| tile.1),
        }),
        expires_at: None,
        decay_time: None,
    }
}

#[test]
fn portals_link_pairs_once_and_name_other_shards() {
    let mut world = World::new(shard(), Limits::WEBGL2);
    let cluster = |name: &'static str, to: &'static str| {
        [(24, 25), (25, 25), (26, 25)].map(|(x, y)| portal((name, x, y), (to, None, Some((x, y)))))
    };
    // W5N5 and E5S5 link both ways; E5N5 leads to a room not scanned yet; W5S5 to another
    // shard.
    world
        .overlay
        .set_portals(room("W5N5"), &cluster("W5N5", "E5S5"));
    world
        .overlay
        .set_portals(room("E5S5"), &cluster("E5S5", "W5N5"));
    world
        .overlay
        .set_portals(room("E5N5"), &cluster("E5N5", "E15N5"));
    world.overlay.set_portals(
        room("W5S5"),
        &[portal(("W5S5", 10, 10), ("W5S5", Some("shard1"), None))],
    );
    let frame = overlay::Frame {
        bounds: shard(),
        visible: shard(),
        px_per_room: 5.0,
        world: true,
        selected: None,
    };
    world.update_overlay(&frame);
    let arcs = &world.overlay.arcs.items;
    assert_eq!(arcs.len(), 2, "{arcs:?}");
    let centre = |name: &str| room_origin(room(name)) + Vec2::new(25.5, 25.5);
    let ends = |from: &str, to: &str| {
        arcs.iter().any(|arc| {
            Vec2::from(arc.from).distance(centre(from)) < 1e-3
                && Vec2::from(arc.to).distance(centre(to)) < 1e-3
        })
    };
    assert!(
        ends("W5N5", "E5S5"),
        "pair linked once, from the lesser room"
    );
    assert!(
        ends("E5N5", "E15N5"),
        "to where an unscanned room's portals arrive"
    );
    let marks = run_marks(&world.overlay.portal_marks.items);
    assert_eq!(marks.len(), 4);
    assert_eq!(tile_marks(&world.overlay.portal_marks.items), 10);
    let shard_marks: Vec<_> = marks
        .iter()
        .filter(|mark| mark.kind == mark::SHARD_PORTAL)
        .collect();
    assert_eq!(shard_marks.len(), 1);
    assert_eq!(
        shard_marks[0].centre,
        (room_origin(room("W5S5")) + Vec2::splat(10.5)).to_array()
    );
    assert!(
        marks
            .iter()
            .all(|mark| mark.radius[0] >= 1.0 && mark.radius[1] > 0.0)
    );
    let names: Vec<&Label> = world
        .overlay
        .overview_labels()
        .iter()
        .filter(|label| label.class == class::SHARD)
        .collect();
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].text, "shard1");
    assert!(names[0].appear <= 0.0, "placed first");
    // Removing a room's portals removes its mark and arc; its partner's arc stays.
    world.overlay.set_portals(room("W5N5"), &[]);
    assert!(!world.overlay.portals.contains(room("W5N5")));
    world.update_overlay(&frame);
    assert_eq!(run_marks(&world.overlay.portal_marks.items).len(), 3);
    assert_eq!(
        world.overlay.arcs.items.len(),
        2,
        "E5S5 still leads to W5N5"
    );
}

/// The marks of portal runs (compact and walls) among `marks`.
fn run_marks(marks: &[MarkInstance]) -> Vec<MarkInstance> {
    marks
        .iter()
        .filter(|mark| matches!(mark.kind, mark::PORTAL | mark::SHARD_PORTAL | mark::WALL))
        .copied()
        .collect()
}

/// How many portal tiles `marks` mark.
fn tile_marks(marks: &[MarkInstance]) -> usize {
    marks
        .iter()
        .filter(|mark| matches!(mark.kind, mark::PORTAL_TILE | mark::SHARD_PORTAL_TILE))
        .count()
}

/// A wall of 48 portals along row `y` of `at` (as on screeps.com's highways), each leading
/// to the tile across the room in `to`.
fn wall(at: &str, to: &str, y: u8) -> Vec<Portal> {
    (1..=48)
        .map(|x| portal((at, x, y), (to, None, Some((x, 49 - y)))))
        .collect()
}

/// The portals of `rooms` (named), and the overlay at a world zoom.
fn portal_world(rooms: &[(String, Vec<Portal>)]) -> World {
    let mut world = World::new(shard(), Limits::WEBGL2);
    for (name, portals) in rooms {
        world.overlay.set_portals(room(name), portals);
    }
    world.update_overlay(&overlay::Frame {
        bounds: shard(),
        visible: shard(),
        px_per_room: 5.0,
        world: true,
        selected: None,
    });
    world
}

#[test]
fn many_portals_in_a_room_make_one_mark_and_one_arc_per_destination() {
    // 48 portals leading to one room, then two walls leading to two rooms.
    let world = portal_world(&[("W5N5".to_owned(), wall("W5N5", "E5S5", 2))]);
    let marks = &world.overlay.portal_marks.items;
    assert_eq!(run_marks(marks).len(), 1);
    assert_eq!(tile_marks(marks), 48);
    assert_eq!(world.overlay.arcs.items.len(), 1);
    let mut both = wall("E31S40", "E31S30", 1);
    both.extend(wall("E31S40", "E31S50", 48));
    let world = portal_world(&[("E31S40".to_owned(), both)]);
    let marks = &world.overlay.portal_marks.items;
    assert_eq!(run_marks(marks).len(), 1);
    assert_eq!(tile_marks(marks), 96);
    let arcs = &world.overlay.arcs.items;
    assert_eq!(arcs.len(), 2, "one per destination");
    let arrival = |name: &str, y: f32| room_origin(room(name)) + Vec2::new(25.0, y);
    for (to, y) in [("E31S30", 48.5), ("E31S50", 1.5)] {
        assert!(
            arcs.iter().any(|arc| [arc.from, arc.to]
                .iter()
                .any(|end| Vec2::from(*end).distance(arrival(to, y)) < 1e-3)),
            "to where the portals to {to} arrive"
        );
    }
}

#[test]
fn portal_highways_make_one_run_per_segment_and_one_arc_per_linked_pair() {
    let mut rooms = Vec::new();
    for index in 1..=9 {
        // A highway segment leading to the parallel one and back (north of it).
        let (south, north) = (format!("W{index}N20"), format!("W{index}N30"));
        rooms.push((south.clone(), wall(&south, &north, 1)));
        rooms.push((north.clone(), wall(&north, &south, 48)));
        // A segment whose rooms lead both ways, to segments not scanned yet.
        let row = format!("E{index}S20");
        let mut both = wall(&row, &format!("E{index}S10"), 1);
        both.extend(wall(&row, &format!("E{index}S30"), 48));
        rooms.push((row, both));
        // A column leading west.
        let column = format!("W20N{index}");
        rooms.push((column.clone(), wall(&column, &format!("W30N{index}"), 5)));
    }
    // Beside the row's east end, two rooms leading to another shard: a run of their own.
    for name in ["E10S20", "E11S20"] {
        rooms.push((
            name.to_owned(),
            [(24, 25), (25, 25), (26, 25)]
                .map(|(x, y)| portal((name, x, y), (name, Some("shard1"), None)))
                .to_vec(),
        ));
    }
    let mut world = portal_world(&rooms);
    let marks = &world.overlay.portal_marks.items;
    assert_eq!(tile_marks(marks), 9 * 48 * 5 + 6, "every tile");
    let runs = run_marks(marks);
    assert_eq!(runs.len(), 5, "{runs:?}");
    let along = |half: [f32; 4]| Vec2::new(half[0], half[1]);
    let rows = runs
        .iter()
        .filter(|mark| along(mark.uv).x.abs() > 150.0 && along(mark.uv).y.abs() < 1e-3)
        .count();
    let columns = runs
        .iter()
        .filter(|mark| along(mark.uv).y.abs() > 150.0 && along(mark.uv).x.abs() < 1e-3)
        .count();
    assert_eq!((rows, columns), (3, 1), "capsules along the segments");
    let shard_runs: Vec<_> = runs
        .iter()
        .filter(|mark| mark.kind == mark::SHARD_PORTAL)
        .collect();
    assert_eq!(shard_runs.len(), 1);
    assert!((along(shard_runs[0].uv).x.abs() - 25.0).abs() < 1e-3);
    let arcs = world.overlay.arcs.items.clone();
    assert_eq!(
        arcs.len(),
        4,
        "N20↔N30 once, S20 both ways, the column: {arcs:?}"
    );
    // The segments' arc joins the middles of their walls.
    let wall_centre = |name: &str, y: f32| room_origin(room(name)) + Vec2::new(25.0, y);
    assert!(arcs.iter().any(|arc| {
        Vec2::from(arc.from).distance(wall_centre("W5N30", 48.5)) < 1e-3
            && Vec2::from(arc.to).distance(wall_centre("W5N20", 1.5)) < 1e-3
    }));
    let names: Vec<&Label> = world
        .overlay
        .overview_labels()
        .iter()
        .filter(|label| label.class == class::SHARD)
        .collect();
    assert_eq!(names.len(), 1, "one shard label per run");
    assert_eq!(names[0].text, "shard1");
    // A segment broken in two: two runs, still one arc to the segment they lead to.
    world.overlay.set_portals(room("W5N20"), &[]);
    world.update_overlay(&overlay::Frame {
        bounds: shard(),
        visible: shard(),
        px_per_room: 5.0,
        world: true,
        selected: None,
    });
    assert_eq!(run_marks(&world.overlay.portal_marks.items).len(), 6);
    assert_eq!(world.overlay.arcs.items.len(), 4);
}

/// A world frame over `visible`, with `selected` selected.
fn frame_over(visible: RoomRect, selected: Option<&str>) -> overlay::Frame {
    overlay::Frame {
        bounds: shard(),
        visible,
        px_per_room: 5.0,
        world: true,
        selected: selected.map(room),
    }
}

#[test]
fn walls_and_compact_runs_are_told_apart_by_how_their_portals_line_up() {
    let to = |at: &str, x: u8, y: u8| portal((at, x, y), ("E40S40", None, Some((x, y))));
    let spread = |at: &'static str, count: u8| -> Vec<Portal> {
        (0..count).map(|index| to(at, 10 + 2 * index, 20)).collect()
    };
    let cluster: Vec<Portal> = (0..9_u8)
        .filter(|index| *index != 4)
        .map(|index| to("W5N5", 24 + index % 3, 24 + index / 3))
        .collect();
    let crossroad = [(15, 14), (33, 33)]
        .map(|(x, y)| portal(("W10N10", x, y), ("W10N10", Some("shard1"), None)))
        .to_vec();
    let column = (1..=48).map(|y| to("W50N50", 3, y)).collect();
    let world = portal_world(&[
        ("W5N5".to_owned(), cluster),
        ("W10N10".to_owned(), crossroad),
        ("W20N20".to_owned(), wall("W20N20", "W20N30", 2)),
        ("W30N30".to_owned(), spread("W30N30", 12)),
        ("W40N40".to_owned(), spread("W40N40", 11)),
        ("W50N50".to_owned(), column),
    ]);
    let marks = run_marks(&world.overlay.portal_marks.items);
    let at = |name: &str| {
        let (origin, end) = (
            room_origin(room(name)),
            room_origin(room(name)) + Vec2::splat(50.0),
        );
        marks
            .iter()
            .find(|mark| {
                let centre = Vec2::from(mark.centre);
                centre.cmpge(origin).all() && centre.cmplt(end).all()
            })
            .copied()
            .unwrap()
    };
    assert_eq!(at("W5N5").kind, mark::PORTAL, "a sector centre's cluster");
    assert_eq!(
        at("W10N10").kind,
        mark::SHARD_PORTAL,
        "far apart, but two portals"
    );
    assert_eq!(
        at("W40N40").kind,
        mark::PORTAL,
        "too few portals for a wall"
    );
    for name in ["W20N20", "W30N30", "W50N50"] {
        let wall = at(name);
        assert_eq!(wall.kind, mark::WALL, "{name}");
        assert!(wall.radius[2] <= 2.0, "{name}: a thin line at every zoom");
    }
    // Along the longer side, through the middle of the portals.
    let row = at("W20N20");
    assert_eq!([row.uv[0], row.uv[1]], [23.5, 0.0]);
    assert_eq!(row.centre[1], room_origin(room("W20N20")).y + 2.5);
    let column = at("W50N50");
    assert_eq!([column.uv[0], column.uv[1]], [0.0, 23.5]);
    assert_eq!(column.centre[0], room_origin(room("W50N50")).x + 3.5);
    // The crossroad's portals to another shard keep their label.
    let names: Vec<&str> = world
        .overlay
        .overview_labels()
        .iter()
        .filter(|label| label.class == class::SHARD)
        .map(|label| label.text.as_str())
        .collect();
    assert_eq!(names, ["shard1"]);
}

#[test]
fn walls_links_show_while_few_are_in_view_and_for_the_focused_rooms() {
    // Eight highway segments of three rooms lined with walls, each linked both ways with
    // the parallel segment ten rooms north; a sector centre linked with another.
    let mut rooms = Vec::new();
    for segment in 0..8 {
        for index in 1..=3 {
            let x = segment * 6 + index;
            let (south, north) = (format!("W{x}N20"), format!("W{x}N30"));
            rooms.push((south.clone(), wall(&south, &north, 1)));
            rooms.push((north.clone(), wall(&north, &south, 48)));
        }
    }
    let cluster = |at: &'static str, to: &'static str| {
        [(24, 25), (25, 25), (26, 25)].map(|(x, y)| portal((at, x, y), (to, None, Some((x, y)))))
    };
    rooms.push(("W5N5".to_owned(), cluster("W5N5", "E5S5").to_vec()));
    rooms.push(("E5S5".to_owned(), cluster("E5S5", "W5N5").to_vec()));
    let mut world = portal_world(&rooms);
    let kinds = |world: &World| {
        let mut kinds: Vec<u32> = world
            .overlay
            .arcs
            .items
            .iter()
            .map(|arc| arc.kind)
            .collect();
        kinds.sort_unstable();
        kinds
    };
    assert_eq!(
        kinds(&world),
        [arc::LINK],
        "eight walls' links in view: only the sector centres'"
    );
    let version = world.overlay.arcs.version;
    world.update_overlay(&frame_over(shard(), None));
    assert_eq!(world.overlay.arcs.version, version, "nothing changed");
    // Zoomed in on one segment: its link shows.
    let segment = RoomRect::new(room("W3N30"), room("W1N20"));
    world.update_overlay(&frame_over(segment, None));
    assert_eq!(kinds(&world), [arc::LINK, arc::WALL]);
    let wall_arc = world.overlay.arcs.items[1];
    assert!(
        Vec2::from(wall_arc.from).distance(room_origin(room("W2N30")) + Vec2::new(25.0, 48.5))
            < 1e-3
    );
    // Zoomed out again, hovering a room at either end of a segment's link shows it,
    // brighter.
    for hovered in ["W8N20", "W9N30"] {
        world.overlay.set_hover(Some(room(hovered)));
        world.update_overlay(&frame_over(shard(), None));
        assert_eq!(kinds(&world), [arc::LINK, arc::FOCUS], "{hovered}");
        let focus = world.overlay.arcs.items[1];
        assert!(focus.color[3] > world.overlay.arcs.items[0].color[3]);
        let x = Vec2::from(focus.to).x;
        assert!(x > room_origin(room("W9N20")).x && x < room_origin(room("W6N20")).x);
    }
    // So does selecting one; a selected compact run's link is drawn again on top.
    world.overlay.set_hover(None);
    world.update_overlay(&frame_over(shard(), Some("W14N30")));
    assert_eq!(kinds(&world), [arc::LINK, arc::FOCUS]);
    world.update_overlay(&frame_over(shard(), Some("E5S5")));
    assert_eq!(kinds(&world), [arc::LINK, arc::FOCUS]);
    // A room with no portals focuses nothing.
    world.update_overlay(&frame_over(shard(), Some("E50S50")));
    assert_eq!(kinds(&world), [arc::LINK]);
}

#[test]
fn hover_outlines_a_room_of_the_scope() {
    let mut view = world_view();
    view.set_hover(Some(room("W1N1")));
    let hover = view.world.overlay.hover(view.world.bounds()).unwrap();
    assert_eq!(hover.centre, room_centre(room("W1N1")).to_array());
    assert_eq!(hover.kind, mark::HOVER);
    view.set_hover(Some(room("W90N90")));
    assert_eq!(
        view.world.overlay.hover(view.world.bounds()),
        None,
        "outside"
    );
    view.set_hover(None);
    assert_eq!(view.world.overlay.hover(view.world.bounds()), None);
    // Labels are drawn for rooms with terrain or pending; rooms outside the world get none.
    view.world.set_terrain(room("E2S2"), &terrain());
    view.world.mark_no_terrain(room("E4S2"));
    place(&mut view, Vec2::new(100.0, 100.0), 120.0);
    let labels = view.world.overlay.room_labels();
    assert!(labels.iter().any(|label| label.text == "E2S2"));
    assert!(labels.iter().all(|label| label.text != "E4S2"));
}

#[test]
fn pending_destinations_are_wanted_focus_first_then_runs_in_view_then_few_walls() {
    let mut world = World::new(shard(), Limits::WEBGL2);
    // A pending cluster in a sector centre, a known one, and a pending wall along a highway.
    let cluster =
        |name: &str| [(24, 25), (25, 25), (26, 25)].map(|(x, y)| Portal::pending(room(name), x, y));
    world.overlay.set_portals(room("W5N5"), &cluster("W5N5"));
    world.overlay.set_portals(room("E5N5"), &cluster("E5N5"));
    world.overlay.set_portals(
        room("E5S5"),
        &[portal(("E5S5", 25, 25), ("W5N5", None, Some((25, 25))))],
    );
    let wall: Vec<Portal> = (1..=40)
        .map(|x| Portal::pending(room("W10N3"), x, 0))
        .collect();
    world.overlay.set_portals(room("W10N3"), &wall);
    let all = RoomRect::new(room("W19N19"), room("E19S19"));
    // Few walls in view: compact runs nearest the centre, then the wall; the known room is
    // not wanted.
    let wanted = world.overlay.wanted_destinations(all, None);
    assert_eq!(wanted, [room("W5N5"), room("E5N5"), room("W10N3")]);
    // The hovered and the selected room first.
    world.overlay.set_hover(Some(room("W10N3")));
    assert_eq!(
        world.overlay.wanted_destinations(all, Some(room("W5N5"))),
        [room("W10N3"), room("W5N5"), room("E5N5")]
    );
    world.overlay.set_hover(None);
    // Many walls in view: no wall is wanted (their arcs do not show), compact runs still are.
    for row in [13, 23, 33, 43, 53, 63] {
        let wall: Vec<Portal> = (1..=40)
            .map(|x| Portal::pending(room(&format!("W10N{row}")), x, 0))
            .collect();
        world
            .overlay
            .set_portals(room(&format!("W10N{row}")), &wall);
    }
    let wide = RoomRect::new(room("W79N79"), room("E79S79"));
    let wanted = world.overlay.wanted_destinations(wide, None);
    assert_eq!(wanted.len(), 2);
    assert!(!wanted.contains(&room("W10N3")));
    // Out of view: nothing.
    let away = RoomRect::new(room("E30S30"), room("E39S39"));
    assert_eq!(world.overlay.wanted_destinations(away, None), []);
}
