//! Room information and the request bodies of room actions.

use overseer::model::world::{
    GameBuildableStructureType, GameIntentCancelOrder, GameIntentNotifyWhenAttacked, Tile,
};
use overseer::{Error, FlagColor, StatInterval};
use serde_json::{Value, json};

use crate::server::Request;
use crate::{connect, query, terrain_document};

/// Answers every room action and read; flag `Taken` exists.
fn routes(request: &Request) -> (u16, String) {
    let reply = match request.path.as_str() {
        "/api/game/room-terrain" => format!(
            r#"{{"ok":1,"terrain":[{}]}}"#,
            terrain_document(query(request, "room").unwrap_or_default())
        ),
        "/api/game/room-status" => r#"{"ok":1,"room":{"status":"normal","novice":5}}"#.to_owned(),
        "/api/game/room-overview" => {
            r#"{"ok":1,"owner":null,"stats":{},"statsMax":{},"totals":{}}"#.to_owned()
        }
        "/api/game/room-objects" => concat!(
            r#"{"ok":1,"users":{},"objects":["#,
            r#"{"_id":"o1","type":"marker","x":3,"y":4,"room":"W1N1"}]}"#
        )
        .to_owned(),
        "/api/game/room-decorations" => r#"{"ok":1,"decorations":[]}"#.to_owned(),
        "/api/game/gen-unique-flag-name" => r#"{"ok":1,"name":"Flag3"}"#.to_owned(),
        "/api/game/gen-unique-object-name" => r#"{"ok":1,"name":"Spawn2"}"#.to_owned(),
        "/api/game/check-unique-flag-name" if request.body.contains("Taken") => {
            r#"{"error":"name exists"}"#.to_owned()
        }
        "/api/game/create-construction" => r#"{"ok":1,"_id":"site1"}"#.to_owned(),
        _ => r#"{"ok":1}"#.to_owned(),
    };
    (200, reply)
}

/// The JSON body of the only request to `path`.
fn body(requests: &[Request], path: &str) -> Value {
    let mut matching = requests.iter().filter(|request| request.path == path);
    let request = matching
        .next()
        .unwrap_or_else(|| panic!("no request to {path}"));
    assert!(matching.next().is_none(), "several requests to {path}");
    assert_eq!(request.method, "POST", "{path}");
    serde_json::from_str(&request.body).unwrap()
}

#[tokio::test]
async fn room_reads_are_typed_and_terrain_is_cached() {
    let (server, overseer, _websocket) = connect(routes).await;
    let rooms = overseer.rooms();

    let terrain = rooms.terrain("W1N1", None).await.unwrap();
    assert_eq!(terrain.get(0, 0), Some(Tile::Wall));
    rooms.terrain("W1N1", None).await.unwrap();
    let status = rooms.status("W1N1", None).await.unwrap();
    assert_eq!(
        (status.status.as_deref(), status.novice),
        (Some("normal"), Some(5.0))
    );
    let overview = rooms
        .overview("W1N1", StatInterval::ThreeHours, None)
        .await
        .unwrap();
    assert!(overview.owner.is_none());
    let objects = rooms.objects("W1N1", None).await.unwrap();
    assert_eq!(objects.objects[0].kind(), "marker");
    assert_eq!(rooms.decorations("W1N1", None).await.unwrap().len(), 0);

    let requests = server.requests();
    let count = |path: &str| requests.iter().filter(|r| r.path == path).count();
    assert_eq!(count("/api/game/room-terrain"), 1);
    let overview = requests
        .iter()
        .find(|request| request.path == "/api/game/room-overview")
        .unwrap();
    assert_eq!(query(overview, "interval"), Some("180"));
    assert_eq!(query(overview, "room"), Some("W1N1"));
}

#[tokio::test]
async fn actions_send_the_documented_request_bodies() {
    let (server, overseer, _websocket) = connect(routes).await;
    let rooms = overseer.rooms();

    let colors = (FlagColor::Red, FlagColor::White);
    rooms
        .create_flag("W1N1", 10, 20, "Home", colors, None)
        .await
        .unwrap();
    let colors = (FlagColor::Blue, FlagColor::Green);
    rooms
        .change_flag_colors("W1N1", "Home", colors, None)
        .await
        .unwrap();
    rooms.remove_flag("W1N1", "Home", None).await.unwrap();
    let site = rooms
        .create_construction_site(
            "W1N1",
            5,
            6,
            GameBuildableStructureType::Spawn,
            Some("Spawn2"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(site.as_deref(), Some("site1"));
    rooms.place_spawn("W2N2", 25, 26, None, None).await.unwrap();
    rooms
        .set_notify_when_attacked("s1", false, None)
        .await
        .unwrap();
    let cancel = GameIntentCancelOrder {
        order_id: Some("order1".to_owned()),
    };
    rooms.global_intent(&cancel, None).await.unwrap();

    let requests = server.requests();
    assert_eq!(
        body(&requests, "/api/game/create-flag"),
        json!({"room": "W1N1", "x": 10.0, "y": 20.0, "name": "Home", "color": 1, "secondaryColor": 10})
    );
    assert_eq!(
        body(&requests, "/api/game/change-flag-color"),
        json!({"room": "W1N1", "name": "Home", "color": 3, "secondaryColor": 5})
    );
    assert_eq!(
        body(&requests, "/api/game/remove-flag"),
        json!({"room": "W1N1", "name": "Home"})
    );
    assert_eq!(
        body(&requests, "/api/game/create-construction"),
        json!({"room": "W1N1", "x": 5, "y": 6, "structureType": "spawn", "name": "Spawn2"})
    );
    assert_eq!(
        body(&requests, "/api/game/place-spawn"),
        json!({"room": "W2N2", "x": 25, "y": 26})
    );
    assert_eq!(
        body(&requests, "/api/game/set-notify-when-attacked"),
        json!({"_id": "s1", "enabled": false})
    );
    assert_eq!(
        body(&requests, "/api/game/add-global-intent"),
        json!({"name": "cancelOrder", "intent": {"orderId": "order1"}})
    );
}

#[tokio::test]
async fn intents_target_objects_or_the_room() {
    let (server, overseer, _websocket) = connect(routes).await;
    let rooms = overseer.rooms();

    rooms.suicide("W1N1", "creep1", None).await.unwrap();
    rooms
        .destroy_structure("W1N1", "wall1", None)
        .await
        .unwrap();
    let notify = GameIntentNotifyWhenAttacked {
        enabled: Some(true),
    };
    rooms
        .object_intent("W1N1", "tower1", &notify, None)
        .await
        .unwrap();

    let bodies = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/game/add-object-intent")
        .map(|request| serde_json::from_str::<Value>(&request.body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        bodies,
        [
            json!({"_id": "creep1", "room": "W1N1", "name": "suicide", "intent": {"id": "creep1"}}),
            json!({"_id": "room", "room": "W1N1", "name": "destroyStructure",
                "intent": [{"id": "wall1", "roomName": "W1N1"}]}),
            json!({"_id": "tower1", "room": "W1N1", "name": "notifyWhenAttacked",
                "intent": {"enabled": true}}),
        ]
    );
}

#[tokio::test]
async fn names_are_generated_and_checked() {
    let (server, overseer, _websocket) = connect(routes).await;
    let rooms = overseer.rooms();

    assert_eq!(rooms.unique_flag_name(None).await.unwrap(), "Flag3");
    assert_eq!(rooms.unique_spawn_name(None).await.unwrap(), "Spawn2");
    assert!(rooms.is_flag_name_free("Free", None).await.unwrap());
    assert!(!rooms.is_flag_name_free("Taken", None).await.unwrap());
    assert!(rooms.is_spawn_name_free("Spawn9", None).await.unwrap());
    assert_eq!(
        body(&server.requests(), "/api/game/gen-unique-object-name"),
        json!({"type": "spawn"})
    );
}

#[tokio::test]
async fn coordinates_outside_the_room_are_rejected_before_sending() {
    let (server, overseer, _websocket) = connect(routes).await;
    let colors = (FlagColor::Red, FlagColor::Red);
    let error = overseer
        .rooms()
        .create_flag("W1N1", 50, 0, "Edge", colors, None)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    let error = overseer
        .rooms()
        .place_spawn("W1N1", 0, 200, None, None)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");
    assert!(
        server
            .requests()
            .iter()
            .all(|request| !request.path.starts_with("/api/game/"))
    );
}
