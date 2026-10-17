//! Smoothed terrain outlines: the cell regions against their signed distance, continuity
//! from cell to cell, and the raised-wall mesh of whole rooms.

use glam::{Vec2, Vec3};
use overseer::model::world::Terrain;

use crate::mesh::contour::{ARC_SEGMENTS, CORNERS, Cell, RADIUS, cell_sd, walls};
use crate::mesh::{Mesh, TerrainVertex, WALL_HEIGHT};

/// Every corner combination of a cell (NW, NE, SW, SE).
fn combinations() -> impl Iterator<Item = [bool; 4]> {
    (0..16_u8).map(|bits| [0, 1, 2, 3].map(|bit| bits >> bit & 1 == 1))
}

#[expect(
    clippy::many_single_char_names,
    reason = "a point and a triangle's corners"
)]
fn inside_triangle(p: Vec2, [a, b, c]: [Vec2; 3]) -> bool {
    let side = |a: Vec2, b: Vec2| (b - a).perp_dot(p - a);
    // Points on a shared edge count for both triangles.
    let (x, y, z) = (side(a, b), side(b, c), side(c, a));
    let e = 1e-6;
    (x >= -e && y >= -e && z >= -e) || (x <= e && y <= e && z <= e)
}

fn triangle_area([a, b, c]: [Vec2; 3]) -> f32 {
    (b - a).perp_dot(c - a).abs() / 2.0
}

/// A cell sample grid, `steps + 1` points a side.
fn samples(steps: u16) -> impl Iterator<Item = Vec2> {
    (0..=steps).flat_map(move |y| {
        (0..=steps).map(move |x| Vec2::new(f32::from(x), f32::from(y)) / f32::from(steps))
    })
}

#[test]
fn every_cell_region_matches_its_signed_distance() {
    for inside in combinations() {
        let cell = Cell::of(inside);
        for q in samples(60) {
            let sd = cell_sd(inside, q);
            if sd.abs() < 0.01 {
                continue;
            }
            let covered = cell.triangles.iter().any(|t| inside_triangle(q, *t));
            assert_eq!(covered, sd < 0.0, "{inside:?} at {q}: sd {sd}");
        }
        // The outline is the zero set, its normals point out of the region.
        for edge in &cell.edges {
            for (point, normal) in edge.points.iter().zip(edge.normals) {
                assert!(cell_sd(inside, *point).abs() < 1e-4, "{inside:?}: {point}");
                assert!(
                    cell_sd(inside, *point + normal * 0.05) > 0.0,
                    "{inside:?}: normal {normal} at {point}"
                );
                assert!((normal.length() - 1.0).abs() < 1e-4);
            }
        }
    }
    // A lone corner: a quarter disc of radius ½.
    let quarter: f32 = Cell::of([true, false, false, false])
        .triangles
        .into_iter()
        .map(triangle_area)
        .sum();
    let exact = std::f32::consts::PI * RADIUS * RADIUS / 4.0;
    assert!(
        (quarter - exact).abs() / exact < 0.01,
        "{quarter} vs {exact}"
    );
}

#[test]
fn shapes_continue_from_cell_to_cell() {
    // A cell's east side (NE, SE) is the next cell's west side (NW, SW).
    for left in combinations() {
        for right in combinations() {
            if (left[1], left[3]) != (right[0], right[2]) {
                continue;
            }
            for step in 0..=20_u8 {
                let y = f32::from(step) / 20.0;
                let a = cell_sd(left, Vec2::new(1.0, y));
                let b = cell_sd(right, Vec2::new(0.0, y));
                assert_eq!(
                    a < 0.0,
                    b < 0.0,
                    "{left:?} | {right:?} at y {y}: {a} vs {b}"
                );
                if a.abs().min(b.abs()) < 0.45 {
                    assert!(
                        (a - b).abs() < 1e-5,
                        "{left:?} | {right:?} at y {y}: {a} vs {b}"
                    );
                }
            }
        }
    }
    assert_eq!(CORNERS[3], Vec2::ONE);
}

/// A room of plains with walls on `tiles`.
fn terrain_of(tiles: &[(u8, u8)]) -> Terrain {
    let digits: String = (0..2500_u16)
        .map(|index| {
            let tile = (
                u8::try_from(index % 50).unwrap(),
                u8::try_from(index / 50).unwrap(),
            );
            if tiles.contains(&tile) { '1' } else { '0' }
        })
        .collect();
    Terrain::decode(&digits).unwrap()
}

/// The wall tops of a mesh, as ground triangles.
fn tops(mesh: &Mesh<TerrainVertex>) -> Vec<[Vec2; 3]> {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|triangle| {
            let vertices = [0, 1, 2].map(|i| mesh.vertices[triangle[i] as usize]);
            vertices
                .iter()
                .all(|v| {
                    (v.position[1] - WALL_HEIGHT).abs() < 1e-6 && Vec3::from(v.normal) == Vec3::Y
                })
                .then(|| vertices.map(|v| Vec2::new(v.position[0], v.position[2])))
        })
        .collect()
}

fn covered(tops: &[[Vec2; 3]], point: Vec2) -> bool {
    tops.iter()
        .any(|triangle| inside_triangle(point, *triangle))
}

#[test]
fn a_lone_wall_is_a_disc_and_blocks_keep_straight_sides() {
    let lone = walls(&terrain_of(&[(10, 10)]));
    let top = tops(&lone);
    let area: f32 = top.iter().copied().map(triangle_area).sum();
    let disc = std::f32::consts::PI / 4.0;
    assert!((area - disc).abs() / disc < 0.01, "{area}");
    assert!(covered(&top, Vec2::new(10.5, 10.5)));
    assert!(
        covered(&top, Vec2::new(10.5, 10.02)),
        "the tile's side midpoints"
    );
    assert!(
        !covered(&top, Vec2::new(10.05, 10.05)),
        "corners are rounded"
    );
    // Sides: four quarter arcs, outward, from the ground to the top.
    let sides = (lone.indices.len() - top.len() * 3) / 6;
    assert_eq!(sides, 4 * usize::from(ARC_SEGMENTS));
    for vertex in &lone.vertices {
        if vertex.normal[1] == 0.0 {
            let out = Vec3::from(vertex.normal);
            let from = Vec3::from(vertex.position) - Vec3::new(10.5, vertex.position[1], 10.5);
            assert!(out.dot(from) > 0.0, "outward");
        }
    }

    let block: Vec<(u8, u8)> = (20..23)
        .flat_map(|y| (20..23).map(move |x| (x, y)))
        .collect();
    let top = tops(&walls(&terrain_of(&block)));
    let area: f32 = top.iter().copied().map(triangle_area).sum();
    let corners = 4.0 * (0.25 - std::f32::consts::PI / 16.0);
    assert!((area - (9.0 - corners)).abs() < 0.02, "{area}");
    assert!(
        covered(&top, Vec2::new(21.5, 20.01)),
        "straight sides stay on the tile edge"
    );
    assert!(
        !covered(&top, Vec2::new(20.03, 20.03)),
        "outer corners rounded"
    );
    assert!(covered(&top, Vec2::new(21.0, 21.0)), "inside");
}

#[test]
fn inner_corners_are_filleted() {
    // An L: (30, 30), (31, 30), (30, 31); the inner corner faces (31, 31).
    let top = tops(&walls(&terrain_of(&[(30, 30), (31, 30), (30, 31)])));
    assert!(
        covered(&top, Vec2::new(31.03, 31.03)),
        "the fillet fills the inner corner"
    );
    assert!(
        !covered(&top, Vec2::new(31.3, 31.3)),
        "within half a tile of (31, 31)"
    );
}

#[test]
fn walls_on_the_room_edge_reach_it() {
    let column: Vec<(u8, u8)> = (0..50).map(|y| (0, y)).collect();
    let mesh = walls(&terrain_of(&column));
    let top = tops(&mesh);
    let area: f32 = top.iter().copied().map(triangle_area).sum();
    assert!((area - 50.0).abs() < 1e-3, "a straight band: {area}");
    assert!(covered(&top, Vec2::new(0.01, 0.01)));
    assert!(covered(&top, Vec2::new(0.99, 49.99)));
    // The room edge has sides too (facing west).
    assert!(
        mesh.vertices
            .iter()
            .any(|v| Vec3::from(v.normal) == Vec3::NEG_X && v.position[0].abs() < 1e-6)
    );
    // The test room's walls: the band x < 5, straight along the room's edges.
    let top = tops(&walls(&super::terrain()));
    let area: f32 = top.iter().copied().map(triangle_area).sum();
    assert!((area - 250.0).abs() < 1e-2, "{area}");
}
