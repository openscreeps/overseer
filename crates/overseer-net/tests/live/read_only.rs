//! Read-only tests of the [`support::reader`] account: the user, the world, the start room and
//! the socket. They only read (GET requests and the socket), so they also run against
//! screeps.com (`cargo xtask e2e --target official`) and catch contract drift there.

use overseer_net::SocketEvent;
use overseer_net::core::api::Tile;
use overseer_net::core::api::endpoints;
use overseer_net::core::api::types::GameRoomObject;
use overseer_net::core::socket::{Channel, Event, Payload};

use crate::support;

/// The reader's user and the server's world decode: `GET /api/version`, `GET /api/auth/me`
/// (through [`overseer_net::Client::world`]), then `GET /api/auth/me` once more.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live server: cargo xtask e2e (read-only, also --target official)"]
async fn the_readers_user_and_the_world_decode() {
    let client = support::reader();
    let world = client.world().await.expect("the world and the user");
    let user = world.user().expect("an authenticated user");
    assert_eq!(world.is_sharded(), support::official());
    if world.is_sharded() {
        let shard = world.shard().expect("a default shard");
        assert!(
            world.shards().iter().any(|known| known == shard),
            "{shard} is not one of {:?}",
            world.shards()
        );
    } else {
        assert_eq!(world.shard(), None);
    }

    let me = client.send(&endpoints::AuthMe).await.expect("auth/me").body;
    assert_eq!(me.id.as_str(), user);
    let username = me.username.as_deref().expect("a username");
    assert_ne!(username, "");
    if !support::official() {
        assert_eq!(username, support::admin().username);
    }
}

/// The reader's start room, its terrain and its objects decode on the room's shard:
/// `GET /api/version`, `GET /api/auth/me`, `GET /api/user/world-start-room`,
/// `GET /api/game/room-terrain`, `GET /api/game/room-objects`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live server: cargo xtask e2e (read-only, also --target official)"]
async fn the_start_room_terrain_and_objects_decode() {
    let client = support::reader();
    let world = client.world().await.expect("the world and the user");
    let user = world.user().expect("an authenticated user").to_owned();
    let (shard, room) = support::start_room(&client).await;
    match &shard {
        Some(shard) => assert!(
            world.shards().contains(shard),
            "{shard} is not one of {:?}",
            world.shards()
        ),
        None => assert!(!world.is_sharded(), "a sharded server names the shard"),
    }
    assert!(
        room.starts_with('W') || room.starts_with('E'),
        "not a room name: {room}"
    );

    let terrain = client
        .terrain(shard.as_deref(), &room)
        .await
        .expect("the start room's terrain");
    let tiles: Vec<Tile> = (0..50)
        .flat_map(|y| (0..50).map(move |x| (x, y)))
        .map(|(x, y)| terrain.get(x, y).expect("a tile"))
        .collect();
    assert!(tiles.contains(&Tile::Wall), "a room has walls");

    let snapshot = client
        .room_objects(shard.as_deref(), &room)
        .await
        .expect("the start room's objects decode");
    assert_ne!(snapshot.objects.len(), 0, "{room} is empty");
    if support::official() {
        // On screeps.com the reader owns its start room.
        assert!(
            snapshot.objects.iter().any(|object| matches!(
                object,
                GameRoomObject::Controller(controller)
                    if controller.user.as_deref() == Some(user.as_str())
            )),
            "the reader does not own {room}"
        );
        assert!(
            snapshot.users.contains_key(&user),
            "the reader is not a user of {room}"
        );
    }
}

/// The socket authenticates with the reader's token and delivers one of its user channels
/// (`user:<id>/cpu` on every run of its code, `user:<id>/resources` at once on `OpenScreeps`):
/// `GET /api/version`, `GET /api/auth/me`, the upgrade `GET /socket/websocket`.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live server: cargo xtask e2e (read-only, also --target official)"]
async fn the_socket_authenticates_and_delivers_a_user_channel() {
    let client = support::reader();
    let user = client
        .world()
        .await
        .expect("the world and the user")
        .user()
        .expect("an authenticated user")
        .to_owned();
    let mut socket = client.connect().await.expect("the socket opens");
    socket.subscribe_cpu().expect("signed in");
    socket.subscribe_resources().expect("signed in");
    support::until(&mut socket, |_, event| match event {
        SocketEvent::Event(Event::Authenticated) => Some(()),
        SocketEvent::Event(Event::AuthFailed) => panic!("the token is rejected"),
        _ => None,
    })
    .await;
    assert_eq!(socket.world().user(), Some(user.as_str()));

    let channel = support::until(&mut socket, |_, event| match event {
        SocketEvent::Event(Event::Channel {
            channel,
            payload: Payload::Cpu(_) | Payload::Resources(_),
        }) => Some(channel.clone()),
        SocketEvent::Event(Event::ChannelError { channel, message }) => {
            panic!("{channel:?} failed: {message}")
        }
        SocketEvent::Event(Event::CannotSubscribe(line)) => panic!("{line}"),
        _ => None,
    })
    .await;
    assert!(
        matches!(
            &channel,
            Channel::Cpu { user: owner } | Channel::Resources { user: owner } if *owner == user
        ),
        "{channel:?}"
    );
    socket.close().await.expect("a clean close");
}
