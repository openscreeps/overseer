use std::collections::BTreeMap;

use overseer::model::account::cpu::{AllocationError, CpuAllocation};

use super::{
    Blocker, LEFT, LEFT_DONE, LEFT_OVER, Step, allocation, blocker_text, cpu_of, field_text, left,
    over_cap, scale, shard_max, stepped, total, typed,
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

/// The rules of an account with `cpu` and no previous change.
const fn rules(cpu: Option<f64>) -> CpuAllocation {
    CpuAllocation { cpu, changed: None }
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
    assert_eq!(scale(rules(Some(100.0)), &values), 100);
    assert_eq!(scale(rules(Some(99.6)), &values), 100);
    assert_eq!(scale(rules(Some(-5.0)), &values), 0);
    assert_eq!(scale(rules(None), &values), 55);
}

#[test]
fn a_shard_gets_at_most_its_cpu_plus_the_cpu_left() {
    let values = drafts(&[("shard0", "20"), ("shard1", "x"), ("shard3", "50")]);
    // 30 CPU are left: shard0 can get 20 + 30, shard3 50 + 30, shard1 (0) 30.
    assert_eq!(shard_max(&values, "shard0", rules(Some(100.0))), 50);
    assert_eq!(shard_max(&values, "shard3", rules(Some(100.0))), 80);
    assert_eq!(shard_max(&values, "shard1", rules(Some(100.0))), 30);
    assert_eq!(shard_max(&values, "shard2", rules(Some(100.0))), 30);
    // The others take more than all: nothing is left.
    assert_eq!(shard_max(&values, "shard0", rules(Some(40.0))), 0);
    // Without the account's CPU a shard keeps what it has.
    assert_eq!(shard_max(&values, "shard3", rules(None)), 50);
}

#[test]
fn a_typed_value_past_the_most_stops_there() {
    assert_eq!(typed("45", 50), "45");
    assert_eq!(typed("50", 50), "50");
    assert_eq!(typed("51", 50), "50");
    assert_eq!(typed(" 900 ", 50), "50");
    assert_eq!(typed("99999999999999999999999", 50), "50");
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
fn the_form_saves_whole_numbers_that_keep_the_rules() {
    let now = 100.0 * HOUR;
    let even = fields(&[("shard0", "20"), ("shard1", ""), ("shard3", "80")]);
    let expected = [
        ("shard0".to_owned(), 20),
        ("shard1".to_owned(), 0),
        ("shard3".to_owned(), 80),
    ]
    .into_iter()
    .collect();
    assert_eq!(allocation(&even, rules(Some(100.0)), now), Ok(expected));
    let wrong = fields(&[("shard0", "20"), ("shard3", "eighty")]);
    assert_eq!(
        allocation(&wrong, rules(Some(100.0)), now),
        Err(Blocker::NotANumber("shard3".to_owned()))
    );
    let blocker = allocation(&even, rules(Some(120.0)), now).unwrap_err();
    assert_eq!(
        blocker_text(&blocker).as_deref(),
        Some("The shards get 100 CPU in total. Give them exactly 120 CPU.")
    );
    let cooldown = CpuAllocation {
        cpu: Some(100.0),
        changed: Some(now - HOUR),
    };
    let blocker = allocation(&even, cooldown, now).unwrap_err();
    assert_eq!(
        blocker,
        Blocker::Rule(AllocationError::Cooldown {
            next: now + 11.0 * HOUR
        })
    );
    // The line of the next allowed change tells the cooldown.
    assert_eq!(blocker_text(&blocker), None);
    assert_eq!(
        blocker_text(&Blocker::Rule(AllocationError::UnknownCpu)).as_deref(),
        Some("The server does not report your CPU limit.")
    );
}
