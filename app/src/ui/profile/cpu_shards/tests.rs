use std::collections::BTreeMap;

use super::{
    Blocker, COOLDOWN_MS, LEFT, LEFT_DONE, LEFT_OVER, Step, allocation, cpu_of, field_text, left,
    next_change, over_cap, scale, shard_max, stepped, total, typed,
};

/// The fields of `pairs` (shard, text).
fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(shard, text)| ((*shard).to_owned(), (*text).to_owned()))
        .collect()
}

/// The field texts of `pairs` (shard, text), by shard.
fn drafts(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    fields(pairs).into_iter().collect()
}

/// One hour, in milliseconds.
const HOUR: f64 = 60.0 * 60.0 * 1000.0;

#[test]
fn a_shard_without_a_value_reads_as_zero() {
    let values = drafts(&[("shard0", "20"), ("shard2", "x")]);
    assert_eq!(field_text(&values, "shard0"), "20");
    assert_eq!(field_text(&values, "shard1"), "0");
    assert_eq!(cpu_of(&values, "shard0"), 20);
    assert_eq!(cpu_of(&values, "shard1"), 0);
    assert_eq!(cpu_of(&values, "shard2"), 0);
}

#[test]
fn totals_read_blank_fields_as_zero_and_name_a_wrong_field() {
    assert_eq!(
        total(&fields(&[("shard0", "20"), ("shard1", " 5 ")])),
        Ok(25)
    );
    assert_eq!(total(&fields(&[("shard0", ""), ("shard1", "0")])), Ok(0));
    assert_eq!(
        total(&fields(&[
            ("shard0", "20"),
            ("shard1", "2.5"),
            ("shard2", "x")
        ])),
        Err("shard1".to_owned())
    );
    assert_eq!(
        total(&fields(&[("shard0", "-1")])),
        Err("shard0".to_owned())
    );
}

#[test]
fn the_sliders_end_at_the_account_cpu_or_at_the_total_without_it() {
    let values = drafts(&[("shard0", "20"), ("shard1", "x"), ("shard3", "35")]);
    assert_eq!(scale(Some(100.0), &values), 100);
    assert_eq!(scale(Some(99.6), &values), 100);
    assert_eq!(scale(Some(-5.0), &values), 0);
    assert_eq!(scale(None, &values), 55);
}

#[test]
fn a_shard_gets_at_most_its_cpu_plus_the_cpu_left() {
    let values = drafts(&[("shard0", "20"), ("shard1", "x"), ("shard3", "50")]);
    // 30 CPU are left: shard0 can get 20 + 30, shard3 50 + 30, shard1 (0) 30.
    assert_eq!(shard_max(&values, "shard0", 100), 50);
    assert_eq!(shard_max(&values, "shard3", 100), 80);
    assert_eq!(shard_max(&values, "shard1", 100), 30);
    assert_eq!(shard_max(&values, "shard2", 100), 30);
    // The others take more than all: nothing is left.
    assert_eq!(shard_max(&values, "shard0", 40), 0);
}

#[test]
fn a_typed_value_past_the_most_stops_there() {
    assert_eq!(typed("45", 50), "45");
    assert_eq!(typed("50", 50), "50");
    assert_eq!(typed("51", 50), "50");
    assert_eq!(typed(" 900 ", 50), "50");
    assert_eq!(typed("99999999999999", 50), "50");
    // Text that is not a whole number stays, so the field can tell what is wrong.
    assert_eq!(typed("", 50), "");
    assert_eq!(typed("x", 50), "x");
    assert_eq!(typed("2.5", 50), "2.5");
    assert_eq!(typed("-1", 50), "-1");
}

#[test]
fn a_step_moves_by_1_between_0_and_the_most() {
    assert_eq!(stepped(20, Step::More, 50), 21);
    assert_eq!(stepped(20, Step::Less, 50), 19);
    assert_eq!(stepped(50, Step::More, 50), 50);
    assert_eq!(stepped(0, Step::Less, 50), 0);
    // A shard past its most (the server's allocation was over the CPU) does not grow.
    assert_eq!(stepped(60, Step::More, 50), 60);
    assert_eq!(stepped(60, Step::Less, 50), 59);
}

#[test]
fn the_line_next_to_the_total_tells_the_cpu_left() {
    assert_eq!(left(90, 100), ("10 CPU left to give".to_owned(), LEFT));
    assert_eq!(left(100, 100), ("All CPU given".to_owned(), LEFT_DONE));
    assert_eq!(left(110, 100), ("10 CPU too many".to_owned(), LEFT_OVER));
}

#[test]
fn a_shard_warns_when_it_gets_more_than_its_cap() {
    assert_eq!(
        over_cap("shard3", 30, Some(20.0)),
        Some("Only 20 of 30 CPU usable on shard3.".to_owned())
    );
    assert_eq!(over_cap("shard3", 20, Some(20.0)), None);
    assert_eq!(over_cap("shard3", 5, Some(20.0)), None);
    assert_eq!(over_cap("shard0", 300, None), None);
}

#[test]
fn the_next_change_comes_12_hours_after_the_previous_one() {
    let now = 100.0 * HOUR;
    assert_eq!(next_change(None, now), None);
    assert_eq!(next_change(Some(now - HOUR), now), Some(now + 11.0 * HOUR));
    assert_eq!(next_change(Some(now - COOLDOWN_MS), now), None);
    assert_eq!(next_change(Some(now - 13.0 * HOUR), now), None);
}

#[test]
fn an_allocation_saves_only_when_it_totals_the_cpu_after_the_cooldown() {
    let now = 100.0 * HOUR;
    let even = fields(&[("shard0", "20"), ("shard1", ""), ("shard3", "80")]);
    let expected = [
        ("shard0".to_owned(), 20),
        ("shard1".to_owned(), 0),
        ("shard3".to_owned(), 80),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        allocation(&even, Some(100.0), Some(now - 13.0 * HOUR), now),
        Ok(expected)
    );
    assert!(allocation(&even, Some(100.0), None, now).is_ok());
    assert_eq!(
        allocation(&even, Some(120.0), None, now),
        Err(Blocker::Total {
            total: 100,
            limit: 120.0
        })
    );
    assert_eq!(
        allocation(&even, None, None, now),
        Err(Blocker::UnknownLimit)
    );
    assert_eq!(
        allocation(&even, Some(100.0), Some(now - HOUR), now),
        Err(Blocker::Cooldown(now + 11.0 * HOUR))
    );
    let wrong = fields(&[("shard0", "20"), ("shard3", "eighty")]);
    assert_eq!(
        allocation(&wrong, Some(100.0), None, now),
        Err(Blocker::NotANumber("shard3".to_owned()))
    );
}
