//! The account's usage, which the session keeps for every consumer, the published profile and
//! the CPU allocation, against a stand-in for screeps.com with two shards.

use std::sync::{Arc, Mutex, PoisonError};

use overseer::account::usage::{RunCpu, Usage};
use overseer::core::ServerUrl;
use overseer::model::account::cpu::ShardCpu;
use overseer::{Error, Overseer};
use tokio::sync::watch;

use crate::server::{self, Request, Server, Ws};
use crate::{connect_as, receive, routes, soon};

/// `GET /api/version` of a sharded server (default shard `shard0`).
const SHARDED_VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":100,"shards":["shard0","shard3"],"renderer":{"resources":{},"metadata":{}}}}"#;

/// `GET /api/game/shards/info`: shard3 caps the CPU at 20.
const SHARDS_INFO: &str = r#"{"ok":1,"shards":[{"name":"shard0","rooms":10,"tick":4100,"users":5},{"name":"shard3","cpuLimit":20,"rooms":10,"tick":3150,"users":5}]}"#;

/// The account routes of the stand-in over [`routes`]: `/api/auth/me` reports 100 CPU and the
/// allocation in `allocation` (a JSON object), which `POST /api/user/cpu-shards` replaces.
fn account_routes(request: &Request, allocation: &Mutex<String>) -> (u16, String) {
    let mut allocation = allocation.lock().unwrap_or_else(PoisonError::into_inner);
    match (request.method.as_str(), request.path.as_str()) {
        (_, "/api/version") => (200, SHARDED_VERSION.to_owned()),
        ("GET", "/api/auth/me") if request.token.is_some() => (
            200,
            format!(
                r#"{{"ok":1,"_id":"u1","username":"Alice","cpu":100,"money":0,"powerExperimentations":0,"cpuShard":{allocation}}}"#
            ),
        ),
        ("GET", "/api/game/shards/info") => (200, SHARDS_INFO.to_owned()),
        ("POST", "/api/user/cpu-shards") => {
            let body: serde_json::Value = serde_json::from_str(&request.body).unwrap();
            *allocation = body["cpu"].to_string();
            (200, r#"{"ok":1}"#.to_owned())
        }
        _ => routes(request),
    }
}

/// A stand-in for screeps.com whose account gives `allocation` (a JSON object) to the shards,
/// and a session signed in to it.
async fn start(allocation: &str) -> (Server, Overseer, Ws) {
    let allocation = Arc::new(Mutex::new(allocation.to_owned()));
    let mut server = Server::start(
        move |request| account_routes(request, &allocation),
        |_| true,
    )
    .await;
    let url = ServerUrl::proxied(server.url().base().as_str(), &ServerUrl::official()).unwrap();
    let (overseer, websocket) = connect_as(&mut server, url).await;
    (server, overseer, websocket)
}

/// The requests to `path`.
fn sent(server: &Server, path: &str) -> usize {
    server
        .requests()
        .iter()
        .filter(|request| request.path == path)
        .count()
}

/// Waits until `usage` holds a value that `ready` accepts, and returns it.
async fn until(usage: &mut watch::Receiver<Usage>, ready: impl Fn(&Usage) -> bool) -> Usage {
    soon(usage.wait_for(|usage| ready(usage)))
        .await
        .unwrap()
        .clone()
}

#[tokio::test]
async fn the_usage_is_one_per_session_and_follows_runs_and_the_profile() {
    let (server, overseer, mut websocket) = start(r#"{"shard0":30,"shard3":70}"#).await;
    let account = overseer.account();

    // Two consumers share one subscription and one history.
    let mut first = account.usage().unwrap();
    assert_eq!(receive(&mut websocket, 1).await, ["subscribe user:u1/cpu"]);
    let mut second = overseer.account().usage().unwrap();
    // The account's CPU and the CPU of each shard, with the caps of the shard information.
    let known = until(&mut first, |usage| usage.shards_known).await;
    assert_eq!(known.cpu, Some(100.0));
    assert_eq!(
        known.shards,
        [
            ShardCpu {
                name: "shard0".to_owned(),
                assigned: 30.0,
                cap: None,
                average_tick: Some(4100.0),
            },
            ShardCpu {
                name: "shard3".to_owned(),
                assigned: 70.0,
                cap: Some(20.0),
                average_tick: Some(3150.0),
            },
        ]
    );
    // The limit of a run: the usable CPU of the shard that the user looks at.
    assert_eq!(known.cpu_limit(Some("shard3")), Some(20.0));
    assert_eq!(known.cpu_limit(Some("shard0")), Some(30.0));
    assert_eq!(known.cpu_limit(None), Some(100.0));
    assert_eq!(sent(&server, "/api/game/shards/info"), 1);

    // The greeting of the new subscription (CPU 0) is no run; later runs are, a CPU error is
    // a limit hit. The channel holds the latest event: each run is awaited before the next.
    server::send(&mut websocket, r#"["user:u1/cpu",{"cpu":0,"memory":10}]"#).await;
    server::send(
        &mut websocket,
        r#"["user:u1/cpu",{"cpu":12,"memory":2048}]"#,
    )
    .await;
    until(&mut second, |usage| usage.runs.len() == 1).await;
    server::send(&mut websocket, r#"["user:u1/cpu",{"cpu":"error"}]"#).await;
    let usage = until(&mut second, |usage| usage.runs.len() == 2).await;
    let runs: Vec<_> = usage.runs.iter().map(|run| (run.cpu, run.memory)).collect();
    assert_eq!(
        runs,
        [(RunCpu::Used(12.0), Some(2048)), (RunCpu::LimitHit, None)]
    );
    assert_eq!(
        until(&mut first, |usage| usage.runs.len() == 2).await,
        usage
    );

    // A change of the allocation reaches the usage through the published profile.
    let allocation = [("shard0".to_owned(), 80), ("shard3".to_owned(), 20)]
        .into_iter()
        .collect();
    account.set_cpu_shards(allocation).await.unwrap();
    let changed = until(&mut first, |usage| {
        usage
            .shards
            .first()
            .is_some_and(|shard| (shard.assigned - 80.0).abs() < 1e-9)
    })
    .await;
    assert_eq!(changed.cpu_limit(Some("shard3")), Some(20.0));
    assert_eq!(changed.cpu_limit(Some("shard0")), Some(80.0));
    // The history survives.
    assert_eq!(changed.runs.len(), 2);

    // The last receiver gone, the session unsubscribes and forgets the runs.
    drop(first);
    drop(second);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe user:u1/cpu"]
    );
    let mut fresh = account.usage().unwrap();
    assert_eq!(receive(&mut websocket, 1).await, ["subscribe user:u1/cpu"]);
    assert!(fresh.borrow_and_update().runs.is_empty());
    // The shard information stays cached for the session.
    until(&mut fresh, |usage| usage.shards_known).await;
    assert_eq!(sent(&server, "/api/game/shards/info"), 1);
}

#[tokio::test]
async fn an_allocation_is_checked_before_it_is_sent_and_a_cap_does_not_block_it() {
    let (server, overseer, _websocket) = start(r#"{"shard0":100}"#).await;
    let account = overseer.account();
    let mut profile = account.profile_watch();
    profile.borrow_and_update();

    // Not the account's 100 CPU: refused without a request.
    let short = [("shard0".to_owned(), 50), ("shard3".to_owned(), 20)]
        .into_iter()
        .collect();
    let error = account.set_cpu_shards(short).await.unwrap_err();
    assert!(
        matches!(&error, Error::InvalidInput(text) if text.contains("70 CPU")),
        "{error:?}"
    );
    assert_eq!(sent(&server, "/api/user/cpu-shards"), 0);

    // More than shard3's cap of 20 is valid: the server accepts it. The session reads the
    // profile again in the background and publishes the new allocation.
    let capped = [("shard3".to_owned(), 100)].into_iter().collect();
    account.set_cpu_shards(capped).await.unwrap();
    assert_eq!(sent(&server, "/api/user/cpu-shards"), 1);
    profile.changed().await.unwrap();
    let published = profile.borrow_and_update().clone().unwrap();
    assert_eq!(published.cpu_shards.get("shard3"), Some(&100.0));
}
