//! The socket: authentication, token rotation, user channels and watched rooms.

use std::collections::BTreeSet;

use overseer_net::core::Token;
use overseer_net::core::api::Tile;
use overseer_net::core::api::types::GameRoomObject;
use overseer_net::core::socket::types::{UserConsole, UserCpuCpu};
use overseer_net::core::socket::{Event, Payload};
use overseer_net::{Socket, SocketEvent};

use crate::support::{self, Player};

/// A token the server never issued.
fn bogus() -> Token {
    Token::new("f".repeat(64))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn the_socket_authenticates_and_streams_user_channels() {
    let player = support::player().await;
    let client = player.account.client();
    let user = client
        .world()
        .await
        .expect("sign-in")
        .user()
        .expect("a user")
        .to_owned();
    let before = client.token();
    let mut socket = client.connect().await.expect("the socket opens");
    socket.subscribe_cpu().expect("signed in");
    socket.subscribe_console().expect("signed in");
    socket.subscribe_memory("ticks").expect("signed in");
    support::until(&mut socket, |_, event| match event {
        SocketEvent::Event(Event::Authenticated) => Some(()),
        SocketEvent::Event(Event::AuthFailed) => panic!("the session token is rejected"),
        _ => None,
    })
    .await;
    // `auth ok <token>` rotates the token and the client keeps it.
    let rotated = client.token();
    assert!(rotated.is_some());
    assert_ne!(rotated, before);
    assert_eq!(socket.world().user(), Some(user.as_str()));

    client
        .console(None, "'live ' + (6 * 7)")
        .await
        .expect("the expression is queued");
    let mut seen = BTreeSet::new();
    support::until(&mut socket, |_, event| {
        let SocketEvent::Event(Event::Channel { payload, .. }) = event else {
            return None;
        };
        match payload {
            Payload::Cpu(cpu) => {
                assert!(
                    matches!(cpu.cpu, UserCpuCpu::Number(spent) if spent >= 0.0),
                    "{cpu:?}"
                );
                seen.insert("cpu");
            }
            Payload::Console(UserConsole::Output(output)) => {
                if output
                    .messages
                    .log
                    .iter()
                    .any(|line| line.starts_with("tick "))
                {
                    seen.insert("log");
                }
                if output.messages.results.iter().any(|line| line == "live 42") {
                    seen.insert("result");
                }
            }
            Payload::Console(UserConsole::Error(error)) => panic!("the bot failed: {error:?}"),
            Payload::Memory(ticks) => {
                if ticks.parse::<u64>().is_ok_and(|ticks| ticks > 0) {
                    seen.insert("memory");
                }
            }
            other => panic!("unexpected payload {other:?}"),
        }
        (seen.len() == 4).then_some(())
    })
    .await;
    socket.close().await.expect("a clean close");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn a_rejected_socket_token_is_renewed_with_credentials_only() {
    let account = support::register().await;
    let client = account.client();
    client.world().await.expect("sign-in");
    let token_only = support::with_token(client.token().expect("a token"));

    // With credentials the socket signs in again and authenticates with the new token.
    client.set_token(Some(bogus()));
    let mut socket = client.connect().await.expect("the socket opens");
    support::until(&mut socket, |_, event| match event {
        SocketEvent::Event(Event::Authenticated) => Some(()),
        SocketEvent::Event(Event::AuthFailed) => panic!("auth failed reached the application"),
        _ => None,
    })
    .await;
    let renewed = client.token().expect("a token");
    assert_ne!(renewed, bogus());
    socket.close().await.expect("a clean close");

    // Without credentials `auth failed` is reported.
    token_only.world().await.expect("the token is valid");
    token_only.set_token(Some(bogus()));
    let mut socket = token_only.connect().await.expect("the socket opens");
    support::until(&mut socket, |_, event| match event {
        SocketEvent::Event(Event::AuthFailed) => Some(()),
        SocketEvent::Event(Event::Authenticated) => panic!("a bogus token was accepted"),
        _ => None,
    })
    .await;
    assert_eq!(token_only.token(), Some(bogus()));
    socket.close().await.expect("a clean close");
}

/// Whether the watched room of `player` shows its spawn and a creep, every object decoding.
fn shows_player(socket: &Socket, player: &Player, user: &str) -> bool {
    let Some(view) = socket.room_view(&player.room) else {
        return false;
    };
    let (mut spawn, mut creep) = (false, false);
    for (id, object) in view.objects() {
        match object.unwrap_or_else(|error| panic!("object {id} does not decode: {error}")) {
            GameRoomObject::Spawn(found) => {
                assert_eq!(found.name.as_deref(), Some("Spawn1"));
                assert_eq!(found.user, user);
                assert_eq!(
                    (found.x, found.y),
                    (u64::from(player.spawn.0), u64::from(player.spawn.1))
                );
                spawn = true;
            }
            GameRoomObject::Creep(found) => creep |= found.user == user,
            _ => {}
        }
    }
    spawn && creep && view.game_time().is_some()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn watched_rooms_decode_terrain_objects_users_and_the_map() {
    let player = support::player().await;
    let client = player.account.client();
    let world = client.world().await.expect("sign-in");
    let user = world.user().expect("a user").to_owned();
    let mut socket = client.connect().await.expect("the socket opens");
    socket
        .watch_room(&player.room)
        .await
        .expect("the room's terrain");
    socket.subscribe_room_map(&player.room);
    let view = socket.room_view(&player.room).expect("a watched room");
    assert_eq!(view.room(), player.room);
    assert_eq!(view.tile(player.spawn.0, player.spawn.1), Some(Tile::Plain));
    assert_eq!(view.tile(50, 0), None);

    let (mut room, mut map) = (false, false);
    support::until(&mut socket, |socket, event| {
        if let SocketEvent::Event(Event::Channel { payload, .. }) = event {
            match payload {
                Payload::Room => room |= shows_player(socket, &player, &user),
                Payload::RoomMap(view) => {
                    // The map lists sources and the player's own objects under its id.
                    if view.other.contains_key(&user) {
                        assert!(view.s.as_ref().is_some_and(|sources| !sources.is_empty()));
                        map = true;
                    }
                }
                other => panic!("unexpected payload {other:?}"),
            }
        }
        (room && map).then_some(())
    })
    .await;
    let view = socket.room_view(&player.room).expect("a watched room");
    let (_, owner) = view
        .users()
        .find(|(id, _)| *id == user)
        .expect("the player is a room user");
    assert_eq!(owner.username, player.account.username);
    assert_eq!(view.flags().expect("flags decode").len(), 0);
    for visual in view.visuals() {
        visual.expect("visuals decode");
    }
    socket.close().await.expect("a clean close");
}
