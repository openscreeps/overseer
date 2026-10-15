//! Reference-counted room subscriptions and room snapshots.

use std::sync::Arc;

use overseer::Notice;
use overseer::core::api::Tile;
use overseer::core::socket::Channel;

use crate::server::{self, Server};
use crate::{connect, receive, routes, soon};

#[tokio::test]
async fn watches_of_a_room_share_one_subscription_until_the_last_is_dropped() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let rooms = overseer.rooms();

    let first = rooms.watch("W1N1").await.unwrap();
    let second = rooms.watch("W1N1").await.unwrap();
    let clone = second.clone();
    assert_eq!(
        *first.channel(),
        Channel::Room {
            shard: None,
            room: "W1N1".to_owned()
        }
    );
    let other = rooms.watch("W2N2").await.unwrap();
    // One `subscribe` per room: the second watch and the clone sent nothing.
    assert_eq!(
        receive(&mut websocket, 2).await,
        ["subscribe room:W1N1", "subscribe room:W2N2"]
    );

    drop(first);
    drop(second);
    drop(other);
    // Dropping two of the three references to W1N1 sent nothing.
    assert_eq!(receive(&mut websocket, 1).await, ["unsubscribe room:W2N2"]);
    drop(clone);
    assert_eq!(receive(&mut websocket, 1).await, ["unsubscribe room:W1N1"]);
    // Terrain is fetched once per room.
    let terrain = server
        .requests()
        .into_iter()
        .filter(|request| request.path == "/api/game/room-terrain")
        .count();
    assert_eq!(terrain, 2);
}

#[tokio::test]
async fn room_updates_become_typed_snapshots() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let mut notices = overseer.notices();
    let mut room = overseer.rooms().watch("W1N1").await.unwrap();
    assert!(room.latest().is_none());
    assert_eq!(receive(&mut websocket, 1).await, ["subscribe room:W1N1"]);
    server::send(
        &mut websocket,
        concat!(
            r#"["room:W1N1",{"gameTime":9,"objects":{"#,
            r#""o1":{"_id":"o1","type":"marker","x":3,"y":4,"room":"W1N1","user":"u1"},"#,
            r#""o2":{"_id":"o2","type":"source","x":"west","y":1,"room":"W1N1"}},"#,
            r#""users":{"u1":{"_id":"u1","username":"Player"}},"#,
            r#""flags":"Home~1~2~10~20","#,
            r#""visual":"{\"t\":\"c\",\"x\":1,\"y\":2}\n{\"t\":\"?\"}\n"}]"#
        ),
    )
    .await;
    let snapshot = soon(room.changed()).await.unwrap();
    assert_eq!(
        (snapshot.room.as_str(), snapshot.shard.as_deref()),
        ("W1N1", None)
    );
    assert_eq!(snapshot.game_time, Some(9));
    assert_eq!(snapshot.terrain.get(0, 0), Some(Tile::Wall));
    assert_eq!(snapshot.terrain.get(1, 0), Some(Tile::Plain));
    assert_eq!(snapshot.objects.len(), 1);
    let marker = &snapshot.objects[0];
    assert_eq!(
        (marker.id(), marker.kind(), marker.x(), marker.y()),
        ("o1", "marker", 3, 4)
    );
    assert_eq!(snapshot.undecoded.len(), 1);
    assert_eq!(snapshot.undecoded[0].0, "o2");
    assert_eq!(snapshot.users["u1"].username, "Player");
    assert_eq!(
        snapshot
            .flags
            .iter()
            .map(|flag| (flag.name.as_str(), flag.x, flag.y))
            .collect::<Vec<_>>(),
        [("Home", 10, 20)]
    );
    assert_eq!(snapshot.visuals.len(), 1);
    assert_eq!(snapshot.subscription_error, None);

    // A new watch of the room starts with the same snapshot, built once.
    let mut again = overseer.rooms().watch("W1N1").await.unwrap();
    assert!(Arc::ptr_eq(
        &soon(again.changed()).await.unwrap(),
        &snapshot
    ));

    server::send(
        &mut websocket,
        r#"["err@room:W1N1","subscribe limit reached"]"#,
    )
    .await;
    let refused = soon(room.changed()).await.unwrap();
    assert_eq!(
        refused.subscription_error.as_deref(),
        Some("subscribe limit reached")
    );
    assert_eq!(refused.game_time, Some(9));
    assert_eq!(
        soon(notices.recv()).await.unwrap(),
        Notice::ChannelError {
            channel: room.channel().clone(),
            message: "subscribe limit reached".to_owned()
        }
    );

    // Diffs apply to the room state; the next snapshot clears the error.
    server::send(
        &mut websocket,
        r#"["room:W1N1",{"gameTime":10,"objects":{"o2":null}}]"#,
    )
    .await;
    let next = soon(room.changed()).await.unwrap();
    assert_eq!(next.game_time, Some(10));
    assert_eq!((next.objects.len(), next.undecoded.len()), (1, 0));
    assert_eq!(next.subscription_error, None);
    assert!(Arc::ptr_eq(&next.terrain, &snapshot.terrain));
}
