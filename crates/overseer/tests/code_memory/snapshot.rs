//! The session's shared Memory snapshots and observed writes, against a sharded server whose
//! player code applies queued writes a few reads later.

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use flate2::Compression;
use flate2::write::GzEncoder;
use futures_util::StreamExt as _;
use overseer::Error;
use overseer::cache::Freshness;
use overseer::memory::{
    MemoryChange, MemoryPath, MemoryValue, ObservePolicy, WriteProgress, json_at,
};
use serde_json::{Value, json};

use overseer::Overseer;
use overseer::core::ServerUrl;

use crate::server::{Request, Server, Ws};
use crate::{connect_as, routes, soon};

/// `GET /api/version` of a sharded server (default shard `shard0`).
const SHARDED_VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":100,"shards":["shard0","shard3"],"renderer":{"resources":{},"metadata":{}}}}"#;

/// Short pauses: the tests do not wait for real ticks.
fn quick() -> ObservePolicy {
    ObservePolicy::new([Duration::from_millis(5); 3])
}

/// Memory per shard, and the writes the player's code has not applied yet.
#[derive(Default)]
struct Game {
    memory: HashMap<String, Value>,
    /// Queued writes: shard, keys, change.
    queued: Vec<(String, Vec<String>, MemoryChange)>,
    /// The Memory reads after which the player's code applies the queued writes; `None`:
    /// never.
    apply_after: Option<usize>,
}

type Shared = Arc<Mutex<Game>>;

fn lock(game: &Shared) -> std::sync::MutexGuard<'_, Game> {
    game.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The value of query parameter `name`.
fn query<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request.query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

/// `value` as the server sends Memory: `gz:` and the base64 of its gzipped JSON.
fn gz(value: &Value) -> String {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(value.to_string().as_bytes()).unwrap();
    format!("gz:{}", STANDARD.encode(encoder.finish().unwrap()))
}

/// The routes of a sharded server over [`routes`], serving `game`.
fn game_routes(game: Shared) -> impl Fn(&Request) -> (u16, String) + Send + Sync + 'static {
    move |request| match (request.method.as_str(), request.path.as_str()) {
        (_, "/api/version") => (200, SHARDED_VERSION.to_owned()),
        ("GET", "/api/user/memory") => {
            let mut game = lock(&game);
            let shard = query(request, "shard").unwrap_or("shard0").to_owned();
            if let Some(left) = game.apply_after.as_mut() {
                if *left == 0 {
                    let queued = std::mem::take(&mut game.queued);
                    for (shard, keys, change) in queued {
                        let memory = game.memory.entry(shard).or_insert_with(|| json!({}));
                        let mut value = MemoryValue::Json(memory.take());
                        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
                        change.apply(&mut value, &keys);
                        if let MemoryValue::Json(value) = value {
                            *memory = value;
                        }
                    }
                } else {
                    *left -= 1;
                }
            }
            let keys: Vec<&str> = query(request, "path")
                .map(|path| path.split('.').collect())
                .unwrap_or_default();
            let memory = game.memory.get(&shard).cloned().unwrap_or(Value::Null);
            match json_at(&memory, &keys) {
                Some(value) => (200, json!({"ok": 1, "data": gz(value)}).to_string()),
                None => (200, r#"{"ok":1}"#.to_owned()),
            }
        }
        ("POST", "/api/user/memory") => {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            let path = body["path"].as_str().unwrap_or_default();
            if path == "refused" {
                return (200, r#"{"error":"memory size is too large"}"#.to_owned());
            }
            let shard = body["shard"].as_str().unwrap_or("shard0").to_owned();
            let keys = path.split('.').map(str::to_owned).collect();
            let change = body
                .get("value")
                .cloned()
                .map_or(MemoryChange::Delete, MemoryChange::Set);
            lock(&game).queued.push((shard, keys, change));
            (200, r#"{"ok":1}"#.to_owned())
        }
        _ => routes(request),
    }
}

/// Connects to `server` as a stand-in for screeps.com, whose requests name shards.
async fn connect(server: &mut Server) -> (Overseer, Ws) {
    let url = ServerUrl::proxied(server.url().base().as_str(), &ServerUrl::official()).unwrap();
    connect_as(server, url).await
}

fn path(text: &str) -> MemoryPath {
    MemoryPath::parse(text).unwrap()
}

/// The Memory reads served so far.
fn reads(server: &Server) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter(|request| request.method == "GET" && request.path == "/api/user/memory")
        .map(|request| request.query)
        .collect()
}

/// A game whose shard0 Memory holds a creep, applying writes after `apply_after` reads.
fn game(apply_after: Option<usize>) -> Shared {
    let mut game = Game {
        apply_after,
        ..Game::default()
    };
    game.memory.insert(
        "shard0".to_owned(),
        json!({"creeps": {"c1": {"role": "harvester"}}}),
    );
    game.memory
        .insert("shard3".to_owned(), json!({"rooms": ["W1N1"]}));
    Arc::new(Mutex::new(game))
}

#[tokio::test]
async fn snapshots_are_shared_by_the_session_and_share_reads() {
    let mut server = Server::start(game_routes(game(None)), |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let first = overseer
        .memory()
        .snapshot(&MemoryPath::root(), Some("shard0"))
        .unwrap();
    // Another handle of the session (a second window of the app) gets the same snapshot.
    let second = overseer
        .clone()
        .memory()
        .snapshot(&MemoryPath::root(), None)
        .unwrap();
    let mut watch = second.watch();
    assert!(first.latest().is_none());
    let freshness = Freshness::new(Duration::from_mins(5), Duration::from_secs(30));
    assert!(second.is_stale(&freshness));

    // Two reads at once: one request, one result.
    server.hold(Some("/api/user/memory"));
    let release = async {
        while reads(&server).is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(first.is_reading());
        assert!(!first.is_stale(&freshness), "a read runs");
        server.hold(None);
    };
    let (a, b, ()) =
        soon(async { tokio::join!(first.refresh(None), second.refresh(None), release) }).await;
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(reads(&server), ["shard=shard0"]);
    assert_eq!(a.revision, 1);
    assert_eq!(
        a.value,
        MemoryValue::Json(json!({"creeps": {"c1": {"role": "harvester"}}}))
    );
    assert!(soon(watch.changed()).await.is_ok());
    assert_eq!(
        watch.borrow_and_update().as_ref().map(|read| read.revision),
        Some(1)
    );

    // Fresh: no second request.
    assert!(!first.is_stale(&freshness));
    let again = first.refresh_stale(&freshness, None).await.unwrap();
    assert!(again.is_some_and(|again| Arc::ptr_eq(&again, &a)));
    assert_eq!(reads(&server).len(), 1);
    // Old: a new read, a new revision.
    let old = Freshness::new(Duration::ZERO, Duration::ZERO);
    let newer = first.refresh_stale(&old, None).await.unwrap().unwrap();
    assert_eq!(newer.revision, 2);
    assert_eq!(second.latest().map(|read| read.revision), Some(2));
    assert_eq!(reads(&server).len(), 2);

    // A path with nothing at it is an empty read: it is old after `fresh_empty`.
    let gone = overseer
        .memory()
        .snapshot(&path("gone"), Some("shard0"))
        .unwrap();
    assert_eq!(
        gone.refresh(None).await.unwrap().value,
        MemoryValue::Undefined
    );
    assert!(gone.is_stale(&Freshness::new(Duration::from_mins(5), Duration::ZERO)));
    assert!(!gone.is_stale(&freshness));
}

#[tokio::test]
async fn snapshots_of_shards_are_apart() {
    let mut server = Server::start(game_routes(game(None)), |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let memory = overseer.memory();
    let shard0 = memory.snapshot(&MemoryPath::root(), None).unwrap();
    let shard3 = memory
        .snapshot(&MemoryPath::root(), Some("shard3"))
        .unwrap();
    assert_eq!(shard0.shard(), Some("shard0"));
    assert_eq!(shard3.shard(), Some("shard3"));
    shard3.refresh(None).await.unwrap();
    assert!(
        shard0.latest().is_none(),
        "a read of shard3 is not shard0's"
    );
    assert_eq!(
        shard3.latest().unwrap().value,
        MemoryValue::Json(json!({"rooms": ["W1N1"]}))
    );
    assert_eq!(reads(&server), ["shard=shard3"]);
}

#[tokio::test]
async fn an_observed_write_changes_every_snapshot_of_its_shard() {
    let game = game(Some(1));
    let mut server = Server::start(game_routes(Arc::clone(&game)), |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let memory = overseer.memory();
    let root = memory.snapshot(&MemoryPath::root(), None).unwrap();
    let role = memory.snapshot(&path("creeps.c1.role"), None).unwrap();
    let other = memory
        .snapshot(&MemoryPath::root(), Some("shard3"))
        .unwrap();
    let before = root.refresh(None).await.unwrap();
    role.refresh(None).await.unwrap();
    other.refresh(None).await.unwrap();
    // The reads above count: the player's code applies the write after one more read.
    lock(&game).apply_after = Some(1);
    let earlier = reads(&server).len();

    let change = MemoryChange::Set(json!({"role": "builder", "level": 2.0}));
    let progress: Vec<WriteProgress> = soon(
        memory
            .write_and_observe(&path("creeps.c1"), change, None, quick())
            .collect(),
    )
    .await;
    let [WriteProgress::Queued, WriteProgress::Observed(read)] = progress.as_slice() else {
        panic!("{progress:?}");
    };
    assert_eq!(
        read.value,
        MemoryValue::Json(json!({"role": "builder", "level": 2.0}))
    );
    // Not shown by the first read, shown by the second.
    assert_eq!(reads(&server).len(), earlier + 2);

    // The snapshot above has the change, with the time of its own read.
    let after = root.latest().unwrap();
    assert_eq!(
        after.value,
        MemoryValue::Json(json!({"creeps": {"c1": {"role": "builder", "level": 2.0}}}))
    );
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(after.read_at, before.read_at);
    // The snapshot below has the written value there.
    assert_eq!(
        role.latest().unwrap().value,
        MemoryValue::Json(json!("builder"))
    );
    // Another shard does not change.
    assert_eq!(other.latest().unwrap().revision, 1);
}

#[tokio::test]
async fn a_write_that_does_not_show_in_time_is_reported() {
    let mut server = Server::start(game_routes(game(None)), |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let memory = overseer.memory();
    let root = memory.snapshot(&MemoryPath::root(), None).unwrap();
    root.refresh(None).await.unwrap();

    let progress: Vec<WriteProgress> = soon(
        memory
            .write_and_observe(&path("flag"), MemoryChange::Set(json!(true)), None, quick())
            .collect(),
    )
    .await;
    assert!(
        matches!(
            progress.as_slice(),
            [WriteProgress::Queued, WriteProgress::NotObserved]
        ),
        "{progress:?}"
    );
    // At most three reads of the written path: screeps.com limits Memory reads per day.
    assert_eq!(
        reads(&server),
        [
            "shard=shard0",
            "path=flag&shard=shard0",
            "path=flag&shard=shard0",
            "path=flag&shard=shard0"
        ]
    );
    assert_eq!(root.latest().unwrap().revision, 1);
}

#[tokio::test]
async fn a_refused_write_is_reported_without_reads() {
    let mut server = Server::start(game_routes(game(Some(0))), |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let progress: Vec<WriteProgress> = soon(
        overseer
            .memory()
            .write_and_observe(&path("refused"), MemoryChange::Delete, None, quick())
            .collect(),
    )
    .await;
    assert!(
        matches!(progress.as_slice(), [WriteProgress::Failed(Error::Net(_))]),
        "{progress:?}"
    );
    assert_eq!(reads(&server), Vec::<String>::new());

    overseer.shutdown().await;
    let progress: Vec<WriteProgress> = overseer
        .memory()
        .write_and_observe(&path("x"), MemoryChange::Delete, None, quick())
        .collect()
        .await;
    assert!(
        matches!(progress.as_slice(), [WriteProgress::Failed(Error::Closed)]),
        "{progress:?}"
    );
}
