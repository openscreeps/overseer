use super::{CheckSchedule, Exemption};
use crate::map::WorldProgress;

/// The checks `schedule` starts each second from `from` to `to` (exclusive), each finishing
/// at once as not exempt, with loading `held_back` or not.
fn checks(schedule: &mut CheckSchedule, from: u64, to: u64, held_back: bool) -> Vec<u64> {
    let mut started = Vec::new();
    for now in (from..to).step_by(1000) {
        if schedule.due(now, 0, false, held_back) {
            started.push(now);
            schedule.finished(now, None);
        }
    }
    started
}

#[test]
fn the_exemption_is_checked_at_start_then_every_minute_only_while_loading_waits() {
    let start = 1_700_000_000_000;
    let mut schedule = CheckSchedule::new(0);
    assert!(!schedule.settled());
    // Held back: at once, then every 60 s.
    assert_eq!(
        checks(&mut schedule, start, start + 150_000, true),
        [start, start + 60_000, start + 120_000]
    );
    assert!(schedule.settled());
    // Idle: nothing, however long.
    let idle = checks(&mut schedule, start + 150_000, start + 3_600_000, false);
    assert_eq!(idle, Vec::<u64>::new());
    // Waiting again: at once (the latest check is long past), then every 60 s.
    assert_eq!(
        checks(&mut schedule, start + 3_600_000, start + 3_700_000, true),
        [start + 3_600_000, start + 3_660_000]
    );
    // Exempt: no minute checks, only when the exemption says (now and then, and at its end).
    let mut schedule = CheckSchedule::new(0);
    assert!(schedule.due(start, 0, false, true));
    let until = start + 7_200_000;
    schedule.finished(start, Some(until));
    for now in (start..start + 900_000).step_by(1000) {
        assert!(!schedule.due(now, 0, true, true), "{now}");
    }
    assert!(schedule.due(start + 900_000, 0, true, true));
    schedule.finished(start + 900_000, Some(start + 960_000));
    assert!(!schedule.due(start + 959_000, 0, true, true));
    assert!(schedule.due(start + 960_000, 0, true, true));
}

#[test]
fn attention_checks_at_most_every_15_s() {
    let start = 1_700_000_000_000;
    let mut schedule = CheckSchedule::new(0);
    assert!(schedule.due(start, 0, false, false));
    schedule.finished(start + 300, None);
    // Focus 5 s after the check: waits until 15 s after it.
    schedule.attention();
    assert_eq!(
        checks(&mut schedule, start + 5_000, start + 30_000, false),
        [start + 15_000]
    );
    // Once per attention: more focus within 15 s makes one check.
    schedule.attention();
    schedule.attention();
    assert_eq!(
        checks(&mut schedule, start + 30_000, start + 60_000, false),
        [start + 30_000]
    );
    // Attention while a check is in flight: one more after the throttle.
    schedule.request();
    assert!(schedule.due(start + 100_000, 0, false, false));
    schedule.attention();
    assert!(!schedule.due(start + 101_000, 0, false, false));
    schedule.finished(start + 101_000, None);
    assert_eq!(
        checks(&mut schedule, start + 101_000, start + 130_000, false),
        [start + 115_000]
    );
    // While exempt too (the exemption may have been extended).
    schedule.finished(start + 130_000, Some(start + 7_200_000));
    schedule.attention();
    assert!(schedule.due(start + 145_000, 0, true, false));
}

#[test]
fn asks_and_refusals_check_once_at_a_time() {
    let start = 1_700_000_000_000;
    let mut schedule = CheckSchedule::new(3);
    assert!(schedule.due(start, 3, false, false));
    // One in flight: nothing else starts, whatever is asked.
    schedule.request();
    schedule.attention();
    assert!(!schedule.due(start + 20_000, 4, false, true));
    assert!(!schedule.due(start + 90_000, 5, false, true));
    assert!(!schedule.settled());
    // Asked at once: due as soon as the check in flight finished.
    schedule.finished(start + 90_000, None);
    assert!(schedule.settled());
    assert!(schedule.due(start + 90_000, 5, false, false));
    schedule.finished(start + 90_500, None);
    // A new HTTP 429 reply: 30 s after the latest check; counts already seen do nothing.
    assert!(!schedule.due(start + 100_000, 5, false, false));
    assert!(!schedule.due(start + 100_000, 6, false, false));
    assert!(!schedule.due(start + 119_000, 6, false, false));
    assert!(schedule.due(start + 120_000, 6, false, false));
    schedule.finished(start + 120_000, None);
    let quiet = checks(&mut schedule, start + 121_000, start + 600_000, false);
    assert_eq!(quiet, Vec::<u64>::new());
}

#[test]
fn an_exemption_counts_until_its_end() {
    let exemption = Exemption {
        until: Some(10),
        settled: true,
    };
    assert_eq!(exemption.until_after(9), Some(10));
    assert_eq!(exemption.until_after(10), None);
    assert_eq!(Exemption::default().until_after(0), None);
}

#[test]
fn loading_waits_on_the_budget_while_anything_is_missing_or_paused() {
    let complete = WorldProgress {
        terrain_known: 4,
        terrain_wanted: 4,
        stats_known: 4,
        stats_wanted: 4,
        portals_scanned: 4,
        portals_wanted: 4,
        ..WorldProgress::default()
    };
    assert!(!complete.waits_on_budget());
    assert!(!WorldProgress::default().waits_on_budget());
    for waiting in [
        WorldProgress {
            terrain_known: 3,
            ..complete
        },
        WorldProgress {
            stats_known: 3,
            ..complete
        },
        WorldProgress {
            portals_scanned: 3,
            ..complete
        },
        WorldProgress {
            portals_pending: 1,
            ..complete
        },
        WorldProgress {
            stats_paused_until: Some(1),
            ..complete
        },
    ] {
        assert!(waiting.waits_on_budget(), "{waiting:?}");
    }
}
