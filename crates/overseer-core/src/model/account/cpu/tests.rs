use std::collections::BTreeMap;

use super::{
    AllocationError, CHANGE_COOLDOWN_MS, CpuAllocation, ShardCpu, cpu_limit, shard_cpu,
    shard_order, usable,
};
use crate::model::world::ShardInfo;

/// One hour, in milliseconds.
const HOUR: f64 = 60.0 * 60.0 * 1000.0;

/// The allocation of `pairs` (shard, CPU).
fn allocation(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs
        .iter()
        .map(|(shard, cpu)| ((*shard).to_owned(), *cpu))
        .collect()
}

#[test]
fn a_shard_lets_the_code_use_at_most_its_cap() {
    assert!((usable(30.0, Some(20.0)) - 20.0).abs() < 1e-9);
    assert!((usable(10.0, Some(20.0)) - 10.0).abs() < 1e-9);
    assert!((usable(30.0, None) - 30.0).abs() < 1e-9);
    let assigned = BTreeMap::from([("a".to_owned(), 1.0), ("shard1".to_owned(), 2.0)]);
    assert_eq!(
        shard_order(&["shard1".to_owned(), "shard0".to_owned()], &assigned),
        ["shard1", "shard0", "a"]
    );
}

#[test]
fn shards_list_the_servers_shards_then_the_allocations_with_their_caps() {
    let shards = ["shard0".to_owned(), "shard3".to_owned()];
    let assigned = BTreeMap::from([("shard3".to_owned(), 30.0), ("shardSeason".to_owned(), 5.0)]);
    let info = [
        ShardInfo {
            name: "shard0".to_owned(),
            cpu_limit: None,
            tick: Some(4100.0),
        },
        ShardInfo {
            name: "shard3".to_owned(),
            cpu_limit: Some(20.0),
            tick: Some(3150.5),
        },
    ];
    let rows = shard_cpu(&shards, &assigned, &info);
    assert_eq!(
        rows,
        [
            ShardCpu {
                name: "shard0".to_owned(),
                assigned: 0.0,
                cap: None,
                average_tick: Some(4100.0),
            },
            ShardCpu {
                name: "shard3".to_owned(),
                assigned: 30.0,
                cap: Some(20.0),
                average_tick: Some(3150.5),
            },
            ShardCpu {
                name: "shardSeason".to_owned(),
                assigned: 5.0,
                cap: None,
                average_tick: None,
            },
        ]
    );
    assert!((rows[1].usable() - 20.0).abs() < 1e-9);
    assert!(rows[1].over_cap());
    assert!(!rows[2].over_cap());
    assert!((rows[2].usable() - 5.0).abs() < 1e-9);
    // The limit of a run: the usable CPU of the shard, else the account's CPU.
    assert_eq!(cpu_limit(Some(35.0), &rows, Some("shard3")), Some(20.0));
    assert_eq!(cpu_limit(Some(35.0), &rows, Some("shard9")), Some(35.0));
    assert_eq!(cpu_limit(Some(35.0), &rows, None), Some(35.0));
    assert_eq!(cpu_limit(None, &[], Some("shard3")), None);
}

#[test]
fn the_account_cpu_rounds_to_a_whole_number() {
    let total = |cpu| CpuAllocation { cpu, changed: None }.total();
    assert_eq!(total(Some(100.0)), Some(100));
    assert_eq!(total(Some(99.6)), Some(100));
    assert_eq!(total(Some(-5.0)), Some(0));
    assert_eq!(total(None), None);
}

#[test]
fn a_shard_gets_at_most_what_the_others_leave() {
    let rules = CpuAllocation {
        cpu: Some(100.0),
        changed: None,
    };
    let given = allocation(&[("shard0", 20), ("shard3", 50)]);
    // 30 CPU are left: shard0 can get 20 + 30, shard3 50 + 30, an absent shard 30.
    assert_eq!(rules.remaining_for(&given, "shard0"), Some(50));
    assert_eq!(rules.remaining_for(&given, "shard3"), Some(80));
    assert_eq!(rules.remaining_for(&given, "shard1"), Some(30));
    // The others take more than all: nothing is left.
    let over = allocation(&[("shard0", 20), ("shard3", 110)]);
    assert_eq!(rules.remaining_for(&over, "shard0"), Some(0));
    assert_eq!(
        CpuAllocation::default().remaining_for(&given, "shard0"),
        None
    );
}

#[test]
fn the_next_change_comes_12_hours_after_the_previous_one() {
    let now = 100.0 * HOUR;
    let at = |changed| CpuAllocation { cpu: None, changed }.next_change_at(now);
    assert_eq!(at(None), None);
    assert_eq!(at(Some(now - HOUR)), Some(now + 11.0 * HOUR));
    assert_eq!(at(Some(now - CHANGE_COOLDOWN_MS)), None);
    assert_eq!(at(Some(now - 13.0 * HOUR)), None);
}

#[test]
fn an_allocation_is_valid_when_it_totals_the_cpu_after_the_cooldown() {
    let now = 100.0 * HOUR;
    let rules = |cpu, changed| CpuAllocation { cpu, changed };
    let even = allocation(&[("shard0", 20), ("shard1", 0), ("shard3", 80)]);
    assert_eq!(
        rules(Some(100.0), Some(now - 13.0 * HOUR)).validate(&even, now),
        Ok(())
    );
    assert_eq!(rules(Some(100.0), None).validate(&even, now), Ok(()));
    assert_eq!(
        rules(Some(120.0), None).validate(&even, now),
        Err(AllocationError::Total {
            total: 100,
            cpu: 120.0
        })
    );
    assert_eq!(
        rules(None, None).validate(&even, now),
        Err(AllocationError::UnknownCpu)
    );
    assert_eq!(
        rules(Some(100.0), Some(now - HOUR)).validate(&even, now),
        Err(AllocationError::Cooldown {
            next: now + 11.0 * HOUR
        })
    );
    // A limit above a shard's cap is valid: only the total counts.
    let capped = allocation(&[("shard3", 100)]);
    assert_eq!(rules(Some(100.0), None).validate(&capped, now), Ok(()));
}
