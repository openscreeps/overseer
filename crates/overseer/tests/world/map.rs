//! Map views (viewport diffing over shared subscriptions), batch terrain, stats and visuals.

use std::collections::BTreeSet;

use overseer::map::{RoomCoord, ViewChange};
use overseer::model::world::{GameRoomStatName, Tile};
use overseer::{MapStat, StatInterval};
use serde_json::{Value, json};

use crate::server::{self, Request};
use crate::{connect, receive, soon, terrain_document};

/// Batch terrain for every requested room except `W9N9`, map stats and the world size.
fn routes(request: &Request) -> (u16, String) {
    match request.path.as_str() {
        "/api/game/rooms" => {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            let documents = body["rooms"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .filter(|room| *room != "W9N9")
                .map(terrain_document)
                .collect::<Vec<_>>();
            (200, format!(r#"{{"ok":1,"rooms":[{}]}}"#, documents.join(",")))
        }
        "/api/game/room-terrain" => (200, r#"{"ok":1,"terrain":[]}"#.to_owned()),
        "/api/game/map-stats" => (
            200,
            r#"{"ok":1,"gameTime":7,"stats":{"W1N1":{"status":"normal","own":{"user":"u1","level":3}}},"users":{}}"#
                .to_owned(),
        ),
        "/api/game/world-size" => (200, r#"{"ok":1,"width":12,"height":10}"#.to_owned()),
        _ => (401, r#"{"error":"unauthorized"}"#.to_owned()),
    }
}

/// The rooms of the 3x3 viewport around `center`.
fn viewport(center: &str) -> Vec<String> {
    let center: RoomCoord = center.parse().unwrap();
    center.around(1).map(|room| room.to_string()).collect()
}

/// `subscribe`/`unsubscribe` frames of `rooms`, as a set (the order within a change is
/// unspecified).
fn frames<'a>(verb: &str, rooms: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    rooms
        .into_iter()
        .map(|room| format!("{verb} roomMap2:{room}"))
        .collect()
}

#[tokio::test]
async fn moving_a_viewport_only_subscribes_and_releases_the_changed_rooms() {
    let (_server, overseer, mut websocket) = connect(routes).await;
    let mut view = overseer.map().view(None).unwrap();

    let first = viewport("W1N1");
    let change = view.set_rooms(&first);
    assert_eq!(change.added.len(), 9);
    assert_eq!(change.removed, Vec::<String>::new());
    let sent = receive(&mut websocket, 9).await;
    assert_eq!(
        sent.into_iter().collect::<BTreeSet<_>>(),
        frames("subscribe", first.iter().map(String::as_str))
    );

    // One room east: the western column leaves, a new eastern column enters.
    let second = viewport("W0N1");
    let change = view.set_rooms(&second);
    assert_eq!(
        change,
        ViewChange {
            added: vec!["E0N0".into(), "E0N1".into(), "E0N2".into()],
            removed: vec!["W2N0".into(), "W2N1".into(), "W2N2".into()],
        }
    );
    let sent = receive(&mut websocket, 6)
        .await
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut expected = frames("unsubscribe", ["W2N0", "W2N1", "W2N2"]);
    expected.extend(frames("subscribe", ["E0N0", "E0N1", "E0N2"]));
    assert_eq!(sent, expected);
    assert_eq!(view.set_rooms(&second), ViewChange::default());

    // A second view shares subscriptions: only its new room is subscribed, and a room both
    // show stays subscribed until both release it.
    let mut other = overseer.map().view(None).unwrap();
    other.set_rooms(["W1N1", "W5N5"]);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe roomMap2:W5N5"]
    );
    view.set_rooms(["W5N5"]);
    let sent = receive(&mut websocket, 8).await;
    assert!(
        !sent.contains(&"unsubscribe roomMap2:W1N1".to_owned()),
        "{sent:?}"
    );
    assert!(
        sent.iter().all(|frame| frame.starts_with("unsubscribe")),
        "{sent:?}"
    );
    drop(other);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe roomMap2:W1N1"]
    );
    drop(view);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe roomMap2:W5N5"]
    );
}

#[tokio::test]
async fn views_report_the_latest_map_view_of_each_room() {
    let (_server, overseer, mut websocket) = connect(routes).await;
    let mut view = overseer.map().view(None).unwrap();
    view.set_rooms(["W1N1", "W2N2"]);
    receive(&mut websocket, 2).await;
    assert!(view.latest("W1N1").is_none());

    server::send(
        &mut websocket,
        r#"["roomMap2:W2N2",{"s":[[10,20]],"u1":[[1,1],[2,2]]}]"#,
    )
    .await;
    let (room, map) = soon(view.changed()).await.unwrap();
    assert_eq!(room, "W2N2");
    assert_eq!(map.sources().len(), 1);
    assert_eq!(
        map.users().next().map(|(user, at)| (user, at.len())),
        Some(("u1", 2))
    );
    assert_eq!(view.views().keys().collect::<Vec<_>>(), ["W2N2"]);
    assert_eq!(view.latest("W2N2").unwrap().sources().len(), 1);

    let mut visuals = overseer.map().visuals(None).unwrap();
    assert_eq!(receive(&mut websocket, 1).await, ["subscribe mapVisual:u1"]);
    server::send(
        &mut websocket,
        r#"["mapVisual:u1","{\"t\":\"c\",\"x\":1,\"y\":2,\"n\":\"W1N1\",\"s\":{}}\n"]"#,
    )
    .await;
    assert_eq!(soon(visuals.changed()).await.unwrap().len(), 1);
}

#[tokio::test]
async fn batch_terrain_fetches_only_uncached_rooms() {
    let (server, overseer, _websocket) = connect(routes).await;
    let map = overseer.map();

    let terrain = map.terrain(["W1N1", "W2N2", "W9N9"], None).await.unwrap();
    assert_eq!(terrain.keys().collect::<Vec<_>>(), ["W1N1", "W2N2"]);
    assert_eq!(terrain["W1N1"].get(0, 0), Some(Tile::Wall));
    let again = map.terrain(["W2N2", "W3N3"], None).await.unwrap();
    assert!(std::sync::Arc::ptr_eq(&again["W2N2"], &terrain["W2N2"]));
    // The single-room terrain shares the cache.
    overseer.rooms().terrain("W3N3", None).await.unwrap();

    let requests = server.requests();
    let bodies = requests
        .iter()
        .filter(|request| request.path == "/api/game/rooms")
        .map(|request| serde_json::from_str::<Value>(&request.body).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        bodies,
        [
            json!({"rooms": ["W1N1", "W2N2", "W9N9"]}),
            json!({"rooms": ["W3N3"]})
        ]
    );
    assert!(
        requests
            .iter()
            .all(|request| request.path != "/api/game/room-terrain")
    );
}

#[tokio::test]
async fn map_stats_and_world_size_are_typed() {
    let (server, overseer, _websocket) = connect(routes).await;
    let map = overseer.map();
    let stat = MapStat::Ranked(
        GameRoomStatName::EnergyHarvested,
        StatInterval::EightMinutes,
    );
    let stats = map.stats(["W1N1", "W2N2"], stat, None).await.unwrap();
    assert_eq!(stats.game_time, 7);
    assert_eq!(stats.stats["W1N1"].own.as_ref().unwrap().level, 3);
    let size = map.world_size(None).await.unwrap();
    assert_eq!((size.width, size.height), (12, 10));
    let request = server
        .requests()
        .into_iter()
        .find(|request| request.path == "/api/game/map-stats")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&request.body).unwrap(),
        json!({"rooms": ["W1N1", "W2N2"], "statName": "energyHarvested8"})
    );
}
