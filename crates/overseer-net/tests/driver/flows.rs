//! Client flows over HTTP: rate-limit retries, the terrain cache, room history, and the
//! Memory, console and code operations of the domain API.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::GzEncoder;
use overseer_core::Token;
use overseer_core::api::official::endpoints;
use overseer_core::api::{ApiError, RateLimit};
use overseer_core::model::code::{CodeTarget, Modules};
use overseer_core::model::memory::MemoryValue;
use overseer_core::model::world::Tile;
use overseer_net::api::{CodeApi as _, ConsoleApi as _, MemoryApi as _, WorldApi as _};
use overseer_net::{Client, Error, GLOBAL_RATE_LIMIT_WAIT, Priority, RateLimitPolicy};
use tokio::time::Instant;

use crate::server::{Request, Response, Server};
use crate::{ME, VERSION};

/// `GET /api/version` of a server with one shard and 100-tick history chunks.
const SHARDED_VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":100,"shards":["shard0"],"renderer":{"resources":{},"metadata":{}}}}"#;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn limited(limit: u32, remaining: u32, reset: u64, status: u16, body: &str) -> Response {
    Response {
        status,
        headers: vec![
            ("X-RateLimit-Limit", limit.to_string()),
            ("X-RateLimit-Remaining", remaining.to_string()),
            ("X-RateLimit-Reset", reset.to_string()),
        ],
        body: body.as_bytes().to_vec(),
    }
}

/// `json` compressed for `Content-Encoding: gzip`, as the official history host serves it.
fn gzip(json: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(json.as_bytes()).unwrap();
    encoder.finish().unwrap()
}

/// Serves `/api/auth/me`: the first `rejections` calls get `rejected`, later ones succeed with
/// 59 of 60 requests left.
async fn rate_limited_server(
    rejections: usize,
    rejected: impl Fn() -> Response + Send + Sync + 'static,
    reset: u64,
) -> Server {
    let calls = AtomicUsize::new(0);
    Server::start(
        move |request: &Request| match request.path.as_str() {
            "/api/auth/me" if calls.fetch_add(1, Ordering::SeqCst) < rejections => rejected(),
            "/api/auth/me" => limited(60, 59, reset, 200, ME),
            _ => (404, "{}".to_owned()).into(),
        },
        |_| true,
    )
    .await
}

fn client(server: &Server, policy: Option<RateLimitPolicy>) -> Client {
    Client::builder(server.url())
        .token(Token::new("t"))
        .rate_limit(policy)
        .build()
        .unwrap()
}

#[tokio::test]
async fn endpoint_rate_limits_wait_for_the_reset_and_retry_once() {
    let reset = now_secs();
    let server = rate_limited_server(1, move || limited(60, 0, reset, 429, "{}"), reset + 60).await;
    let client = client(&server, Some(RateLimitPolicy::default()));
    assert_eq!(client.rate_limited_count(), 0);
    let started = Instant::now();
    let me = client.send(&endpoints::AuthMe).await.unwrap();
    assert_eq!(me.body.id, "u1");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(server.requests().len(), 2);
    // The refusal counts, though the retry succeeded.
    assert_eq!(client.rate_limited_count(), 1);
    let expected = RateLimit {
        limit: 60,
        remaining: 59,
        reset: reset + 60,
    };
    assert_eq!(client.rate_limit::<endpoints::AuthMe>(), Some(expected));
    assert_eq!(client.rate_limit::<endpoints::GameTime>(), None);
    let all = client.rate_limits();
    assert_eq!(all.len(), 1);
    assert_eq!(
        (all[0].method.as_str(), all[0].path, all[0].limit),
        ("GET", "api/auth/me", expected)
    );
}

#[tokio::test]
async fn endpoint_rate_limits_beyond_the_maximum_wait_are_returned() {
    let reset = now_secs() + 3600;
    let server = rate_limited_server(1, move || limited(60, 0, reset, 429, "{}"), reset).await;
    let client = client(&server, Some(RateLimitPolicy::default()));
    let error = client.send(&endpoints::AuthMe).await.unwrap_err();
    let rejected = RateLimit {
        limit: 60,
        remaining: 0,
        reset,
    };
    assert!(
        matches!(error, Error::Api(ApiError::RateLimited(Some(limit))) if limit == rejected),
        "{error:?}"
    );
    assert_eq!(server.requests().len(), 1);
    assert_eq!(client.rate_limit::<endpoints::AuthMe>(), Some(rejected));
    // A limit known to be stale (lifted outside the client) is forgotten until the next reply.
    client.forget_rate_limit::<endpoints::AuthMe>();
    assert_eq!(client.rate_limit::<endpoints::AuthMe>(), None);
    assert_eq!(client.rate_limits(), []);
}

#[tokio::test]
async fn global_rate_limits_retry_after_the_delay() {
    let server = rate_limited_server(1, || (429, "{}".to_owned()).into(), now_secs()).await;
    let policy = RateLimitPolicy {
        max_wait: Duration::ZERO,
        global_delay: Duration::from_millis(100),
    };
    let client = client(&server, Some(policy));
    let started = Instant::now();
    client.send(&endpoints::AuthMe).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert_eq!(server.requests().len(), 2);
}

#[tokio::test]
async fn rate_limited_requests_are_retried_only_once_and_never_when_disabled() {
    let server = rate_limited_server(2, || (429, "{}".to_owned()).into(), now_secs()).await;
    let policy = RateLimitPolicy {
        max_wait: Duration::ZERO,
        global_delay: Duration::from_millis(10),
    };
    let retrying = client(&server, Some(policy));
    let error = retrying.send(&endpoints::AuthMe).await.unwrap_err();
    // Both refusals count, server-wide ones (no headers) too.
    assert_eq!(retrying.rate_limited_count(), 2);
    assert!(
        matches!(error, Error::Api(ApiError::RateLimited(None))),
        "{error:?}"
    );
    assert_eq!(server.requests().len(), 2);

    let server = rate_limited_server(1, || (429, "{}".to_owned()).into(), now_secs()).await;
    let error = client(&server, None)
        .send(&endpoints::AuthMe)
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Api(ApiError::RateLimited(None))),
        "{error:?}"
    );
    assert_eq!(server.requests().len(), 1);
}

#[tokio::test]
async fn budget_waits_follow_the_budget_and_cancel_when_dropped() {
    let server = rate_limited_server(1, || (429, "{}".to_owned()).into(), now_secs()).await;
    let client = client(&server, None);
    // No limit seen yet: no budget, no wait.
    let started = Instant::now();
    client.wait_for_budget(Priority::Background, 0).await;
    assert!(started.elapsed() < Duration::from_secs(1));
    client.send(&endpoints::AuthMe).await.unwrap_err();
    // HTTP 429 without the endpoint's limit: every priority waits a minute.
    let ready = client.budget_ready_at(Priority::Viewed, 0).unwrap();
    assert!(ready > Instant::now() + GLOBAL_RATE_LIMIT_WAIT - Duration::from_secs(5));
    let cancelled = tokio::time::timeout(
        Duration::from_millis(100),
        client.wait_for_budget(Priority::Viewed, 0),
    )
    .await;
    assert!(cancelled.is_err(), "dropped before the budget allows");
    // A token exempt from rate limits never waits.
    client.set_rate_limit_exempt_until(Some(u64::MAX));
    let started = Instant::now();
    client.wait_for_budget(Priority::Background, 1000).await;
    assert!(started.elapsed() < Duration::from_secs(1));
}

/// An encoded terrain: wall at (0, 0), swamp at (1, 0), wall-and-swamp at (2, 0), plain
/// elsewhere.
fn encoded_terrain() -> String {
    format!("123{}", "0".repeat(2497))
}

/// A single-world server with terrain for `W1N1` only.
pub fn terrain_routes(request: &Request) -> (u16, String) {
    match request.path.as_str() {
        "/api/version" => (200, VERSION.to_owned()),
        "/api/game/room-terrain" if request.query.contains("room=W1N1") => (
            200,
            format!(
                r#"{{"ok":1,"terrain":[{{"_id":"t1","room":"W1N1","terrain":"{}","type":"terrain"}}]}}"#,
                encoded_terrain()
            ),
        ),
        "/api/game/room-terrain" => (200, r#"{"error":"invalid room"}"#.to_owned()),
        _ => (404, "{}".to_owned()),
    }
}

#[tokio::test]
async fn terrain_is_decoded_and_fetched_once_per_room() {
    let server = Server::start(terrain_routes, |_| true).await;
    let client = Client::builder(server.url()).build().unwrap();
    let first = client.terrain(None, "W1N1").await.unwrap();
    let again = client
        .clone()
        .terrain(Some("shard9"), "W1N1")
        .await
        .unwrap();
    assert!(
        Arc::ptr_eq(&first, &again),
        "single-world servers ignore shards"
    );
    assert_eq!(first.get(0, 0), Some(Tile::Wall));
    assert_eq!(first.get(1, 0), Some(Tile::Swamp));
    assert_eq!(first.get(2, 0), Some(Tile::Wall));
    assert_eq!(first.get(49, 49), Some(Tile::Plain));

    let terrain_requests: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/game/room-terrain")
        .collect();
    assert_eq!(terrain_requests.len(), 1);
    assert!(terrain_requests[0].query.contains("encoded=1"));
    assert!(!terrain_requests[0].query.contains("shard"));

    let error = client.terrain(None, "W9N9").await.unwrap_err();
    assert!(
        matches!(&error, Error::Api(ApiError::Server(message)) if message == "invalid room"),
        "{error:?}"
    );
}

/// A history chunk of `W1N1` from tick 40: a source and a wall, the source harvested at 41, the
/// wall destroyed and a road built at 43.
const CHUNK: &str = r#"{"base":40,"timestamp":1760000000000,"ticks":{
    "43":{"b":null,"c":{"_id":"c","type":"road","x":2,"y":2,"room":"W1N1"}},
    "40":{"a":{"_id":"a","type":"source","x":10,"y":10,"room":"W1N1","energy":3000},
          "b":{"_id":"b","type":"constructedWall","x":1,"y":1,"room":"W1N1"}},
    "41":{"a":{"energy":2990}}}}"#;

#[tokio::test]
async fn single_world_history_is_fetched_by_chunk_and_replayed() {
    let server = Server::start(
        |request: &Request| match request.path.as_str() {
            "/api/version" => (200, VERSION.to_owned()),
            "/room-history" => (200, CHUNK.to_owned()),
            _ => (404, "{}".to_owned()),
        },
        |_| true,
    )
    .await;
    let client = Client::builder(server.url()).build().unwrap();
    // 20-tick chunks: tick 47 is in the chunk from 40.
    let api = client.api().await.unwrap();
    let history = api.room_history(None, "W1N1", 47).await.unwrap();
    let request = server.requests().pop().unwrap();
    assert_eq!(request.path, "/room-history");
    assert_eq!(request.query, "room=W1N1&time=40");

    assert_eq!(history.room(), "W1N1");
    assert_eq!(history.base(), 40);
    assert_eq!(history.ticks().collect::<Vec<_>>(), [40, 41, 43]);
    let states: Vec<_> = history.replay().collect();
    assert_eq!(states.len(), 3);
    let energy =
        |state: &overseer_core::socket::RoomState| state.object("a").map(|a| a["energy"].clone());
    assert_eq!(energy(&states[0].1), Some(3000.into()));
    assert_eq!(states[0].1.objects().count(), 2);
    assert_eq!(energy(&states[1].1), Some(2990.into()));
    assert_eq!(states[1].1.game_time(), Some(41));
    let (tick, last) = &states[2];
    assert_eq!(*tick, 43);
    assert!(last.object("b").is_none());
    assert_eq!(last.object("c").unwrap()["type"], "road");
    assert_eq!(energy(last), Some(2990.into()));
}

#[tokio::test]
async fn sharded_history_reads_the_gzip_encoded_static_chunk_files() {
    let server = Server::start(
        |request: &Request| match request.path.as_str() {
            "/api/version" => (200, SHARDED_VERSION.to_owned()).into(),
            "/room-history/shard0/W1N1/100.json" => Response {
                status: 200,
                headers: vec![("Content-Encoding", "gzip".to_owned())],
                body: gzip(
                    r#"{"base":100,"timestamp":1,"ticks":{"100":{"a":{"_id":"a","type":"road","x":1,"y":1,"room":"W1N1"}}}}"#,
                ),
            },
            _ => (404, "{}".to_owned()).into(),
        },
        |_| true,
    )
    .await;
    let client = Client::builder(server.official_url()).build().unwrap();
    let api = client.api().await.unwrap();
    let history = api.room_history(None, "W1N1", 150).await.unwrap();
    assert_eq!((history.room(), history.base()), ("W1N1", 100));
    let states: Vec<_> = history.replay().collect();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].1.object("a").unwrap()["type"], "road");
    let error = api.room_history(None, "W1N1", 250).await.unwrap_err();
    assert!(
        matches!(&error, Error::Api(ApiError::Status { status, .. }) if status.as_u16() == 404),
        "{error:?}"
    );
}

/// Sends every Memory, segment, console and code operation through the domain API of an
/// `OpenScreeps` server, or of an official one with shard `shard0`, and returns the requests
/// under `/api/user/`.
async fn user_requests(official: bool) -> Vec<Request> {
    let version = if official { SHARDED_VERSION } else { VERSION };
    // screeps.com echoes the queued console document; the others answer `{ok: 1}`.
    let console = if official {
        r#"{"ok":1,"result":{"ok":1,"n":1},"ops":[{"user":"u1","expression":"Game.time","shard":"shard0","_id":"c1"}],"insertedCount":1,"insertedIds":["c1"]}"#
    } else {
        r#"{"ok":1}"#
    };
    let server = Server::start(
        move |request: &Request| match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/api/version") => (200, version.to_owned()),
            ("GET", "/api/auth/me") => (200, ME.to_owned()),
            ("GET", "/api/user/memory") => {
                (200, r#"{"ok":1,"data":"Incorrect memory path"}"#.to_owned())
            }
            ("GET", "/api/user/memory-segment") => (200, r#"{"ok":1,"data":null}"#.to_owned()),
            ("GET", "/api/user/branches") => (
                200,
                r#"{"ok":1,"list":[{"_id":"b1","branch":"default","activeWorld":true}]}"#
                    .to_owned(),
            ),
            ("GET", "/api/user/code") => (
                200,
                r#"{"ok":1,"branch":"default","modules":{"main":"x"}}"#.to_owned(),
            ),
            ("POST", "/api/user/console") => (200, console.to_owned()),
            ("POST", _) => (200, r#"{"ok":1,"timestamp":1}"#.to_owned()),
            _ => (404, "{}".to_owned()),
        },
        |_| true,
    )
    .await;
    let url = if official {
        server.official_url()
    } else {
        server.url()
    };
    let client = Client::builder(url).token(Token::new("t")).build().unwrap();
    let api = client.api().await.unwrap();

    assert_eq!(
        api.memory("a.b", None).await.unwrap(),
        MemoryValue::IncorrectPath
    );
    api.set_memory("a.b", 5.into(), None).await.unwrap();
    api.delete_memory("a.b", None).await.unwrap();
    assert_eq!(api.memory_segment(3, None).await.unwrap(), None);
    api.set_memory_segment(3, "text", None).await.unwrap();
    api.console("Game.time", None).await.unwrap();

    let branches = api.branches().await.unwrap();
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].active_world, Some(true));
    let code = api.code(CodeTarget::World.selector()).await.unwrap();
    assert_eq!(
        (code.branch.as_str(), code.modules.js("main")),
        ("default", Some("x"))
    );
    let mut modules = Modules::new();
    modules.insert_js("main", "loop");
    let sent = api.set_code("default", modules.clone()).await.unwrap();
    assert_eq!(sent, modules);
    api.clone_branch("default", "next").await.unwrap();
    api.create_branch("fresh", modules).await.unwrap();
    api.delete_branch("old").await.unwrap();
    api.set_active_branch(CodeTarget::Simulation, "next")
        .await
        .unwrap();

    server
        .requests()
        .into_iter()
        .filter(|request| request.path.starts_with("/api/user/"))
        .collect()
}

/// The JSON body of `request`.
fn body(request: &Request) -> serde_json::Value {
    serde_json::from_str(&request.body).unwrap()
}

#[tokio::test]
async fn user_operations_send_the_documented_requests() {
    use serde_json::json;

    let requests = user_requests(false).await;
    let summary: Vec<_> = requests
        .iter()
        .map(|request| (request.method.as_str(), request.path.as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            ("GET", "/api/user/memory"),
            ("POST", "/api/user/memory"),
            ("POST", "/api/user/memory"),
            ("GET", "/api/user/memory-segment"),
            ("POST", "/api/user/memory-segment"),
            ("POST", "/api/user/console"),
            ("GET", "/api/user/branches"),
            ("GET", "/api/user/code"),
            ("POST", "/api/user/code"),
            ("POST", "/api/user/clone-branch"),
            ("POST", "/api/user/clone-branch"),
            ("POST", "/api/user/delete-branch"),
            ("POST", "/api/user/set-active-branch"),
        ]
    );
    // Single-world servers get no shard.
    assert_eq!(requests[0].query, "path=a.b");
    assert_eq!(body(&requests[1]), json!({"path": "a.b", "value": 5}));
    assert_eq!(body(&requests[2]), json!({"path": "a.b"}));
    assert_eq!(requests[3].query, "segment=3");
    assert_eq!(body(&requests[4]), json!({"data": "text", "segment": 3}));
    assert_eq!(body(&requests[5]), json!({"expression": "Game.time"}));
    assert_eq!(requests[6].query, "withCode=false");
    assert_eq!(requests[7].query, "branch=%24activeWorld");
    assert_eq!(
        body(&requests[8]),
        json!({"branch": "default", "modules": {"main": "loop"}})
    );
    assert_eq!(
        body(&requests[9]),
        json!({"branch": "default", "newName": "next"})
    );
    assert_eq!(
        body(&requests[10]),
        json!({"defaultModules": {"main": "loop"}, "newName": "fresh"})
    );
    assert_eq!(body(&requests[11]), json!({"branch": "old"}));
    assert_eq!(
        body(&requests[12]),
        json!({"activeName": "activeSim", "branch": "next"})
    );
}

#[tokio::test]
async fn official_user_operations_name_the_default_shard() {
    use serde_json::json;

    let requests = user_requests(true).await;
    assert_eq!(requests.len(), 13);
    assert_eq!(requests[0].query, "path=a.b&shard=shard0");
    assert_eq!(
        body(&requests[1]),
        json!({"path": "a.b", "shard": "shard0", "value": 5})
    );
    assert_eq!(
        body(&requests[2]),
        json!({"path": "a.b", "shard": "shard0"})
    );
    assert_eq!(requests[3].query, "segment=3&shard=shard0");
    assert_eq!(
        body(&requests[4]),
        json!({"data": "text", "segment": 3, "shard": "shard0"})
    );
    assert_eq!(
        body(&requests[5]),
        json!({"expression": "Game.time", "shard": "shard0"})
    );
}
