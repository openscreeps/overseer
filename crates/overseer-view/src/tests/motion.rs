//! Tick timing: the tick duration measured from snapshot arrivals, and glides that arrive
//! just before the next tick.

use glam::Vec2;
use overseer::RoomSnapshot;
use overseer::map::{RoomCoord, RoomRect};

use super::{ME, creep, room, snapshot, terrain};
use crate::scene::motion::{DEFAULT_TICK_MS, GLIDE_SHARE, TickClock, ease, progress};
use crate::world::{Limits, World};
use crate::{MAX_TICK_MS, MIN_TICK_MS, MOVE_DURATION_MS};

fn feed(clock: &mut TickClock, room: RoomCoord, ticks: &[(u64, f64)]) {
    for &(time, now) in ticks {
        clock.observe(room, Some(time), now);
    }
}

#[test]
fn the_tick_is_the_median_of_recent_intervals() {
    let w1n1 = room("W1N1");
    let mut clock = TickClock::default();
    assert_eq!(clock.tick_ms(), DEFAULT_TICK_MS, "before any interval");
    // About three seconds a tick, with jitter and one late snapshot.
    feed(
        &mut clock,
        w1n1,
        &[
            (100, 0.0),
            (101, 3100.0),
            (102, 6000.0),
            (103, 9050.0),
            (104, 15_000.0),
            (105, 18_000.0),
        ],
    );
    let tick = clock.tick_ms();
    assert!((2900.0..=3100.0).contains(&tick), "{tick}");
    assert!((clock.glide_ms() - tick * GLIDE_SHARE).abs() < 1e-9);
}

#[test]
fn skipped_ticks_divide_and_gaps_repeats_and_other_rooms_do_not_mislead() {
    let (a, b) = (room("W1N1"), room("W2N1"));
    let mut clock = TickClock::default();
    // Two ticks apart: half the interval each.
    feed(&mut clock, a, &[(10, 0.0), (12, 1000.0)]);
    assert_eq!(clock.tick_ms(), 500.0);
    // The same tick again and an earlier one say nothing.
    feed(&mut clock, a, &[(12, 1100.0), (11, 1200.0)]);
    assert_eq!(clock.tick_ms(), 500.0);
    // A long gap (a lost connection) says nothing either.
    feed(&mut clock, a, &[(40, 30_000.0)]);
    assert_eq!(clock.tick_ms(), 500.0);
    // Rooms are measured apart: interleaved snapshots of two rooms.
    feed(&mut clock, b, &[(40, 30_010.0), (41, 30_510.0)]);
    feed(&mut clock, a, &[(41, 30_500.0)]);
    assert_eq!(clock.tick_ms(), 500.0);
    clock.forget(a);
    feed(&mut clock, a, &[(42, 90_000.0)]);
    assert_eq!(clock.tick_ms(), 500.0, "a forgotten room starts over");
}

#[test]
fn the_tick_stays_within_sensible_bounds() {
    let w1n1 = room("W1N1");
    let mut fast = TickClock::default();
    feed(&mut fast, w1n1, &[(1, 0.0), (2, 5.0), (3, 10.0)]);
    assert_eq!(fast.tick_ms(), MIN_TICK_MS);
    assert_eq!(
        fast.glide_ms(),
        MOVE_DURATION_MS,
        "never quicker than the shortest glide"
    );
    let mut slow = TickClock::default();
    feed(&mut slow, w1n1, &[(1, 0.0), (2, 60_000.0)]);
    assert_eq!(slow.tick_ms(), MAX_TICK_MS);
}

#[test]
fn easing_starts_and_ends_at_rest() {
    assert_eq!(ease(0.0), 0.0);
    assert_eq!(ease(0.5), 0.5);
    assert_eq!(ease(1.0), 1.0);
    assert!(ease(0.05) < 0.05 && ease(0.95) > 0.95, "slow at both ends");
    assert_eq!(progress(500.0, 0.0, 1000.0), 0.5);
    assert_eq!(progress(-1.0, 0.0, 1000.0), 0.0);
    assert_eq!(progress(5.0, f64::NEG_INFINITY, 1000.0), 1.0);
    assert_eq!(progress(5.0, 0.0, 0.0), 1.0);
}

fn tick(time: u64, x: u8) -> RoomSnapshot {
    let mut snapshot = snapshot(&terrain(), vec![creep("c", x, 10, ME)]);
    snapshot.game_time = Some(time);
    snapshot
}

#[test]
fn creeps_glide_over_most_of_the_measured_tick() {
    let mut world = World::new(RoomRect::new(room("W1N1"), room("W1N1")), Limits::WEBGL2);
    let w1n1 = room("W1N1");
    // Ticks every two seconds; the creep waits, then moves one tile east.
    for (index, time) in (1..=5).enumerate() {
        #[expect(clippy::cast_precision_loss, reason = "small numbers")]
        world.set_room(&tick(time, 10), index as f64 * 2000.0);
    }
    assert!((world.tick_ms() - 2000.0).abs() < 1e-9);
    let arrival = 10_000.0;
    world.set_room(&tick(6, 11), arrival);
    let glide = 2000.0 * GLIDE_SHARE;
    let x = |world: &World, now: f64| world.detail(w1n1).unwrap().scene.drawn("c", now).unwrap().0;
    assert_eq!(x(&world, arrival), Vec2::new(10.0, 10.0));
    assert_eq!(
        x(&world, arrival + glide / 2.0),
        Vec2::new(10.5, 10.0),
        "halfway at half the glide"
    );
    let before = x(&world, arrival + glide - 50.0).x;
    assert!(before > 10.99 && before < 11.0, "still arriving: {before}");
    assert_eq!(x(&world, arrival + glide), Vec2::new(11.0, 10.0), "arrived");
    assert!(arrival + glide < arrival + 2000.0, "before the next tick");
}
