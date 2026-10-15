//! Replies shaped like real official and standalone server replies (invented values, no
//! account data) decode through [`Session::response`] into the generated types.

use ::http::Response;

use super::endpoints::{
    AuthMe, GameRoomObjects, GameRoomTerrain, GameTick, MarketMyOrders, UserMemoryGet,
    UserOverview, UserRooms, UserWorldStartRoom, UserWorldStatus,
};
use super::types::{
    AuthSteamAccountOwnership, GameRoomObject, GameRoomTerrainResponse, GameTerrainTileType,
    UserBadgeType, UserWorldStatus as WorldStatus,
};
use super::{Endpoint, MemoryValue, Session, Tile};
use crate::{ServerUrl, Token};

/// Decodes `body` as a 200 reply to `E`.
fn decode<E: Endpoint>(body: &str) -> E::Response {
    let mut session = Session::new(ServerUrl::official(), Some(Token::new("test-token")));
    let reply = Response::builder().status(200).body(body).unwrap();
    session.response::<E, _>(reply).unwrap().body
}

#[test]
fn auth_me_reads_the_underscore_id_and_both_steam_shapes() {
    let official = decode::<AuthMe>(
        r##"{"ok":1,"_id":"5a1b2c3d4e5f60718293a4b5","email":"player@example.com",
            "username":"Player","cpu":20,"badge":{"type":21,"color1":"#260d0d","color2":"#6b2e41",
            "color3":"#ffe56d","param":-100,"flip":false},"password":true,"lastRespawnDate":1700000000000,
            "gcl":1234567,"money":2500.5,"subscriptionTokens":0,"powerExperimentations":3,
            "steam":{"id":"76561190000000000","displayName":"Player","ownership":[464350],
            "steamProfileLinkHidden":0}}"##,
    );
    assert_eq!(official.id, "5a1b2c3d4e5f60718293a4b5");
    assert_eq!(official.username.as_deref(), Some("Player"));
    assert_eq!(official.cpu, Some(20.0));
    assert!(matches!(
        official.badge.map(|badge| badge.r#type),
        Some(UserBadgeType::Integer(21))
    ));
    let steam = official.steam.unwrap();
    assert_eq!(
        steam.ownership,
        Some(AuthSteamAccountOwnership::List(vec![464_350]))
    );

    let standalone = decode::<AuthMe>(
        r#"{"ok":1,"_id":"u1","username":"Player","cpu":100,"gcl":0,"money":0,
            "powerExperimentations":0,"steam":{"id":"76561190000000000","ownership":true,
            "steamProfileLinkHidden":false}}"#,
    );
    assert_eq!(
        standalone.steam.unwrap().ownership,
        Some(AuthSteamAccountOwnership::Bool(true))
    );
}

#[test]
fn room_objects_decode_into_the_member_of_their_type() {
    let reply = decode::<GameRoomObjects>(
        r#"{"ok":1,"objects":[
            {"_id":"c1","type":"creep","room":"W1N1","x":10,"y":20,"name":"Harvester1",
             "user":"u1","body":[{"type":"work","hits":100},{"type":"move","hits":100,"boost":"UO"}],
             "hits":200,"hitsMax":200,"store":{"energy":50,"H":null},"storeCapacity":50,
             "fatigue":0,"ageTime":1500,"spawning":false,
             "actionLog":{"harvest":{"x":11,"y":20},"say":null}},
            {"_id":"s1","type":"spawn","room":"W1N1","x":25,"y":25,"name":"Spawn1","user":"u1",
             "hits":5000,"hitsMax":5000,"notifyWhenAttacked":true,"store":{"energy":300},
             "storeCapacityResource":{"energy":300},
             "spawning":{"name":"Harvester2","needTime":9,"spawnTime":1209},
             "effects":{"0":{"effect":1001,"endTime":1500},"1":{"power":1,"level":2,"endTime":900}}},
            {"_id":"k1","type":"controller","room":"W1N1","x":30,"y":9,"user":"u1","level":4,
             "progress":5000,"downgradeTime":50000,"safeModeAvailable":2,
             "reservation":null,"sign":{"user":"u1","text":"hi","time":70,"datetime":1700000000000}},
            {"_id":"w1","type":"constructedWall","room":"W1N1","x":0,"y":5,"decayTime":{"timestamp":1700000000000}},
            {"_id":"d1","type":"energy","room":"W1N1","x":11,"y":21,"resourceType":"H","H":17},
            {"_id":"x1","type":"seasonObject","room":"W1N1","x":2,"y":3,"score":5}
        ],"users":{"u1":{"_id":"u1","username":"Player"}}}"#,
    );
    let [creep, spawn, controller, wall, resource, other] = reply.objects.as_slice() else {
        panic!("six objects");
    };
    let GameRoomObject::Creep(creep_fields) = creep else {
        panic!("not a creep: {creep:?}");
    };
    assert_eq!((creep.kind(), creep.x(), creep.y()), ("creep", 10, 20));
    assert_eq!(
        (creep.id(), creep.room(), creep.user()),
        ("c1", "W1N1", Some("u1"))
    );
    assert_eq!(creep_fields.body.len(), 2);
    assert_eq!(creep_fields.store["H"], None);
    let GameRoomObject::Spawn(spawn_fields) = spawn else {
        panic!("not a spawn: {spawn:?}");
    };
    let effects = spawn_fields.effects.as_ref().unwrap();
    assert_eq!(effects.len(), 2);
    assert_eq!(effects[0].effect, Some(1001));
    assert_eq!(effects[1].end_time, 900);
    let GameRoomObject::Controller(controller_fields) = controller else {
        panic!("not a controller: {controller:?}");
    };
    assert_eq!(controller_fields.level, 4);
    assert!(matches!(wall, GameRoomObject::ConstructedWall(_)));
    assert_eq!(wall.user(), None);
    let GameRoomObject::Resource(resource_fields) = resource else {
        panic!("not a dropped resource: {resource:?}");
    };
    assert_eq!((resource.kind(), resource_fields.amount()), ("energy", 17));
    let GameRoomObject::Other(other_fields) = other else {
        panic!("unknown types are not the fallback: {other:?}");
    };
    assert_eq!(other.kind(), "seasonObject");
    assert_eq!(other_fields.other["score"], 5);
    assert_eq!(reply.users["u1"].username.as_deref(), Some("Player"));
    // Serialization writes each member as it is, `type` included.
    let creep_json = serde_json::to_value(creep).unwrap();
    assert_eq!(creep_json["type"], "creep");
    assert_eq!(
        serde_json::from_value::<GameRoomObject>(creep_json).unwrap(),
        *creep
    );
}

#[test]
fn room_terrain_has_an_encoded_and_a_tile_form() {
    let mut encoded = "0".repeat(2500);
    encoded.replace_range(0..1, "1");
    encoded.replace_range(51..52, "2");
    let body = format!(
        r#"{{"ok":1,"terrain":[{{"_id":"t1","room":"W1N1","terrain":"{encoded}","type":"terrain"}}]}}"#
    );
    let GameRoomTerrainResponse::Encoded(reply) = decode::<GameRoomTerrain>(&body) else {
        panic!("encoded terrain");
    };
    let terrain = reply.terrain[0].decode().unwrap();
    assert_eq!(terrain.get(0, 0), Some(Tile::Wall));
    assert_eq!(terrain.get(1, 1), Some(Tile::Swamp));
    assert_eq!(terrain.get(2, 2), Some(Tile::Plain));

    let tiles = decode::<GameRoomTerrain>(
        r#"{"ok":1,"terrain":[{"room":"W1N1","x":0,"y":0,"type":"wall"},{"room":"W1N1","x":1,"y":1,"type":"swamp"}]}"#,
    );
    let GameRoomTerrainResponse::Tiles(tiles) = tiles else {
        panic!("terrain tiles");
    };
    assert_eq!(tiles.terrain[1].r#type, GameTerrainTileType::Swamp);
}

#[test]
fn sharded_and_flat_replies_both_decode() {
    let official = decode::<UserRooms>(
        r#"{"ok":1,"shards":{"shard0":["W1N1"],"shard3":[]},"reservations":{"shard0":[]}}"#,
    );
    assert_eq!(official.shards.unwrap()["shard0"], ["W1N1"]);
    let standalone = decode::<UserRooms>(r#"{"ok":1,"rooms":["W1N1","W2N2"]}"#);
    assert_eq!(standalone.rooms.unwrap(), ["W1N1", "W2N2"]);

    let official = decode::<UserOverview>(
        r#"{"ok":1,"statsMax":1200,"totals":{"energyHarvested":5000},
            "shards":{"shard0":{"rooms":["W1N1"],"gameTimes":[100,200],
            "stats":{"W1N1":[{"value":10,"endTime":100}]}}}}"#,
    );
    assert_eq!(
        official.shards.unwrap()["shard0"].rooms.as_deref(),
        Some(&["W1N1".to_owned()][..])
    );
    let standalone = decode::<UserOverview>(
        r#"{"ok":1,"rooms":["W1N1"],"stats":{"W1N1":[{"value":10,"endTime":100}]},"statsMax":10,
            "totals":{"energyHarvested":10},"gametimes":[100]}"#,
    );
    assert_eq!(standalone.stats.unwrap()["W1N1"][0].end_time, 100);

    let order = r#"{"_id":"o1","type":"sell","resourceType":"energy","roomName":"W1N1",
        "amount":1000,"remainingAmount":1000,"price":0.5,"active":true}"#;
    let official =
        decode::<MarketMyOrders>(&format!(r#"{{"ok":1,"shards":{{"shard0":[{order}]}}}}"#));
    assert_eq!(official.shards.unwrap()["shard0"][0].remaining_amount, 1000);
    let standalone = decode::<MarketMyOrders>(&format!(r#"{{"ok":1,"list":[{order}]}}"#));
    assert_eq!(standalone.list.unwrap()[0].price, 0.5);
}

#[test]
fn small_account_replies_decode() {
    assert_eq!(
        decode::<UserWorldStatus>(r#"{"ok":1,"status":"empty"}"#).status,
        WorldStatus::Empty
    );
    assert_eq!(
        decode::<UserWorldStartRoom>(r#"{"ok":1,"room":["shard3/W1N1"]}"#).room,
        ["shard3/W1N1"]
    );
    assert_eq!(
        decode::<GameTick>(r#"{"ok":1,"tick":1003.5}"#).tick,
        Some(1003.5)
    );
    let memory = decode::<UserMemoryGet>(r#"{"ok":1,"data":"Incorrect memory path"}"#);
    assert_eq!(memory.value().unwrap(), MemoryValue::IncorrectPath);
    assert_eq!(
        decode::<UserMemoryGet>(r#"{"ok":1}"#).value().unwrap(),
        MemoryValue::Undefined
    );
}
