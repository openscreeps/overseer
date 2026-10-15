//! What the world map learned, kept across restarts in `IndexedDB`: room terrain (it never
//! changes, so a room fetched once is never fetched again), rooms without terrain, and room
//! overviews (owner, level, status) with when they were fetched, so a reopened world shows
//! them at once and asks again only for the stale ones.
//!
//! # Storage
//!
//! Database `overseer-terrain` (schema version 2) with three object stores, all
//! keyed per room by `<FORMAT>|<server URL>|<shard>|<room>` ([`room_key`]; `<shard>` is `-` on
//! single-world servers):
//!
//! - `terrain`: the terrain packed 2 bits per tile ([`Terrain::packed`],
//!   [`Terrain::PACKED_LEN`] = 625 bytes), or no bytes when the server has no terrain for the
//!   room ([`Cached::NoTerrain`]); see [`terrain_record`].
//! - `stats`: the overview as a JSON string ([`stat_json`], read by [`parse_stat`]), with its
//!   own version (`"v"`, [`STAT_VERSION`]); overviews older than [`STAT_MAX_AGE_MS`] are not
//!   used.
//! - `worlds`: per `<FORMAT>|<server URL>|<shard>|` prefix ([`world_key`]) when it was last
//!   opened, in milliseconds since the epoch.
//!
//! Schema version 1 had `terrain` and `worlds` only: upgrading adds the missing stores and
//! keeps the records ([`missing_stores`]). The record format is versioned by [`FORMAT`] in the
//! key: records of another format are deleted when the cache opens.
//!
//! # Bound
//!
//! At most [`MAX_ROOMS`] terrain records (150 000: four full screeps.com shards of 182 × 182
//! rooms) are kept: about 670 bytes each with the key, so 100 MB of terrain, plus about
//! 150 bytes per overview (at most one per room, 22 MB). Past the bound, whole worlds
//! (terrain and overviews) are evicted, least recently opened first and never the open one,
//! down to [`TARGET_ROOMS`] ([`plan_eviction`]); when the open world alone fills the bound,
//! its further terrain is not stored ([`writable`]).
//!
//! On a world's start the app loads all its cached rooms and hands them to the `WorldMap`
//! (`insert_terrain`, `insert_no_terrain`, `insert_stats`), which the viewers get them from;
//! what the library fetches later (`WorldEvent::Terrain`, `NoTerrain`, `Stats`) is written in
//! batches (`TerrainCache::store`, browsers only).

use overseer::core::api::Terrain;
use overseer::map::{RoomCoord, RoomOwner, RoomReservation, RoomStat, RoomStatus};
use serde_json::{Map, Value, json};

/// Record format, the first part of every key.
pub const FORMAT: u32 = 1;
/// Most terrain records kept.
pub const MAX_ROOMS: u32 = 150_000;
/// Terrain records kept after an eviction.
pub const TARGET_ROOMS: u32 = 135_000;

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
/// Every store of the current schema.
pub const STORES: [&str; 3] = [TERRAIN, STATS, WORLDS];

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
        record.insert(
            "owner".to_owned(),
            json!({"id": owner.id, "name": owner.username, "level": owner.level}),
        );
    }
    if let Some(reservation) = &stat.reservation {
        record.insert(
            "reservation".to_owned(),
            json!({"id": reservation.id, "name": reservation.username}),
        );
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
        }),
    };
    let reservation = match value.get("reservation") {
        None => None,
        Some(reservation) => Some(RoomReservation {
            id: text(reservation, "id")?,
            username: text(reservation, "name"),
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
pub use idb::TerrainCache;

/// The `IndexedDB` side (browsers only).
#[cfg(target_arch = "wasm32")]
mod idb {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Duration;

    use js_sys::{Array, Function, Promise, Uint8Array};
    use overseer::core::api::Terrain;
    use overseer::map::{RoomCoord, RoomStat, WorldEvent};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        IdbDatabase, IdbKeyRange, IdbObjectStore, IdbOpenDbRequest, IdbRequest, IdbTransaction,
        IdbTransactionMode,
    };

    use super::{
        Cached, FORMAT, STATS, STORES, TERRAIN, WORLDS, WorldEntry, key_room, missing_stores,
        parse_stat, plan_eviction, room_key, stat_json, terrain_record, world_key, writable,
    };

    /// `IndexedDB` database name.
    const DB_NAME: &str = "overseer-terrain";
    /// `IndexedDB` schema version (object stores): 1 had `terrain` and `worlds`, 2 adds
    /// `stats`.
    const DB_VERSION: u32 = 2;
    /// How long records wait to be written with those arriving after them.
    const FLUSH_DELAY: Duration = Duration::from_secs(1);

    /// What a world's cache holds.
    #[derive(Default)]
    pub struct Loaded {
        /// Rooms and their terrain.
        pub terrain: Vec<(RoomCoord, Arc<Terrain>)>,
        /// Rooms the server has no terrain for.
        pub no_terrain: Vec<RoomCoord>,
        /// Rooms and their overview, with when it was fetched.
        pub stats: Vec<(RoomCoord, Arc<RoomStat>)>,
    }

    /// A record waiting to be written.
    enum Write {
        /// A terrain record: the terrain, or `None` for a room without one.
        Terrain(RoomCoord, Option<Arc<Terrain>>),
        /// An overview record.
        Stat(RoomCoord, Arc<RoomStat>),
    }

    /// The cache of one world; cheap to clone.
    #[derive(Clone)]
    pub struct TerrainCache {
        db: IdbDatabase,
        world: Rc<str>,
        /// Terrain records stored across all worlds, as last counted plus writes since.
        stored: Rc<Cell<u32>>,
        /// Records waiting to be written.
        pending: Rc<RefCell<Vec<Write>>>,
    }

    impl TerrainCache {
        /// Opens the cache of the world `shard` of `server`: deletes records of other formats,
        /// marks the world opened now, and evicts other worlds past the bound.
        ///
        /// # Errors
        ///
        /// Returns a message when `IndexedDB` is unavailable or fails.
        pub async fn open(server: &str, shard: Option<&str>) -> Result<Self, String> {
            let db = open_db().await?;
            let world: Rc<str> = world_key(server, shard).into();
            let cache = Self {
                db,
                world,
                stored: Rc::new(Cell::new(0)),
                pending: Rc::new(RefCell::new(Vec::new())),
            };
            cache.drop_other_formats().await?;
            cache.touch().await?;
            cache.evict().await?;
            Ok(cache)
        }

        /// Everything cached of the world; overviews older than
        /// [`STAT_MAX_AGE_MS`](super::STAT_MAX_AGE_MS) are left out.
        ///
        /// # Errors
        ///
        /// Returns a message when `IndexedDB` fails.
        pub async fn load(&self) -> Result<Loaded, String> {
            let transaction = self.transaction(&[TERRAIN, STATS], IdbTransactionMode::Readonly)?;
            let mut loaded = Loaded::default();
            let mut buffer = [0; Terrain::PACKED_LEN];
            for (room, value) in self.records(&transaction, TERRAIN).await? {
                let Some(bytes) = value.dyn_ref::<Uint8Array>() else {
                    continue;
                };
                let record = match bytes.length() as usize {
                    0 => terrain_record(&[]),
                    Terrain::PACKED_LEN => {
                        bytes.copy_to(&mut buffer);
                        terrain_record(&buffer)
                    }
                    _ => None,
                };
                match record {
                    Some(Cached::Terrain(terrain)) => {
                        loaded.terrain.push((room, Arc::new(terrain)));
                    }
                    Some(Cached::NoTerrain) => loaded.no_terrain.push(room),
                    None => {}
                }
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the epoch time in milliseconds is a non-negative integer far below u64::MAX"
            )]
            let now = js_sys::Date::now() as u64;
            for (room, value) in self.records(&transaction, STATS).await? {
                if let Some(stat) = value.as_string().and_then(|json| parse_stat(&json, now)) {
                    loaded.stats.push((room, Arc::new(stat)));
                }
            }
            Ok(loaded)
        }

        /// The world's records of `store` in `transaction`, by room.
        async fn records(
            &self,
            transaction: &IdbTransaction,
            store: &str,
        ) -> Result<Vec<(RoomCoord, JsValue)>, String> {
            let store = transaction.object_store(store).map_err(js_message)?;
            let range = prefix_range(&self.world)?;
            let keys = request(&store.get_all_keys_with_key(&range).map_err(js_message)?).await?;
            let values = request(&store.get_all_with_key(&range).map_err(js_message)?).await?;
            let (keys, values) = (Array::from(&keys), Array::from(&values));
            Ok(keys
                .iter()
                .zip(values.iter())
                .filter_map(|(key, value)| {
                    let room = key_room(&self.world, &key.as_string()?)?;
                    Some((room, value))
                })
                .collect())
        }

        /// Stores what `event` brought: a terrain, a room without terrain, or overviews.
        /// Records arriving within [`FLUSH_DELAY`] are written in one transaction. Best
        /// effort: a record that fails to be written is fetched again next time.
        pub fn store(&self, event: &WorldEvent) {
            let first = {
                let mut pending = self.pending.borrow_mut();
                let before = pending.len();
                match event {
                    WorldEvent::Terrain(room, terrain) => {
                        pending.push(Write::Terrain(*room, Some(Arc::clone(terrain))));
                    }
                    WorldEvent::NoTerrain(room) => pending.push(Write::Terrain(*room, None)),
                    WorldEvent::Stats(stats) => pending.extend(
                        stats
                            .iter()
                            .map(|(room, stat)| Write::Stat(*room, Arc::clone(stat))),
                    ),
                    _ => {}
                }
                before == 0 && !pending.is_empty()
            };
            if first {
                let cache = self.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    overseer::net::rt::sleep(FLUSH_DELAY).await;
                    // Best effort (see above).
                    let _ = cache.flush().await;
                });
            }
        }

        /// Writes the queued records in one transaction, terrain as far as the bound allows.
        async fn flush(&self) -> Result<(), String> {
            let writes = std::mem::take(&mut *self.pending.borrow_mut());
            let terrain_count = writes
                .iter()
                .filter(|write| matches!(write, Write::Terrain(..)))
                .count();
            let mut allowed = writable(
                self.stored.get(),
                u32::try_from(terrain_count).unwrap_or(u32::MAX),
            );
            let written = allowed;
            let transaction = self.transaction(&[TERRAIN, STATS], IdbTransactionMode::Readwrite)?;
            let terrain = transaction.object_store(TERRAIN).map_err(js_message)?;
            let stats = transaction.object_store(STATS).map_err(js_message)?;
            for write in &writes {
                match write {
                    Write::Terrain(room, record) => {
                        if allowed == 0 {
                            continue;
                        }
                        allowed -= 1;
                        let bytes = record.as_ref().map_or(&[][..], |terrain| terrain.packed());
                        self.put(&terrain, *room, &Uint8Array::from(bytes))?;
                    }
                    Write::Stat(room, stat) => {
                        self.put(&stats, *room, &JsValue::from_str(&stat_json(stat)))?;
                    }
                }
            }
            done(&transaction).await?;
            self.stored.set(self.stored.get().saturating_add(written));
            Ok(())
        }

        /// Puts `value` under the key of `room` into `store`.
        fn put(
            &self,
            store: &IdbObjectStore,
            room: RoomCoord,
            value: &JsValue,
        ) -> Result<(), String> {
            store
                .put_with_key(value, &JsValue::from_str(&room_key(&self.world, room)))
                .map(drop)
                .map_err(js_message)
        }

        /// Records that the world was opened now.
        async fn touch(&self) -> Result<(), String> {
            let transaction = self.transaction(&[WORLDS], IdbTransactionMode::Readwrite)?;
            let store = transaction.object_store(WORLDS).map_err(js_message)?;
            store
                .put_with_key(
                    &JsValue::from_f64(js_sys::Date::now()),
                    &JsValue::from_str(&self.world),
                )
                .map_err(js_message)?;
            done(&transaction).await
        }

        /// Deletes the records of other formats (keys outside `<FORMAT>|…`).
        async fn drop_other_formats(&self) -> Result<(), String> {
            let prefix = format!("{FORMAT}|");
            let transaction = self.transaction(&STORES, IdbTransactionMode::Readwrite)?;
            for name in STORES {
                let store = transaction.object_store(name).map_err(js_message)?;
                let below = IdbKeyRange::upper_bound_with_open(&JsValue::from_str(&prefix), true)
                    .map_err(js_message)?;
                let above = IdbKeyRange::lower_bound_with_open(
                    &JsValue::from_str(&format!("{prefix}\u{ffff}")),
                    true,
                )
                .map_err(js_message)?;
                store.delete(&below.into()).map_err(js_message)?;
                store.delete(&above.into()).map_err(js_message)?;
            }
            done(&transaction).await
        }

        /// Counts the terrain records of every world and deletes the worlds [`plan_eviction`]
        /// picks (their terrain and overviews).
        async fn evict(&self) -> Result<(), String> {
            let transaction = self.transaction(&STORES, IdbTransactionMode::Readwrite)?;
            let terrain = transaction.object_store(TERRAIN).map_err(js_message)?;
            let stats = transaction.object_store(STATS).map_err(js_message)?;
            let worlds = transaction.object_store(WORLDS).map_err(js_message)?;
            let keys = Array::from(&request(&worlds.get_all_keys().map_err(js_message)?).await?);
            let opened = Array::from(&request(&worlds.get_all().map_err(js_message)?).await?);
            let mut entries = Vec::with_capacity(keys.length() as usize);
            for (key, opened) in keys.iter().zip(opened.iter()) {
                let Some(key) = key.as_string() else {
                    continue;
                };
                let count = request(
                    &terrain
                        .count_with_key(&prefix_range(&key)?)
                        .map_err(js_message)?,
                )
                .await?;
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "IndexedDB counts are non-negative integers far below u32::MAX"
                )]
                let rooms = count.as_f64().unwrap_or(0.0) as u32;
                entries.push(WorldEntry {
                    key,
                    opened: opened.as_f64().unwrap_or(0.0),
                    rooms,
                });
            }
            let evicted = plan_eviction(&entries, &self.world);
            let mut stored: u32 = entries.iter().map(|entry| entry.rooms).sum();
            for key in &evicted {
                terrain.delete(&prefix_range(key)?).map_err(js_message)?;
                stats.delete(&prefix_range(key)?).map_err(js_message)?;
                worlds.delete(&JsValue::from_str(key)).map_err(js_message)?;
                if let Some(entry) = entries.iter().find(|entry| &entry.key == key) {
                    stored -= entry.rooms;
                }
            }
            done(&transaction).await?;
            self.stored.set(stored);
            Ok(())
        }

        /// A transaction on `stores`.
        fn transaction(
            &self,
            stores: &[&str],
            mode: IdbTransactionMode,
        ) -> Result<IdbTransaction, String> {
            let names: Array = stores.iter().map(|name| JsValue::from_str(name)).collect();
            self.db
                .transaction_with_str_sequence_and_mode(&names, mode)
                .map_err(js_message)
        }
    }

    /// Deletes everything cached of the server with base URL `server` (all its worlds).
    ///
    /// # Errors
    ///
    /// Returns a message when `IndexedDB` fails.
    pub async fn forget_server(server: &str) -> Result<(), String> {
        let db = open_db().await?;
        let prefix = format!("{FORMAT}|{server}|");
        let names: Array = STORES.iter().map(|name| JsValue::from_str(name)).collect();
        let transaction = db
            .transaction_with_str_sequence_and_mode(&names, IdbTransactionMode::Readwrite)
            .map_err(js_message)?;
        for name in STORES {
            transaction
                .object_store(name)
                .map_err(js_message)?
                .delete(&prefix_range(&prefix)?)
                .map_err(js_message)?;
        }
        let result = done(&transaction).await;
        db.close();
        result
    }

    /// Opens (creating or upgrading) the database.
    async fn open_db() -> Result<IdbDatabase, String> {
        let factory = web_sys::window()
            .ok_or_else(|| "no window".to_owned())?
            .indexed_db()
            .map_err(js_message)?
            .ok_or_else(|| "IndexedDB is unavailable".to_owned())?;
        let open: IdbOpenDbRequest = factory
            .open_with_u32(DB_NAME, DB_VERSION)
            .map_err(js_message)?;
        let upgrade_request = open.clone();
        let upgrade = Closure::<dyn FnMut()>::new(move || {
            if let Ok(result) = upgrade_request.result()
                && let Ok(db) = result.dyn_into::<IdbDatabase>()
            {
                let names = db.object_store_names();
                let existing = (0..names.length())
                    .filter_map(|index| names.item(index))
                    .collect::<Vec<_>>();
                for name in missing_stores(&existing) {
                    let _ = db.create_object_store(name);
                }
            }
        });
        open.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        let result = request(&open).await;
        open.set_onupgradeneeded(None);
        drop(upgrade);
        result?
            .dyn_into()
            .map_err(|_| "IndexedDB returned no database".to_owned())
    }

    /// The keys starting with `prefix` (an `IDBKeyRange`).
    fn prefix_range(prefix: &str) -> Result<JsValue, String> {
        IdbKeyRange::bound(
            &JsValue::from_str(prefix),
            &JsValue::from_str(&format!("{prefix}\u{ffff}")),
        )
        .map(JsValue::from)
        .map_err(js_message)
    }

    /// The result of `request` once it succeeds.
    async fn request(request: &IdbRequest) -> Result<JsValue, String> {
        let promise = Promise::new(&mut |resolve: Function, reject: Function| {
            request.set_onsuccess(Some(&resolve));
            request.set_onerror(Some(&reject));
        });
        let outcome = JsFuture::from(promise).await;
        request.set_onsuccess(None);
        request.set_onerror(None);
        match outcome {
            Ok(_) => request.result().map_err(js_message),
            Err(_) => Err(request.error().ok().flatten().map_or_else(
                || "IndexedDB request failed".to_owned(),
                |error| error.message(),
            )),
        }
    }

    /// Waits until `transaction` commits.
    async fn done(transaction: &IdbTransaction) -> Result<(), String> {
        let promise = Promise::new(&mut |resolve: Function, reject: Function| {
            transaction.set_oncomplete(Some(&resolve));
            transaction.set_onerror(Some(&reject));
            transaction.set_onabort(Some(&reject));
        });
        let outcome = JsFuture::from(promise).await;
        transaction.set_oncomplete(None);
        transaction.set_onerror(None);
        transaction.set_onabort(None);
        outcome.map(drop).map_err(|_| {
            transaction.error().map_or_else(
                || "IndexedDB transaction failed".to_owned(),
                |error| error.message(),
            )
        })
    }

    /// The message of a JavaScript error value.
    #[expect(clippy::needless_pass_by_value, reason = "used with `map_err`")]
    fn js_message(error: JsValue) -> String {
        crate::platform::js_message(&error)
    }
}

#[cfg(target_arch = "wasm32")]
pub use idb::forget_server;

#[cfg(test)]
mod tests {
    use overseer::core::api::{Terrain, Tile};
    use overseer::map::{RoomOwner, RoomReservation, RoomStat, RoomStatus};

    use super::{
        Cached, MAX_ROOMS, STAT_MAX_AGE_MS, STORES, WorldEntry, key_room, missing_stores,
        parse_stat, plan_eviction, room_key, stat_json, terrain_record, world_key, writable,
    };

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
        assert_eq!(missing_stores(&schema_1), ["stats"]);
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
            }),
            reservation: Some(RoomReservation {
                id: "u2".to_owned(),
                username: None,
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
