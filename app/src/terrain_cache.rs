//! What the world map learned, kept across restarts in `IndexedDB`: room terrain (it never
//! changes, so a room fetched once is never fetched again), rooms without terrain, room
//! overviews (owner, level, status, the players' badge parameters) with when they were
//! fetched, so a reopened world shows them at once and asks again only for the stale ones, the
//! rooms scanned for portals with their portals and scan time, and the players' rasterized
//! badges.
//!
//! # Storage
//!
//! Database `overseer-terrain` (schema version 3) with five object stores. The world stores
//! are keyed per room by `<FORMAT>|<server URL>|<shard>|<room>` ([`room_key`]; `<shard>` is
//! `-` on single-world servers):
//!
//! - `terrain`: the terrain packed 2 bits per tile ([`Terrain::packed`],
//!   [`Terrain::PACKED_LEN`] = 625 bytes), or no bytes when the server has no terrain for the
//!   room ([`Cached::NoTerrain`]); see [`terrain_record`].
//! - `stats`: the overview as a JSON string ([`stat_json`], read by [`parse_stat`]), with its
//!   own version (`"v"`, [`STAT_VERSION`]); overviews older than [`STAT_MAX_AGE_MS`] are not
//!   used.
//! - `portals`: a scanned room's portals and scan time as a JSON string
//!   ([`crate::portals::portals_json`], read by [`crate::portals::parse_portals`]); the
//!   `WorldMap` rescans stale ones.
//! - `worlds`: per `<FORMAT>|<server URL>|<shard>|` prefix ([`world_key`]) when it was last
//!   opened, in milliseconds since the epoch.
//!
//! The `badges` store is per server: `<FORMAT>|<server URL>|~badge|<user id>|<hash>`
//! ([`badge_key`]; the hash of the badge's parameters, so a changed badge is fetched again)
//! holds the badge rasterized to [`crate::badges::BADGE_SIZE`]² premultiplied RGBA pixels
//! (16 KiB).
//!
//! Schema version 1 had `terrain` and `worlds` only, version 2 added `stats`: upgrading adds
//! the missing stores and keeps the records ([`missing_stores`]). The record format is
//! versioned by [`FORMAT`] in the key: records of another format are deleted when the cache
//! opens.
//!
//! # Bound
//!
//! At most [`MAX_ROOMS`] terrain records (150 000: four full screeps.com shards of 182 × 182
//! rooms) are kept: about 670 bytes each with the key, so 100 MB of terrain, plus about
//! 150 bytes per overview (at most one per room, 22 MB) and a few hundred bytes per portal
//! room (a few hundred per shard). Past the bound, whole worlds (terrain, overviews and
//! portals) are evicted, least recently opened first and never the open one, down to
//! [`TARGET_ROOMS`] ([`plan_eviction`]); when the open world alone fills the bound, its
//! further terrain is not stored ([`writable`]). Badges are bounded apart: past
//! [`MAX_BADGES`] (64 MiB) the badge store is emptied when a cache opens (badges are fetched
//! again as they are needed).
//!
//! On a world's start the app loads all its cached rooms and hands them to the `WorldMap`
//! (`insert_terrain`, `insert_no_terrain`, `insert_stats`, `insert_portals`), which the
//! viewers get them from; what the library fetches later (`WorldEvent::Terrain`, `NoTerrain`,
//! `Stats`, `Portals`) is written in batches (`TerrainCache::store`, browsers only).

use overseer::map::{RoomCoord, RoomOwner, RoomReservation, RoomStat, RoomStatus};
use overseer::model::account::UserBadge;
use overseer::model::world::Terrain;
use serde_json::{Map, Value, json};

/// Record format, the first part of every key.
pub const FORMAT: u32 = 1;
/// Most terrain records kept.
pub const MAX_ROOMS: u32 = 150_000;
/// Terrain records kept after an eviction.
pub const TARGET_ROOMS: u32 = 135_000;
/// Most badge records kept (64 MiB of pixels).
pub const MAX_BADGES: u32 = 4096;

/// Rooms of a screeps.com shard (182 × 182).
const SHARD_ROOMS: u32 = 182 * 182;
// Four full shards fit, and an eviction frees room.
const _: () = assert!(MAX_ROOMS >= 4 * SHARD_ROOMS && TARGET_ROOMS < MAX_ROOMS);
/// Version of the overview records ([`stat_json`]).
pub const STAT_VERSION: u64 = 1;
/// Overviews older than this (a week, in milliseconds) are not used: owners change.
pub const STAT_MAX_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// Store of the terrain records.
pub const TERRAIN: &str = "terrain";
/// Store of the overview records.
pub const STATS: &str = "stats";
/// Store of the worlds' last-opened times.
pub const WORLDS: &str = "worlds";
/// Store of the scanned rooms' portals.
pub const PORTALS: &str = "portals";
/// Store of the rasterized badges.
pub const BADGES: &str = "badges";
/// Every store of the current schema.
pub const STORES: [&str; 5] = [TERRAIN, STATS, WORLDS, PORTALS, BADGES];

/// The key of the badge of user `user` with parameter hash `hash`
/// ([`crate::badges::badge_hash`]) on the server with base URL `server`.
pub fn badge_key(server: &str, user: &str, hash: u64) -> String {
    format!("{FORMAT}|{server}|~badge|{user}|{hash:016x}")
}

/// The key prefix of the world `shard` (`None` on single-world servers) of the server with
/// base URL `server`.
pub fn world_key(server: &str, shard: Option<&str>) -> String {
    format!("{FORMAT}|{server}|{}|", shard.unwrap_or("-"))
}

/// The key of `room` in the world [`world_key`] `world`.
pub fn room_key(world: &str, room: RoomCoord) -> String {
    format!("{world}{room}")
}

/// The room of a key in the world `world`, if it is one.
pub fn key_room(world: &str, key: &str) -> Option<RoomCoord> {
    key.strip_prefix(world)?.parse().ok()
}

/// The stores of [`STORES`] a database with the stores `existing` lacks: created on upgrade,
/// while the existing ones keep their records.
pub fn missing_stores(existing: &[String]) -> Vec<&'static str> {
    STORES
        .into_iter()
        .filter(|store| !existing.iter().any(|name| name == store))
        .collect()
}

/// A room's terrain record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cached {
    /// Its terrain.
    Terrain(Terrain),
    /// The server has no terrain for it.
    NoTerrain,
}

/// Reads a terrain record: [`Terrain::PACKED_LEN`] packed bytes, or none for a room without
/// terrain; `None` for anything else.
pub fn terrain_record(bytes: &[u8]) -> Option<Cached> {
    if bytes.is_empty() {
        return Some(Cached::NoTerrain);
    }
    let packed = <&[u8; Terrain::PACKED_LEN]>::try_from(bytes).ok()?;
    Terrain::from_packed(packed).ok().map(Cached::Terrain)
}

/// The overview record of `stat` (see [`parse_stat`]).
pub fn stat_json(stat: &RoomStat) -> String {
    let status = match stat.status {
        RoomStatus::Normal => "normal",
        RoomStatus::Novice => "novice",
        RoomStatus::Respawn => "respawn",
        RoomStatus::Closed => "closed",
        RoomStatus::OutOfBorders => "out of borders",
    };
    let mut record = Map::new();
    record.insert("v".to_owned(), json!(STAT_VERSION));
    record.insert("at".to_owned(), json!(stat.fetched_at));
    record.insert("status".to_owned(), json!(status));
    if let Some(until) = stat.status_until {
        record.insert("until".to_owned(), json!(until));
    }
    if let Some(owner) = &stat.owner {
        let mut entry = json!({"id": owner.id, "name": owner.username, "level": owner.level});
        insert_badge(&mut entry, owner.badge.as_ref());
        record.insert("owner".to_owned(), entry);
    }
    if let Some(reservation) = &stat.reservation {
        let mut entry = json!({"id": reservation.id, "name": reservation.username});
        insert_badge(&mut entry, reservation.badge.as_ref());
        record.insert("reservation".to_owned(), entry);
    }
    if stat.safe_mode {
        record.insert("safe".to_owned(), json!(true));
    }
    if let Some(mineral) = &stat.mineral {
        record.insert("mineral".to_owned(), json!(mineral));
    }
    if let Some(density) = stat.mineral_density {
        record.insert("density".to_owned(), json!(density));
    }
    Value::Object(record).to_string()
}

/// Adds `badge` (when known) to a user `entry` of an overview record.
fn insert_badge(entry: &mut Value, badge: Option<&UserBadge>) {
    if let (Some(entry), Some(badge)) = (entry.as_object_mut(), badge)
        && let Ok(badge) = serde_json::to_value(badge)
    {
        entry.insert("badge".to_owned(), badge);
    }
}

/// The badge of a user entry of an overview record; a malformed one is left out.
fn read_badge(entry: &Value) -> Option<UserBadge> {
    serde_json::from_value(entry.get("badge")?.clone()).ok()
}

/// Reads an overview record of [`stat_json`] at `now` (milliseconds since the epoch): `None`
/// when it is malformed, of another [`STAT_VERSION`], or older than [`STAT_MAX_AGE_MS`]. Its
/// `fetched_at` comes back as stored, so the `WorldMap` knows how stale it is.
pub fn parse_stat(record: &str, now: u64) -> Option<RoomStat> {
    let value: Value = serde_json::from_str(record).ok()?;
    if value.get("v")?.as_u64()? != STAT_VERSION {
        return None;
    }
    let fetched_at = value.get("at")?.as_u64()?;
    if now.saturating_sub(fetched_at) > STAT_MAX_AGE_MS {
        return None;
    }
    let status = match value.get("status")?.as_str()? {
        "normal" => RoomStatus::Normal,
        "novice" => RoomStatus::Novice,
        "respawn" => RoomStatus::Respawn,
        "closed" => RoomStatus::Closed,
        "out of borders" => RoomStatus::OutOfBorders,
        _ => return None,
    };
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    let owner = match value.get("owner") {
        None => None,
        Some(owner) => Some(RoomOwner {
            id: text(owner, "id")?,
            username: text(owner, "name"),
            level: u8::try_from(owner.get("level")?.as_u64()?).ok()?,
            badge: read_badge(owner),
        }),
    };
    let reservation = match value.get("reservation") {
        None => None,
        Some(reservation) => Some(RoomReservation {
            id: text(reservation, "id")?,
            username: text(reservation, "name"),
            badge: read_badge(reservation),
        }),
    };
    Some(RoomStat {
        status,
        status_until: value.get("until").and_then(Value::as_u64),
        owner,
        reservation,
        safe_mode: value.get("safe").and_then(Value::as_bool) == Some(true),
        mineral: text(&value, "mineral"),
        mineral_density: value
            .get("density")
            .and_then(Value::as_u64)
            .and_then(|density| u8::try_from(density).ok()),
        fetched_at,
    })
}

/// A cached world: its key, when it was last opened and how many rooms it has.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldEntry {
    /// [`world_key`].
    pub key: String,
    /// Last opened, milliseconds since the epoch.
    pub opened: f64,
    /// Rooms stored.
    pub rooms: u32,
}

/// The worlds to delete so that at most [`MAX_ROOMS`] stay (see the module docs): none while
/// the total is within the bound, else the least recently opened worlds other than `open`
/// until [`TARGET_ROOMS`] or fewer remain (or only `open` is left).
pub fn plan_eviction(worlds: &[WorldEntry], open: &str) -> Vec<String> {
    let mut total: u32 = worlds.iter().map(|world| world.rooms).sum();
    if total <= MAX_ROOMS {
        return Vec::new();
    }
    let mut others: Vec<&WorldEntry> = worlds.iter().filter(|world| world.key != open).collect();
    others.sort_by(|a, b| a.opened.total_cmp(&b.opened));
    let mut evicted = Vec::new();
    for world in others {
        if total <= TARGET_ROOMS {
            break;
        }
        total -= world.rooms;
        evicted.push(world.key.clone());
    }
    evicted
}

/// How many of `wanted` new rooms may be stored when `stored` rooms are cached.
pub const fn writable(stored: u32, wanted: u32) -> u32 {
    let free = MAX_ROOMS.saturating_sub(stored);
    if wanted < free { wanted } else { free }
}

#[cfg(target_arch = "wasm32")]
pub use idb::{BadgeStore, TerrainCache};

#[cfg(target_arch = "wasm32")]
mod idb;

#[cfg(target_arch = "wasm32")]
pub use idb::forget_server;

#[cfg(test)]
mod tests {
    use overseer::map::{RoomOwner, RoomReservation, RoomStat, RoomStatus};
    use overseer::model::world::{Terrain, Tile};

    use super::{
        Cached, MAX_BADGES, MAX_ROOMS, STAT_MAX_AGE_MS, STORES, WorldEntry, badge_key, key_room,
        missing_stores, parse_stat, plan_eviction, room_key, stat_json, terrain_record, world_key,
        writable,
    };
    use crate::badges::test_badge;

    fn terrain() -> Terrain {
        let digits: String = (0..2500)
            .map(|index| match index % 7 {
                0 => '1',
                3 => '2',
                5 => '3',
                _ => '0',
            })
            .collect();
        Terrain::decode(&digits).unwrap()
    }

    #[test]
    fn keys_are_per_server_shard_and_room() {
        let world = world_key("https://screeps.com/", Some("shard3"));
        assert_eq!(world, "1|https://screeps.com/|shard3|");
        let single = world_key("http://localhost:21025/", None);
        assert_eq!(single, "1|http://localhost:21025/|-|");
        let room = "W8N3".parse().unwrap();
        let key = room_key(&world, room);
        assert_eq!(key, "1|https://screeps.com/|shard3|W8N3");
        assert_eq!(key_room(&world, &key), Some(room));
        assert_eq!(key_room(&single, &key), None);
        assert_eq!(key_room(&world, "1|https://screeps.com/|shard3|sim"), None);
        // A shard prefix never matches a longer shard name's records.
        let shard1 = world_key("https://screeps.com/", Some("shard1"));
        let shard10 = room_key(&world_key("https://screeps.com/", Some("shard10")), room);
        assert!(!shard10.starts_with(&shard1));
    }

    #[test]
    fn badge_keys_are_per_server_user_and_parameters() {
        // The documented bound: 16 KiB per badge, 64 MiB in all.
        assert_eq!(crate::badges::BADGE_BYTES, 16 * 1024);
        assert_eq!(MAX_BADGES as usize * crate::badges::BADGE_BYTES, 64 << 20);
        assert_eq!(crate::badges::BADGE_SIZE, 64);
        let key = badge_key("https://screeps.com/", "u1", 0xabc);
        assert_eq!(key, "1|https://screeps.com/|~badge|u1|0000000000000abc");
        // Forgetting the server (its `<FORMAT>|<url>|` prefix) forgets its badges; no world
        // of it holds them.
        assert!(key.starts_with("1|https://screeps.com/|"));
        let world = world_key("https://screeps.com/", Some("shard3"));
        assert_eq!(key_room(&world, &key), None);
        assert!(!key.starts_with(&world));
    }

    #[test]
    fn terrain_records_are_packed_or_empty() {
        let terrain = terrain();
        let packed = terrain.packed();
        let Some(Cached::Terrain(read)) = terrain_record(packed) else {
            panic!("a terrain expected");
        };
        assert_eq!(read, terrain);
        assert_eq!(read.get(5, 0), Some(Tile::Wall), "wall bit wins");
        assert_eq!(terrain_record(&[]), Some(Cached::NoTerrain));
        assert_eq!(terrain_record(&packed[1..]), None);
        let mut invalid = *packed;
        invalid[0] = 0b11;
        assert_eq!(terrain_record(&invalid), None);
    }

    #[test]
    fn upgrades_add_the_missing_stores_only() {
        assert_eq!(missing_stores(&[]), STORES);
        let schema_1 = ["terrain".to_owned(), "worlds".to_owned()];
        assert_eq!(missing_stores(&schema_1), ["stats", "portals", "badges"]);
        let schema_2 = [
            "terrain".to_owned(),
            "worlds".to_owned(),
            "stats".to_owned(),
        ];
        assert_eq!(missing_stores(&schema_2), ["portals", "badges"]);
        let current = STORES.map(str::to_owned);
        assert_eq!(missing_stores(&current), Vec::<&str>::new());
    }

    const NOW: u64 = 1_800_000_000_000;

    fn stat(fetched_at: u64) -> RoomStat {
        RoomStat {
            status: RoomStatus::Novice,
            status_until: Some(NOW + 5_000),
            owner: Some(RoomOwner {
                id: "u1".to_owned(),
                username: Some("Alice".to_owned()),
                level: 8,
                badge: Some(test_badge(3)),
            }),
            reservation: Some(RoomReservation {
                id: "u2".to_owned(),
                username: None,
                badge: None,
            }),
            safe_mode: true,
            mineral: Some("H".to_owned()),
            mineral_density: Some(3),
            fetched_at,
        }
    }

    #[test]
    fn overviews_round_trip_with_their_fetch_time() {
        let fetched = stat(NOW - 60_000);
        let record = stat_json(&fetched);
        assert_eq!(parse_stat(&record, NOW), Some(fetched));
        let plain = RoomStat {
            status: RoomStatus::OutOfBorders,
            status_until: None,
            owner: None,
            reservation: None,
            safe_mode: false,
            mineral: None,
            mineral_density: None,
            fetched_at: NOW,
        };
        let record = stat_json(&plain);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&record).unwrap(),
            serde_json::json!({"v": 1, "at": 1_800_000_000_000_u64, "status": "out of borders"})
        );
        assert_eq!(parse_stat(&record, NOW), Some(plain));
        // Records written before badges were kept read without them; a malformed badge is
        // left out, the overview kept.
        let old = r#"{"v":1,"at":1800000000000,"status":"normal","owner":{"id":"u1","name":"Alice","level":3}}"#;
        let read = parse_stat(old, NOW).unwrap();
        assert_eq!(read.owner.unwrap().badge, None);
        let malformed = r#"{"v":1,"at":1800000000000,"status":"normal","owner":{"id":"u1","level":3,"badge":{"type":1}}}"#;
        let read = parse_stat(malformed, NOW).unwrap();
        assert_eq!(read.owner.unwrap().badge, None);
    }

    #[test]
    fn stale_malformed_and_other_versions_are_not_used() {
        // A week old is still used (the world map refreshes it), older is not.
        let week = stat(NOW - STAT_MAX_AGE_MS);
        assert_eq!(parse_stat(&stat_json(&week), NOW), Some(week));
        assert_eq!(
            parse_stat(&stat_json(&stat(NOW - STAT_MAX_AGE_MS - 1)), NOW),
            None
        );
        // A clock running behind keeps the record (the world map treats it as fresh).
        assert!(parse_stat(&stat_json(&stat(NOW + 1_000)), NOW).is_some());
        for record in [
            r#"{"v":2,"at":1800000000000,"status":"normal"}"#,
            r#"{"at":1800000000000,"status":"normal"}"#,
            r#"{"v":1,"status":"normal"}"#,
            r#"{"v":1,"at":1800000000000,"status":"lost"}"#,
            r#"{"v":1,"at":1800000000000,"status":"normal","owner":{"name":"Alice","level":3}}"#,
            r#"{"v":1,"at":1800000000000,"status":"normal","owner":{"id":"u1","level":300}}"#,
            "not json",
        ] {
            assert_eq!(parse_stat(record, NOW), None, "{record}");
        }
    }

    fn world(key: &str, opened: f64, rooms: u32) -> WorldEntry {
        WorldEntry {
            key: key.to_owned(),
            opened,
            rooms,
        }
    }

    #[test]
    fn evicts_least_recently_opened_worlds_but_never_the_open_one() {
        let within = [world("a", 1.0, MAX_ROOMS - 10), world("b", 2.0, 10)];
        assert_eq!(plan_eviction(&within, "b"), Vec::<String>::new());

        let over = [
            world("old", 1.0, 60_000),
            world("open", 0.5, 60_000),
            world("older", 0.0, 5_000),
            world("recent", 3.0, 33_124),
        ];
        // 158 124 rooms: drop "older" (153 124 left, above the target), then "old".
        assert_eq!(plan_eviction(&over, "open"), ["older", "old"]);

        let alone = [world("open", 1.0, MAX_ROOMS + 1)];
        assert_eq!(plan_eviction(&alone, "open"), Vec::<String>::new());
    }

    #[test]
    fn writes_stop_at_the_bound() {
        assert_eq!(writable(0, 100), 100);
        assert_eq!(writable(MAX_ROOMS - 10, 100), 10);
        assert_eq!(writable(MAX_ROOMS, 100), 0);
        assert_eq!(writable(MAX_ROOMS + 5, 1), 0);
    }
}
