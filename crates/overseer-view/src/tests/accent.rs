//! The status and owner accent over full-resolution terrain: subtle at the terrain and
//! minimap levels (terrain contrast kept), gone at the detail level and in a room scope, the
//! same for every kind of room.

use glam::Vec3;
use overseer::map::{RoomReservation, RoomStatus};

use super::{ME, room, shard, stat, view, world_view};
use crate::mesh::{self, PLAIN, SWAMP, WALL_TOP};
use crate::view::Fades;
use crate::world::cells::accent;
use crate::world::{Limits, World};
use crate::{DETAIL_PX_PER_ROOM, MINIMAP_PX_PER_ROOM, TERRAIN_PX_PER_ROOM, ViewMode};

fn linear(rgb: u32) -> Vec3 {
    let [r, g, b, _] = mesh::linear(rgb);
    Vec3::new(r, g, b)
}

fn luminance(color: Vec3) -> f32 {
    color.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

#[test]
fn the_accent_shows_until_detail_and_fades_out_with_it() {
    for px_per_room in [
        TERRAIN_PX_PER_ROOM,
        TERRAIN_PX_PER_ROOM * 1.5,
        MINIMAP_PX_PER_ROOM,
        MINIMAP_PX_PER_ROOM * 1.5,
        DETAIL_PX_PER_ROOM,
    ] {
        assert_eq!(Fades::at(px_per_room).accent, 1.0, "{px_per_room} px/room");
    }
    let mut last = 1.0;
    let mut px_per_room = DETAIL_PX_PER_ROOM;
    while px_per_room <= DETAIL_PX_PER_ROOM * 1.25 {
        let fades = Fades::at(px_per_room);
        assert!(fades.accent <= last, "the accent only fades out");
        assert!(
            (fades.accent + fades.detail - 1.0).abs() < 1e-6,
            "it gives way to the detail scenes"
        );
        last = fades.accent;
        px_per_room += 5.0;
    }
    assert_eq!(Fades::at(DETAIL_PX_PER_ROOM * 1.25).accent, 0.0);
    assert_eq!(Fades::at(5000.0).accent, 0.0);
    // A room scope (room control, pop-out) never shows it.
    assert_eq!(view(ViewMode::TopDown).fades().accent, 0.0);
    assert_eq!(world_view().fades().accent, 1.0, "the fitted shard");
}

#[test]
fn accents_keep_the_terrain_readable_for_every_kind_of_room() {
    let mut world = World::new(shard(), Limits::WEBGL2);
    world.set_user(Some(ME.to_owned()));
    let reserved = |user: &str| {
        let mut stat = stat(RoomStatus::Normal, None);
        stat.reservation = Some(RoomReservation {
            id: user.to_owned(),
            username: None,
        });
        stat
    };
    let rooms = [
        ("own", stat(RoomStatus::Normal, Some((ME, 8)))),
        ("other", stat(RoomStatus::Normal, Some(("other", 3)))),
        ("invader", stat(RoomStatus::Normal, Some(("2", 1)))),
        ("keeper", stat(RoomStatus::Normal, Some(("3", 1)))),
        ("reserved by me", reserved(ME)),
        ("reserved", reserved("other")),
        ("novice", stat(RoomStatus::Novice, None)),
        ("respawn", stat(RoomStatus::Respawn, None)),
        ("closed", stat(RoomStatus::Closed, None)),
        ("unowned", stat(RoomStatus::Normal, None)),
    ];
    let (plain, swamp, wall) = (linear(PLAIN), linear(SWAMP), linear(WALL_TOP));
    for (index, (name, stat)) in rooms.iter().enumerate() {
        let coord = room(&format!("E{index}S0"));
        world.set_stat(coord, Some(stat));
        let tint = world.tint(coord);
        let bounds = shard();
        let index =
            (coord.y - bounds.min.y) * (bounds.max.x - bounds.min.x + 1) + (coord.x - bounds.min.x);
        let instance = world.cells.instances()[usize::try_from(index).unwrap()];
        assert_eq!(
            instance.tint, tint,
            "{name}: detail walls share the floor's tint"
        );
        let tinted = |color: Vec3| accent(color, tint, 1.0);
        // Contrast between walls, swamps and plains: what makes terrain read.
        let ratio = |a: Vec3, b: Vec3| luminance(a) / luminance(b);
        for (a, b) in [(plain, wall), (swamp, plain), (swamp, wall)] {
            let (before, after) = (ratio(a, b), ratio(tinted(a), tinted(b)));
            assert!(
                (after / before - 1.0).abs() < 0.02,
                "{name}: contrast {before} became {after}"
            );
        }
        if *name == "unowned" {
            assert_eq!(tint[3], 0);
            assert_eq!(tinted(plain), plain, "no accent");
        } else {
            assert_ne!(tinted(plain), plain, "{name}: an accent shows");
        }
        if *name == "closed" {
            assert!(
                luminance(tinted(plain)) < luminance(plain) * 0.6,
                "closed rooms dim"
            );
        } else {
            // Coloured accents shift the hue without darkening or washing out the terrain.
            let change = luminance(tinted(plain)) / luminance(plain);
            assert!(
                (0.85..=1.15).contains(&change),
                "{name}: brightness × {change}"
            );
        }
        // At the detail level (and in a room scope) the terrain is exactly untinted.
        for color in [plain, swamp, wall] {
            assert_eq!(accent(color, tint, 0.0), color, "{name}");
        }
        // Zoomed out the overlay is the stronger signal.
        assert!(
            instance.overlay[3] >= instance.tint[3] || *name == "unowned",
            "{name}"
        );
    }
}
