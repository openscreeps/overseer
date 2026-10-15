//! Room terrain kept across restarts in `IndexedDB`: terrain never changes, so a room fetched
//! once is never fetched again.
//!
//! # Storage
//!
//! Database `overseer-terrain` (schema version 1) with two object stores:
//!
//! - `terrain`: one record per room under the key `<FORMAT>|<server URL>|<shard>|<room>`
//!   ([`room_key`]; `<shard>` is `-` on single-world servers), its value the terrain packed
//!   2 bits per tile ([`pack`], [`PACKED_LEN`] = 625 bytes).
//! - `worlds`: per `<FORMAT>|<server URL>|<shard>|` prefix ([`world_key`]) when it was last
//!   opened, in milliseconds since the epoch.
//!
//! The record format is versioned by [`FORMAT`] in the key: records of another format are
//! deleted when the cache opens.
//!
//! # Bound
//!
//! At most [`MAX_ROOMS`] rooms (about 31 MB of terrain) are kept. Past the bound, whole
//! worlds are evicted, least recently opened first and never the open one, down to
//! [`TARGET_ROOMS`] ([`plan_eviction`]); when the open world alone fills the bound, its
//! further rooms are not stored ([`writable`]).
//!
//! On a world's start the app loads all its cached rooms and hands them to
//! `WorldMap::insert_terrain` and `Viewer::set_terrain`; rooms the library fetches later
//! (`WorldEvent::Terrain`) are written in batches (`TerrainCache::store`, browsers only).

use overseer::core::api::{Terrain, Tile};
use overseer::map::RoomCoord;

/// Record format, the first part of every key.
pub const FORMAT: u32 = 1;
/// Most rooms kept.
pub const MAX_ROOMS: u32 = 50_000;
/// Rooms kept after an eviction.
pub const TARGET_ROOMS: u32 = 45_000;
/// Bytes of a packed terrain: 2500 tiles, 2 bits each.
pub const PACKED_LEN: usize = 625;

/// Tiles per room side.
const SIZE: u8 = 50;

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

/// Packs `terrain` row by row, 4 tiles per byte from the low bits: 0 plain, 1 wall, 2 swamp.
pub fn pack(terrain: &Terrain) -> [u8; PACKED_LEN] {
    let mut packed = [0; PACKED_LEN];
    let mut index = 0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let bits = match terrain.get(x, y) {
                Some(Tile::Wall) => 1,
                Some(Tile::Swamp) => 2,
                Some(Tile::Plain) | None => 0,
            };
            packed[index / 4] |= bits << ((index % 4) * 2);
            index += 1;
        }
    }
    packed
}

/// Unpacks [`pack`]'s form; `None` unless `packed` is [`PACKED_LEN`] bytes of valid tiles.
pub fn unpack(packed: &[u8]) -> Option<Terrain> {
    if packed.len() != PACKED_LEN {
        return None;
    }
    let mut digits = String::with_capacity(PACKED_LEN * 4);
    for byte in packed {
        for shift in [0, 2, 4, 6] {
            digits.push(match (byte >> shift) & 3 {
                0 => '0',
                1 => '1',
                2 => '2',
                _ => return None,
            });
        }
    }
    Terrain::decode(&digits).ok()
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
    use overseer::map::RoomCoord;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        IdbDatabase, IdbKeyRange, IdbOpenDbRequest, IdbRequest, IdbTransaction, IdbTransactionMode,
    };

    use super::{
        FORMAT, WorldEntry, key_room, pack, plan_eviction, room_key, unpack, world_key, writable,
    };

    /// `IndexedDB` database name.
    const DB_NAME: &str = "overseer-terrain";
    /// `IndexedDB` schema version (object stores and indexes).
    const DB_VERSION: u32 = 1;
    /// Store of the terrain records.
    const TERRAIN: &str = "terrain";
    /// Store of the worlds' last-opened times.
    const WORLDS: &str = "worlds";
    /// How long terrain waits to be written with the rooms arriving after it.
    const FLUSH_DELAY: Duration = Duration::from_secs(1);

    /// Rooms and their terrain.
    type Rooms = Vec<(RoomCoord, Arc<Terrain>)>;

    /// The terrain cache of one world; cheap to clone.
    #[derive(Clone)]
    pub struct TerrainCache {
        db: IdbDatabase,
        world: Rc<str>,
        /// Rooms stored across all worlds, as last counted plus writes since.
        stored: Rc<Cell<u32>>,
        /// Rooms waiting to be written.
        pending: Rc<RefCell<Rooms>>,
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

        /// Every cached room of the world.
        ///
        /// # Errors
        ///
        /// Returns a message when `IndexedDB` fails.
        pub async fn load(&self) -> Result<Rooms, String> {
            let transaction = self.transaction(&[TERRAIN], IdbTransactionMode::Readonly)?;
            let store = transaction.object_store(TERRAIN).map_err(js_message)?;
            let range = prefix_range(&self.world)?;
            let keys = request(&store.get_all_keys_with_key(&range).map_err(js_message)?).await?;
            let values = request(&store.get_all_with_key(&range).map_err(js_message)?).await?;
            let (keys, values) = (Array::from(&keys), Array::from(&values));
            let mut rooms = Vec::with_capacity(keys.length() as usize);
            for (key, value) in keys.iter().zip(values.iter()) {
                let Some(room) = key.as_string().and_then(|key| key_room(&self.world, &key)) else {
                    continue;
                };
                let Some(bytes) = value.dyn_ref::<Uint8Array>() else {
                    continue;
                };
                if let Some(terrain) = unpack(&bytes.to_vec()) {
                    rooms.push((room, Arc::new(terrain)));
                }
            }
            Ok(rooms)
        }

        /// Stores `terrain` of `room`: rooms arriving within [`FLUSH_DELAY`] are written in
        /// one transaction. Best effort: a room that fails to be written is fetched again
        /// next time.
        pub fn store(&self, room: RoomCoord, terrain: Arc<Terrain>) {
            let first = {
                let mut pending = self.pending.borrow_mut();
                pending.push((room, terrain));
                pending.len() == 1
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

        /// Writes the queued rooms in one transaction, as many as the bound allows.
        async fn flush(&self) -> Result<(), String> {
            let rooms = std::mem::take(&mut *self.pending.borrow_mut());
            let wanted = u32::try_from(rooms.len()).unwrap_or(u32::MAX);
            let allowed = writable(self.stored.get(), wanted);
            if allowed == 0 {
                return Ok(());
            }
            let transaction = self.transaction(&[TERRAIN], IdbTransactionMode::Readwrite)?;
            let store = transaction.object_store(TERRAIN).map_err(js_message)?;
            for (room, terrain) in rooms.iter().take(allowed as usize) {
                let value = Uint8Array::from(pack(terrain).as_slice());
                store
                    .put_with_key(&value, &JsValue::from_str(&room_key(&self.world, *room)))
                    .map_err(js_message)?;
            }
            done(&transaction).await?;
            self.stored.set(self.stored.get().saturating_add(allowed));
            Ok(())
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
            let transaction =
                self.transaction(&[TERRAIN, WORLDS], IdbTransactionMode::Readwrite)?;
            for name in [TERRAIN, WORLDS] {
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

        /// Counts the rooms of every world and deletes the worlds [`plan_eviction`] picks.
        async fn evict(&self) -> Result<(), String> {
            let transaction =
                self.transaction(&[TERRAIN, WORLDS], IdbTransactionMode::Readwrite)?;
            let terrain = transaction.object_store(TERRAIN).map_err(js_message)?;
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

    /// Deletes every cached room of the server with base URL `server` (all its worlds).
    ///
    /// # Errors
    ///
    /// Returns a message when `IndexedDB` fails.
    pub async fn forget_server(server: &str) -> Result<(), String> {
        let db = open_db().await?;
        let prefix = format!("{FORMAT}|{server}|");
        let names: Array = [TERRAIN, WORLDS]
            .iter()
            .map(|name| JsValue::from_str(name))
            .collect();
        let transaction = db
            .transaction_with_str_sequence_and_mode(&names, IdbTransactionMode::Readwrite)
            .map_err(js_message)?;
        for name in [TERRAIN, WORLDS] {
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
                // Schema 1: recreate both stores (no earlier schema exists).
                for name in [TERRAIN, WORLDS] {
                    if db.object_store_names().contains(name) {
                        let _ = db.delete_object_store(name);
                    }
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

    use super::{
        MAX_ROOMS, PACKED_LEN, WorldEntry, key_room, pack, plan_eviction, room_key, unpack,
        world_key, writable,
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
    fn packs_losslessly() {
        let terrain = terrain();
        let packed = pack(&terrain);
        assert_eq!(packed.len(), PACKED_LEN);
        let unpacked = unpack(&packed).unwrap();
        assert_eq!(unpacked, terrain);
        assert_eq!(unpacked.get(5, 0), Some(Tile::Wall), "wall bit wins");
        assert_eq!(unpack(&packed[1..]), None);
        let mut invalid = packed;
        invalid[0] = 0b11;
        assert_eq!(unpack(&invalid), None);
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
            world("old", 1.0, 20_000),
            world("open", 0.5, 20_000),
            world("older", 0.0, 3_000),
            world("recent", 3.0, 10_000),
        ];
        // 53 000 rooms: drop "older" (50 000 left, above the target), then "old".
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
