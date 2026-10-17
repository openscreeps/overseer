//! The `IndexedDB` side of the terrain cache (browsers only).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use js_sys::{Array, Function, Promise, Uint8Array};
use overseer::map::{Portal, RoomCoord, RoomStat, WorldEvent};
use overseer::model::world::Terrain;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    IdbDatabase, IdbKeyRange, IdbObjectStore, IdbOpenDbRequest, IdbRequest, IdbTransaction,
    IdbTransactionMode,
};

use super::{
    BADGES, Cached, FORMAT, MAX_BADGES, PORTALS, STATS, STORES, TERRAIN, WORLDS, WorldEntry,
    badge_key, key_room, missing_stores, parse_stat, plan_eviction, room_key, stat_json,
    terrain_record, world_key, writable,
};
use crate::badges::BADGE_BYTES;
use crate::portals::{parse_portals, portals_json};

/// `IndexedDB` database name.
const DB_NAME: &str = "overseer-terrain";
/// `IndexedDB` schema version (object stores): 1 had `terrain` and `worlds`, 2 adds
/// `stats`, 3 adds `portals` and `badges`.
const DB_VERSION: u32 = 3;
/// How long records wait to be written with those arriving after them.
const FLUSH_DELAY: Duration = Duration::from_secs(1);
/// Most records written in one transaction: each is encoded and put on the main thread,
/// so thousands at once (an overview response, a second of portal scans) are written in
/// turns of a few milliseconds, the browser handling input and frames in between.
const FLUSH_CHUNK: usize = 250;

/// What a world's cache holds.
#[derive(Default)]
pub struct Loaded {
    /// Rooms and their terrain.
    pub terrain: Vec<(RoomCoord, Arc<Terrain>)>,
    /// Rooms the server has no terrain for.
    pub no_terrain: Vec<RoomCoord>,
    /// Rooms and their overview, with when it was fetched.
    pub stats: Vec<(RoomCoord, Arc<RoomStat>)>,
    /// Rooms scanned for portals, their portals and when they were scanned.
    pub portals: Vec<(RoomCoord, Arc<[Portal]>, u64)>,
}

/// A record waiting to be written.
enum Write {
    /// A terrain record: the terrain, or `None` for a room without one.
    Terrain(RoomCoord, Option<Arc<Terrain>>),
    /// An overview record.
    Stat(RoomCoord, Arc<RoomStat>),
    /// A scanned room's portals and when it was scanned.
    Portals(RoomCoord, Arc<[Portal]>, u64),
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
        let transaction =
            self.transaction(&[TERRAIN, STATS, PORTALS], IdbTransactionMode::Readonly)?;
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
        let now = now_ms();
        for (room, value) in self.records(&transaction, STATS).await? {
            if let Some(stat) = value.as_string().and_then(|json| parse_stat(&json, now)) {
                loaded.stats.push((room, Arc::new(stat)));
            }
        }
        for (room, value) in self.records(&transaction, PORTALS).await? {
            if let Some((portals, scanned_at)) = value
                .as_string()
                .and_then(|json| parse_portals(&json, room))
            {
                loaded.portals.push((room, portals.into(), scanned_at));
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

    /// Stores what `event` brought: a terrain, a room without terrain, overviews, or a
    /// scanned room's portals (scanned now). Records arriving within [`FLUSH_DELAY`] are
    /// written in one transaction. Best effort: a record that fails to be written is
    /// fetched again next time.
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
                WorldEvent::Portals(room, portals) => {
                    pending.push(Write::Portals(*room, Arc::clone(portals), now_ms()));
                }
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

    /// Writes the queued records, [`FLUSH_CHUNK`] per transaction, terrain as far as the
    /// bound allows.
    async fn flush(&self) -> Result<(), String> {
        let writes = std::mem::take(&mut *self.pending.borrow_mut());
        for chunk in writes.chunks(FLUSH_CHUNK) {
            self.write(chunk).await?;
        }
        Ok(())
    }

    /// Writes `writes` in one transaction, terrain as far as the bound allows.
    async fn write(&self, writes: &[Write]) -> Result<(), String> {
        let terrain_count = writes
            .iter()
            .filter(|write| matches!(write, Write::Terrain(..)))
            .count();
        let mut allowed = writable(
            self.stored.get(),
            u32::try_from(terrain_count).unwrap_or(u32::MAX),
        );
        let written = allowed;
        let transaction =
            self.transaction(&[TERRAIN, STATS, PORTALS], IdbTransactionMode::Readwrite)?;
        let terrain = transaction.object_store(TERRAIN).map_err(js_message)?;
        let stats = transaction.object_store(STATS).map_err(js_message)?;
        let portals = transaction.object_store(PORTALS).map_err(js_message)?;
        for write in writes {
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
                Write::Portals(room, list, scanned_at) => {
                    let record = portals_json(list, *scanned_at);
                    self.put(&portals, *room, &JsValue::from_str(&record))?;
                }
            }
        }
        done(&transaction).await?;
        self.stored.set(self.stored.get().saturating_add(written));
        Ok(())
    }

    /// Puts `value` under the key of `room` into `store`.
    fn put(&self, store: &IdbObjectStore, room: RoomCoord, value: &JsValue) -> Result<(), String> {
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
    /// picks (their terrain, overviews and portals).
    async fn evict(&self) -> Result<(), String> {
        let transaction = self.transaction(&STORES, IdbTransactionMode::Readwrite)?;
        let terrain = transaction.object_store(TERRAIN).map_err(js_message)?;
        let stats = transaction.object_store(STATS).map_err(js_message)?;
        let portals = transaction.object_store(PORTALS).map_err(js_message)?;
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
            portals.delete(&prefix_range(key)?).map_err(js_message)?;
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

/// The rasterized badges of one server; cheap to clone.
#[derive(Clone)]
pub struct BadgeStore {
    db: IdbDatabase,
    server: Rc<str>,
}

impl BadgeStore {
    /// Opens the badges of the server with base URL `server`; empties the badge store
    /// when it holds more than [`MAX_BADGES`] records.
    ///
    /// # Errors
    ///
    /// Returns a message when `IndexedDB` is unavailable or fails.
    pub async fn open(server: &str) -> Result<Self, String> {
        let store = Self {
            db: open_db().await?,
            server: server.into(),
        };
        let transaction = store.transaction(IdbTransactionMode::Readwrite)?;
        let badges = transaction.object_store(BADGES).map_err(js_message)?;
        let count = request(&badges.count().map_err(js_message)?).await?;
        if count.as_f64().unwrap_or(0.0) > f64::from(MAX_BADGES) {
            badges.clear().map_err(js_message)?;
        }
        done(&transaction).await?;
        Ok(store)
    }

    /// The pixels of `user`'s badge with parameter hash `hash`, if cached.
    ///
    /// # Errors
    ///
    /// Returns a message when `IndexedDB` fails.
    pub async fn get(&self, user: &str, hash: u64) -> Result<Option<Vec<u8>>, String> {
        let transaction = self.transaction(IdbTransactionMode::Readonly)?;
        let badges = transaction.object_store(BADGES).map_err(js_message)?;
        let key = JsValue::from_str(&badge_key(&self.server, user, hash));
        let value = request(&badges.get(&key).map_err(js_message)?).await?;
        Ok(value
            .dyn_ref::<Uint8Array>()
            .filter(|bytes| bytes.length() as usize == BADGE_BYTES)
            .map(Uint8Array::to_vec))
    }

    /// Stores the pixels of `user`'s badge with parameter hash `hash`.
    ///
    /// # Errors
    ///
    /// Returns a message when `IndexedDB` fails.
    pub async fn put(&self, user: &str, hash: u64, pixels: &[u8]) -> Result<(), String> {
        let transaction = self.transaction(IdbTransactionMode::Readwrite)?;
        transaction
            .object_store(BADGES)
            .map_err(js_message)?
            .put_with_key(
                &Uint8Array::from(pixels),
                &JsValue::from_str(&badge_key(&self.server, user, hash)),
            )
            .map_err(js_message)?;
        done(&transaction).await
    }

    /// A transaction on the badge store.
    fn transaction(&self, mode: IdbTransactionMode) -> Result<IdbTransaction, String> {
        self.db
            .transaction_with_str_and_mode(BADGES, mode)
            .map_err(js_message)
    }
}

/// `Date.now()` in milliseconds since the epoch.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the epoch time in milliseconds is a non-negative integer far below u64::MAX"
)]
fn now_ms() -> u64 {
    js_sys::Date::now() as u64
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
