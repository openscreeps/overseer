//! The world operations where the contracts differ: the shard information (native, or
//! screepsmod-admin-utils' on community servers), the tick duration (per shard on
//! screeps.com), room objects (native, or screepsmod-admin-utils' on community servers), the
//! first spawn (screepsmod-admin-utils requires a name), room history and the operations stock
//! community servers lack, and the ranking of map statistics.

use overseer_core::Token;
use overseer_core::model::platform::Subsystem;
use overseer_core::model::world::{GameRoomObject, ShardInfo, Tile};
use overseer_net::api::{RoomObjectsSource as _, WorldApi as _};
use overseer_net::{Client, Error};
use serde_json::{Value, json};

use crate::server::{Request, Server};
use crate::{ME, VERSION};

/// `GET /api/version` of screeps.com.
const OFFICIAL_VERSION: &str = r#"{"ok":1,"protocol":14,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":100,"renderer":{"resources":{},"metadata":{}},"features":[{"name":"auth","version":1}],"shards":["shard0","shard1"]}}"#;

/// `GET /api/version` of the stock Screeps server: no features.
const STOCK_VERSION: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,"serverData":{"customObjectTypes":{},"historyChunkSize":20,"socketUpdateThrottle":100,"renderer":{"resources":{},"metadata":{}}},"packageVersion":"4.2.21"}"#;

/// `GET /api/version` of the Screeps server with screepsmod-admin-utils.
const ADMIN_UTILS_VERSION: &str = r#"{"ok":1,"protocol":14,"useNativeAuth":false,"users":1,"serverData":{"shards":[null],"customObjectTypes":{},"historyChunkSize":20,"socketUpdateThrottle":1000,"renderer":{"resources":{},"metadata":{}},"features":[{"name":"screepsmod-admin-utils","version":"1.37.1"}]}}"#;

/// screepsmod-admin-utils' room objects: no `ok`, storage bookkeeping fields included.
const ADMIN_UTILS_OBJECTS: &str = r#"{"objects":[
    {"_id":"c1","type":"controller","room":"W1N1","x":10,"y":12,"level":2,"user":"u1","progress":5,"meta":{"revision":1},"$loki":3},
    {"_id":"s1","type":"source","room":"W1N1","x":20,"y":22,"energy":3000,"energyCapacity":3000,"ticksToRegeneration":300}],
    "users":{"u1":{"_id":"u1","username":"Alice"},"2":{"_id":"2","username":"Invader"}}}"#;

/// A server answering `GET /api/version` with `version`, `GET /api/auth/me` for token `t`,
/// and the other paths with `world`; HTTP 404 for what `world` does not answer.
fn routes(
    version: &'static str,
    world: impl Fn(&Request) -> Option<(u16, String)> + Send + Sync + 'static,
) -> impl Fn(&Request) -> (u16, String) + Send + Sync + 'static {
    move |request| match request.path.as_str() {
        "/api/version" => (200, version.to_owned()),
        "/api/auth/me" if request.token.as_deref() == Some("t") => (200, ME.to_owned()),
        _ => world(request).unwrap_or_else(|| (404, "Not Found".to_owned())),
    }
}

/// A client of `server` holding token `t`.
fn client(server: &Server, official: bool) -> Client {
    let url = if official {
        server.official_url()
    } else {
        server.url()
    };
    Client::builder(url).token(Token::new("t")).build().unwrap()
}

/// The JSON body of the last request to `path`.
fn body(server: &Server, path: &str) -> Value {
    let request = server
        .requests()
        .into_iter()
        .rfind(|request| request.path == path)
        .unwrap_or_else(|| panic!("no request to {path}"));
    serde_json::from_str(&request.body).unwrap()
}

#[tokio::test]
async fn community_room_objects_and_named_spawns_come_from_screepsmod_admin_utils() {
    let server = Server::start(
        routes(ADMIN_UTILS_VERSION, |request| match request.path.as_str() {
            "/api/game/room-objects" => Some((200, ADMIN_UTILS_OBJECTS.to_owned())),
            "/api/game/gen-unique-object-name" => {
                Some((200, r#"{"ok":1,"name":"Spawn2"}"#.to_owned()))
            }
            "/api/game/place-spawn" => Some((200, r#"{"ok":1,"newbie":true}"#.to_owned())),
            _ => None,
        }),
        |_| true,
    )
    .await;
    let api = client(&server, false).api().await.unwrap();
    let objects = api
        .room_objects_source()
        .expect("screepsmod-admin-utils provides room objects")
        .room_objects(Some("shard0"), "W1N1")
        .await
        .unwrap();
    assert!(matches!(
        objects.objects.as_slice(),
        [GameRoomObject::Controller(_), GameRoomObject::Source(_)]
    ));
    assert_eq!(objects.users["u1"].username.as_deref(), Some("Alice"));
    let request = server
        .requests()
        .into_iter()
        .find(|request| request.path == "/api/game/room-objects")
        .unwrap();
    // A community server serves one world: no shard is named.
    assert_eq!(request.query, "room=W1N1");

    // The plugin requires a name: the server's next free one when none is given.
    api.place_spawn(None, "W1N1", (10, 12), None).await.unwrap();
    assert_eq!(
        body(&server, "/api/game/gen-unique-object-name"),
        json!({"type": "spawn"})
    );
    assert_eq!(
        body(&server, "/api/game/place-spawn"),
        json!({"name": "Spawn2", "room": "W1N1", "x": 10, "y": 12})
    );
    api.place_spawn(None, "W1N1", (11, 13), Some("Home"))
        .await
        .unwrap();
    assert_eq!(
        body(&server, "/api/game/place-spawn"),
        json!({"name": "Home", "room": "W1N1", "x": 11, "y": 13})
    );
}

#[tokio::test]
async fn stock_community_servers_lack_room_objects_decorations_and_history() {
    let server = Server::start(
        routes(STOCK_VERSION, |request| match request.path.as_str() {
            // backend-local's default history provider: answers the discovery probe, so a
            // chunk is never asked for.
            "/room-history" => Some((500, "not implemented".to_owned())),
            "/api/game/place-spawn" => Some((200, r#"{"ok":1,"newbie":true}"#.to_owned())),
            _ => None,
        }),
        |_| true,
    )
    .await;
    let api = client(&server, false).api().await.unwrap();
    let requested = server.requests().len();
    assert!(api.room_objects_source().is_none());
    let error = api.room_decorations(None, "W1N1").await.unwrap_err();
    assert!(
        matches!(error, Error::Unsupported(Subsystem::RoomDecorations)),
        "{error:?}"
    );
    let error = api.find_object("o1").await.unwrap_err();
    assert!(
        matches!(error, Error::Unsupported(Subsystem::ObjectSearch)),
        "{error:?}"
    );
    let error = api.random_novice_room(None).await.unwrap_err();
    assert!(
        matches!(error, Error::Unsupported(Subsystem::NoviceRooms)),
        "{error:?}"
    );
    let error = api.room_history(None, "W1N1", 47).await.unwrap_err();
    assert!(
        matches!(error, Error::Unsupported(Subsystem::RoomHistory)),
        "{error:?}"
    );
    assert_eq!(server.requests().len(), requested, "nothing was sent");

    // Without the plugin a spawn may stay unnamed.
    api.place_spawn(None, "W1N1", (10, 12), None).await.unwrap();
    assert_eq!(
        body(&server, "/api/game/place-spawn"),
        json!({"room": "W1N1", "x": 10, "y": 12})
    );
}

/// The shard of `name` with `cpu_limit` and `tick`.
fn shard(name: &str, cpu_limit: Option<f64>, tick: Option<f64>) -> ShardInfo {
    ShardInfo {
        name: name.to_owned(),
        cpu_limit,
        tick,
    }
}

/// The only request to `path`.
fn only_request(server: &Server, path: &str) -> Request {
    let mut sent = server
        .requests()
        .into_iter()
        .filter(|request| request.path == path);
    let request = sent.next().unwrap_or_else(|| panic!("a request to {path}"));
    assert!(sent.next().is_none(), "one request to {path}");
    request
}

#[tokio::test]
async fn official_shards_info_reads_capped_and_uncapped_shards() {
    let server = Server::start(
        routes(OFFICIAL_VERSION, |request| match request.path.as_str() {
            "/api/game/shards/info" => Some((
                200,
                r#"{"ok":1,"shards":[
                    {"name":"shard0","lastTicks":[4100,3900],"cpuLimit":0,"rooms":30000,"users":1200,"tick":4000},
                    {"name":"shard3","lastTicks":[3100,3200],"cpuLimit":20,"rooms":4000,"users":900,"tick":3150.5}]}"#
                    .to_owned(),
            )),
            _ => None,
        }),
        |_| true,
    )
    .await;
    let api = client(&server, true).api().await.unwrap();
    assert!(api.supports(Subsystem::ShardsInfo));
    let shards = api.shards_info().await.unwrap();
    assert_eq!(
        shards,
        [
            shard("shard0", None, Some(4000.0)),
            shard("shard3", Some(20.0), Some(3150.5)),
        ]
    );
    let request = only_request(&server, "/api/game/shards/info");
    assert_eq!(request.method, "GET");
    assert_eq!(request.query, "");
}

#[tokio::test]
async fn community_shards_info_comes_from_screepsmod_admin_utils() {
    let server = Server::start(
        routes(ADMIN_UTILS_VERSION, |request| match request.path.as_str() {
            "/api/game/shards/info" => Some((
                200,
                r#"{"ok":1,"shards":[
                    {"name":"shardS","cpuLimit":0,"lastTicks":[1000,1200],"rooms":169,"tick":1100},
                    {"name":null,"cpuLimit":0,"lastTicks":[],"rooms":169,"tick":null}]}"#
                    .to_owned(),
            )),
            _ => None,
        }),
        |_| true,
    )
    .await;
    let api = client(&server, false).api().await.unwrap();
    assert!(api.supports(Subsystem::ShardsInfo));
    // The plugin reports no cap (0); an entry without a name is skipped.
    let shards = api.shards_info().await.unwrap();
    assert_eq!(shards, [shard("shardS", None, Some(1100.0))]);
    assert_eq!(only_request(&server, "/api/game/shards/info").method, "GET");
}

#[tokio::test]
async fn other_servers_refuse_the_shards_info_without_a_request() {
    for version in [VERSION, STOCK_VERSION] {
        let server = Server::start(
            routes(version, |request| match request.path.as_str() {
                "/room-history" => Some((500, "not implemented".to_owned())),
                _ => None,
            }),
            |_| true,
        )
        .await;
        let api = client(&server, false).api().await.unwrap();
        assert!(!api.supports(Subsystem::ShardsInfo));
        let before = server.requests().len();
        let error = api.shards_info().await.unwrap_err();
        assert!(
            matches!(error, Error::Unsupported(Subsystem::ShardsInfo)),
            "{error:?}"
        );
        assert_eq!(server.requests().len(), before, "nothing was sent");
    }
}

#[tokio::test]
async fn official_tick_duration_is_the_shards() {
    let server = Server::start(
        routes(OFFICIAL_VERSION, |request| match request.path.as_str() {
            "/api/game/shards/tick" => Some((
                200,
                match request.query.as_str() {
                    "shard=shard1" => r#"{"ok":1,"tick":3150.5}"#,
                    _ => r#"{"ok":1,"tick":null}"#,
                }
                .to_owned(),
            )),
            _ => None,
        }),
        |_| true,
    )
    .await;
    let api = client(&server, true).api().await.unwrap();
    assert_eq!(
        api.tick_duration(Some("shard1")).await.unwrap(),
        Some(3150.5)
    );
    let request = only_request(&server, "/api/game/shards/tick");
    assert_eq!(request.method, "GET");
    assert_eq!(request.query, "shard=shard1");
    // A shard whose duration the server does not know yet.
    assert_eq!(api.tick_duration(Some("shard0")).await.unwrap(), None);
}

#[tokio::test]
async fn openscreeps_and_community_tick_durations_are_the_servers() {
    for (version, reply, expected) in [
        (VERSION, r#"{"ok":1,"tick":1000}"#, Some(1000.0)),
        (VERSION, r#"{"ok":1,"tick":0}"#, None),
        (STOCK_VERSION, r#"{"ok":1,"tick":245.5}"#, Some(245.5)),
        // Fewer than two ticks completed since the server started.
        (STOCK_VERSION, r#"{"ok":1,"tick":null}"#, None),
    ] {
        let server = Server::start(
            routes(version, move |request| match request.path.as_str() {
                "/api/game/tick" => Some((200, reply.to_owned())),
                "/room-history" => Some((500, "not implemented".to_owned())),
                _ => None,
            }),
            |_| true,
        )
        .await;
        let api = client(&server, false).api().await.unwrap();
        // A single world: the shard is not sent.
        assert_eq!(
            api.tick_duration(Some("shard0")).await.unwrap(),
            expected,
            "{reply}"
        );
        let request = only_request(&server, "/api/game/tick");
        assert_eq!(request.method, "GET");
        assert_eq!(request.query, "");
    }
}

/// A map-statistics reply for `W1N1` whose `energyHarvested8` ranking is `ranking`.
fn map_stats(ranking: &Value) -> String {
    json!({
        "ok": 1,
        "gameTime": 5,
        "stats": {"W1N1": {
            "status": "normal",
            "own": {"user": "u1", "level": 3},
            "minerals0": {"type": "H", "density": 2},
            "energyHarvested8": ranking,
        }},
        "statsMax": {"energyHarvested8": 40},
        "users": {"u1": {"_id": "u1", "username": "Alice"}, "u2": {"_id": "u2", "username": "Bob"}},
    })
    .to_string()
}

#[tokio::test]
async fn map_stats_rankings_read_alike_from_every_contract() {
    // `OpenScreeps` ranks every user; screeps.com sends one entry.
    let openscreeps = Server::start(
        routes(VERSION, |request| {
            (request.path == "/api/game/map-stats").then(|| {
                let ranking = json!([{"user": "u1", "value": 40}, {"user": "u2", "value": 7}]);
                (200, map_stats(&ranking))
            })
        }),
        |_| true,
    )
    .await;
    let official = Server::start(
        routes(OFFICIAL_VERSION, |request| {
            (request.path == "/api/game/map-stats")
                .then(|| (200, map_stats(&json!({"user": "u1", "value": 40}))))
        }),
        |_| true,
    )
    .await;
    let rooms = || vec!["W1N1".to_owned()];
    let api = client(&openscreeps, false).api().await.unwrap();
    let stats = api
        .map_stats(Some("shard0"), rooms(), "energyHarvested8")
        .await
        .unwrap();
    let room = &stats.stats["W1N1"];
    let ranked: Vec<_> = room.ranking["energyHarvested8"]
        .iter()
        .map(|entry| (entry.user.as_str(), entry.value))
        .collect();
    assert_eq!(ranked, [("u1", 40.0), ("u2", 7.0)]);
    assert_eq!(room.own.as_ref().map(|own| own.level), Some(3));
    assert_eq!(
        room.mineral
            .as_ref()
            .and_then(|mineral| mineral.kind.as_deref()),
        Some("H")
    );
    assert_eq!(
        body(&openscreeps, "/api/game/map-stats"),
        json!({"rooms": ["W1N1"], "statName": "energyHarvested8"})
    );

    let api = client(&official, true).api().await.unwrap();
    let stats = api
        .map_stats(None, rooms(), "energyHarvested8")
        .await
        .unwrap();
    let ranking = &stats.stats["W1N1"].ranking["energyHarvested8"];
    assert_eq!(ranking.len(), 1);
    assert_eq!(ranking[0].user, "u1");
    assert_eq!(stats.stats_max["energyHarvested8"], 40.0);
    // screeps.com names the default shard.
    assert_eq!(
        body(&official, "/api/game/map-stats"),
        json!({"rooms": ["W1N1"], "shard": "shard0", "statName": "energyHarvested8"})
    );
}

/// Community servers store the terrain of rooms that the stock CLI creates without `type`
/// (screeps.newbieland.net, for example): such documents decode.
#[tokio::test]
async fn community_terrain_documents_without_a_type_decode() {
    let server = Server::start(
        |request: &Request| match request.path.as_str() {
            "/api/version" => (200, STOCK_VERSION.to_owned()),
            "/api/game/rooms" => (
                200,
                format!(
                    r#"{{"ok":1,"rooms":[{{"_id":"t1","room":"W1N1","terrain":"1{}"}}]}}"#,
                    "0".repeat(2499)
                ),
            ),
            _ => (404, "{}".to_owned()),
        },
        |_| true,
    )
    .await;
    let client = Client::builder(server.url()).build().unwrap();
    let terrains = client.terrains(None, ["W1N1"]).await.unwrap();
    assert_eq!(terrains.len(), 1);
    assert_eq!(terrains["W1N1"].get(0, 0), Some(Tile::Wall));
    assert_eq!(terrains["W1N1"].get(1, 0), Some(Tile::Plain));
}
