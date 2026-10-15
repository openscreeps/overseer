//! Client flows over HTTP: rate-limit retries, the terrain cache, room history, Memory and
//! console.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use flate2::Compression;
use flate2::write::GzEncoder;
use overseer_core::Token;
use overseer_core::api::{ApiError, MemoryValue, RateLimit, Tile, endpoints};
use overseer_net::{Client, Error, RateLimitPolicy};
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
    let started = Instant::now();
    let me = client.send(&endpoints::AuthMe).await.unwrap();
    assert_eq!(me.body.id, "u1");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(server.requests().len(), 2);
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
    let error = client(&server, Some(policy))
        .send(&endpoints::AuthMe)
        .await
        .unwrap_err();
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
    let history = client.room_history(None, "W1N1", 47).await.unwrap();
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
    let client = Client::builder(server.url()).build().unwrap();
    let history = client.room_history(None, "W1N1", 150).await.unwrap();
    assert_eq!((history.room(), history.base()), ("W1N1", 100));
    let states: Vec<_> = history.replay().collect();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0].1.object("a").unwrap()["type"], "road");
    let error = client.room_history(None, "W1N1", 250).await.unwrap_err();
    assert!(
        matches!(&error, Error::Api(ApiError::Status { status, .. }) if status.as_u16() == 404),
        "{error:?}"
    );
}

#[tokio::test]
async fn memory_and_console_flows_send_the_documented_bodies() {
    let server = Server::start(
        |request: &Request| match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/api/version") => (200, VERSION.to_owned()),
            ("GET", "/api/auth/me") => (200, ME.to_owned()),
            ("GET", "/api/user/memory") => {
                (200, r#"{"ok":1,"data":"Incorrect memory path"}"#.to_owned())
            }
            ("POST", "/api/user/memory" | "/api/user/console") => (200, r#"{"ok":1}"#.to_owned()),
            _ => (404, "{}".to_owned()),
        },
        |_| true,
    )
    .await;
    let client = Client::builder(server.url())
        .token(Token::new("t"))
        .build()
        .unwrap();
    assert_eq!(
        client.memory(None, Some("a.b")).await.unwrap(),
        MemoryValue::IncorrectPath
    );
    client.set_memory(None, "a.b", 5.into()).await.unwrap();
    client.delete_memory(None, "a.b").await.unwrap();
    client.console(None, "Game.time").await.unwrap();

    let requests: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.path.starts_with("/api/user/"))
        .collect();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].query, "path=a.b");
    let set: serde_json::Value = serde_json::from_str(&requests[1].body).unwrap();
    assert_eq!(set, serde_json::json!({"path": "a.b", "value": 5}));
    let delete: serde_json::Value = serde_json::from_str(&requests[2].body).unwrap();
    assert_eq!(delete, serde_json::json!({"path": "a.b"}));
    let console: serde_json::Value = serde_json::from_str(&requests[3].body).unwrap();
    assert_eq!(console, serde_json::json!({"expression": "Game.time"}));
}
