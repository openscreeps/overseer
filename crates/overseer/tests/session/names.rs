//! The names of the user's game for console completion: shard filtering, the rooms read,
//! coverage, one load for concurrent calls, and freshness.

use std::sync::Arc;

use futures_util::StreamExt as _;
use overseer::console::{COMPLETION_FRESHNESS, NAMES_MAX_ROOMS};
use overseer::core::ServerUrl;

use crate::server::{Request, Server, Ws};
use crate::{connect_as, routes, soon};

/// `GET /api/version` of a sharded server (default shard `shard0`).
const SHARDED_VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":100,"shards":["shard0","shard3"],"renderer":{"resources":{},"metadata":{}}}}"#;

/// The own rooms on `shard0`: `W1N1` to `W18N1`, more than one load reads.
fn shard0_rooms() -> Vec<String> {
    (1..=18).map(|x| format!("W{x}N1")).collect()
}

/// A creep named `name` of user `user` in `room`.
fn creep(room: &str, name: &str, user: &str) -> String {
    format!(
        r#"{{"_id":"{name}","type":"creep","room":"{room}","x":1,"y":1,"name":"{name}","user":"{user}","hits":100,"hitsMax":100,"store":{{}},"body":[{{"type":"move","hits":100}}]}}"#
    )
}

/// A sharded official stand-in: `u1` owns [`shard0_rooms`] and `W5N5` on `shard3`. Each room
/// `W<x>N1` holds the creep `c<x>` of `u1` and a creep of `u2`; `W1N1` also `Spawn1`;
/// `W2N1` cannot be read; `W5N5` holds the creep `far`.
fn names_routes(request: &Request) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        (_, "/api/version") => (200, SHARDED_VERSION.to_owned()),
        ("GET", "/api/user/rooms") => {
            let rooms: Vec<String> = shard0_rooms()
                .iter()
                .map(|room| format!(r#""{room}""#))
                .collect();
            (
                200,
                format!(
                    r#"{{"ok":1,"shards":{{"shard0":[{}],"shard3":["W5N5"]}}}}"#,
                    rooms.join(",")
                ),
            )
        }
        ("GET", "/api/game/room-objects") => {
            let room = query(request, "room");
            let mut objects = match room {
                "W2N1" => return (500, r#"{"error":"busy"}"#.to_owned()),
                "W5N5" => vec![creep(room, "far", "u1")],
                _ => {
                    let x = room.trim_start_matches('W').trim_end_matches("N1");
                    vec![
                        creep(room, &format!("c{x}"), "u1"),
                        creep(room, &format!("foe{x}"), "u2"),
                    ]
                }
            };
            if room == "W1N1" {
                objects.push(
                    r#"{"_id":"s1","type":"spawn","room":"W1N1","x":25,"y":25,"name":"Spawn1","user":"u1","hits":5000,"hitsMax":5000,"notifyWhenAttacked":true,"store":{},"storeCapacityResource":{}}"#
                        .to_owned(),
                );
            }
            (
                200,
                format!(
                    r#"{{"ok":1,"objects":[{}],"users":{{}}}}"#,
                    objects.join(",")
                ),
            )
        }
        _ => routes(request),
    }
}

/// The value of `name` in the query of `request`; empty when missing.
fn query<'a>(request: &'a Request, name: &str) -> &'a str {
    request
        .query
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .unwrap_or_default()
}

/// The names requests so far: the room list and each room read, with its shard.
fn sent(server: &Server) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter_map(|request| match request.path.as_str() {
            "/api/user/rooms" => Some("rooms".to_owned()),
            "/api/game/room-objects" => Some(format!(
                "{}/{}",
                query(&request, "shard"),
                query(&request, "room")
            )),
            _ => None,
        })
        .collect()
}

/// Reads the client's frames until the socket closes, so that keep-alive pings get answers
/// while the clock jumps.
fn drain(mut websocket: Ws) {
    tokio::spawn(async move { while websocket.next().await.is_some() {} });
}

async fn connect_sharded(server: &mut Server) -> overseer::Overseer {
    // A stand-in for screeps.com: the room list comes by shard.
    let url = ServerUrl::proxied(server.url().base().as_str(), &ServerUrl::official()).unwrap();
    let (overseer, websocket) = connect_as(server, url).await;
    drain(websocket);
    overseer
}

#[tokio::test]
async fn names_come_from_the_first_rooms_of_the_shard_with_their_coverage() {
    let mut server = Server::start(names_routes, |_| true).await;
    let overseer = connect_sharded(&mut server).await;
    let console = overseer.console();
    assert_eq!(console.cached_names(None), None);

    // Two consoles at once share one load of the default shard.
    let (first, second) =
        soon(async { tokio::join!(console.names(None), console.names(None)) }).await;
    let names = first.unwrap().unwrap();
    assert!(Arc::ptr_eq(&names, &second.unwrap().unwrap()));
    let mut rooms = shard0_rooms();
    rooms.sort();
    assert_eq!(names.rooms, rooms);
    // The first 16 rooms in the order of the room list, less the unreadable `W2N1`; never
    // other players' creeps.
    let mut creeps: Vec<String> = (1..=NAMES_MAX_ROOMS)
        .filter(|x| *x != 2)
        .map(|x| format!("c{x}"))
        .collect();
    creeps.sort();
    assert_eq!(names.creeps, creeps);
    assert_eq!(names.spawns, ["Spawn1"]);
    assert_eq!(names.rooms_read, NAMES_MAX_ROOMS - 1);
    assert!(!names.is_complete());
    let mut expected = vec!["rooms".to_owned()];
    expected.extend((1..=NAMES_MAX_ROOMS).map(|x| format!("shard0/W{x}N1")));
    let mut loaded = sent(&server);
    loaded[1..].sort();
    expected[1..].sort();
    assert_eq!(loaded, expected);

    // The default shard by name is the same entry: no request.
    let again = console.names(Some("shard0")).await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&names, &again));
    assert!(Arc::ptr_eq(&names, &console.cached_names(None).unwrap()));
    assert_eq!(sent(&server).len(), expected.len());

    // Another shard has its own rooms and names.
    let far = console.names(Some("shard3")).await.unwrap().unwrap();
    assert_eq!(
        (far.rooms.as_slice(), far.creeps.as_slice(), far.rooms_read),
        (
            ["W5N5".to_owned()].as_slice(),
            ["far".to_owned()].as_slice(),
            1
        )
    );
    assert!(far.is_complete());
    assert_eq!(&sent(&server)[expected.len()..], ["rooms", "shard3/W5N5"]);
}

#[tokio::test]
async fn names_load_again_once_old_and_sooner_when_a_room_failed() {
    let mut server = Server::start(names_routes, |_| true).await;
    let overseer = connect_sharded(&mut server).await;
    let console = overseer.console();
    tokio::time::pause();
    let partial = console.names(Some("shard0")).await.unwrap().unwrap();
    let complete = console.names(Some("shard3")).await.unwrap().unwrap();
    let count = sent(&server).len();

    // A room could not be read: the shard loads again after the short freshness only.
    tokio::time::advance(COMPLETION_FRESHNESS.fresh_empty).await;
    let reloaded = console.names(Some("shard0")).await.unwrap().unwrap();
    assert!(!Arc::ptr_eq(&partial, &reloaded));
    assert_eq!(*partial, *reloaded);
    let same = console.names(Some("shard3")).await.unwrap().unwrap();
    assert!(Arc::ptr_eq(&complete, &same));
    assert_eq!(sent(&server).len(), count + 1 + NAMES_MAX_ROOMS);

    // Complete names stay in use for the full freshness.
    let rest = COMPLETION_FRESHNESS
        .fresh
        .checked_sub(COMPLETION_FRESHNESS.fresh_empty)
        .unwrap();
    tokio::time::advance(rest).await;
    let renewed = console.names(Some("shard3")).await.unwrap().unwrap();
    assert!(!Arc::ptr_eq(&complete, &renewed));
    assert_eq!(
        &sent(&server)[count + 1 + NAMES_MAX_ROOMS..],
        ["rooms", "shard3/W5N5"]
    );
}
