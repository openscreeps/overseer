//! Reconnects, backoff, cancellation, keep-alive, `auth ok` tokens, the event stream and
//! watched rooms.

use std::time::Duration;

use async_tungstenite::tungstenite::Message;
use futures_util::StreamExt;
use futures_util::stream::FusedStream;
use overseer_core::Token;
use overseer_core::api::endpoints;
use overseer_core::socket::{Channel, Event};
use overseer_net::{Client, ReconnectPolicy, SocketEvent};
use tokio::time::Instant;

use crate::server::{self, Request, Server};
use crate::{ME, PATIENCE, VERSION, describe, descriptions_until, events_until, fast_reconnect};

fn single_world(request: &Request) -> (u16, String) {
    match request.path.as_str() {
        "/api/version" => (200, VERSION.to_owned()),
        _ => (404, "{}".to_owned()),
    }
}

#[tokio::test]
async fn subscriptions_are_sent_again_after_the_server_drops_the_connection() {
    let mut server = Server::start(single_world, |_| true).await;
    let client = Client::builder(server.url())
        .reconnect(fast_reconnect(None))
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    assert_eq!(socket.subscribe_server_messages(), Channel::ServerMessage);
    let room = socket.subscribe_room("W1N1");
    assert_eq!(
        room,
        Channel::Room {
            shard: None,
            room: "W1N1".to_owned()
        }
    );
    assert!(matches!(
        socket.subscribe_cpu(),
        Err(overseer_net::Error::Anonymous)
    ));

    let (events, (first, second, _open)) = tokio::join!(
        descriptions_until(&mut socket, |event| matches!(
            event,
            SocketEvent::Event(Event::Channel { .. })
        )),
        async {
            let mut first = server.socket().await;
            let lines = server::receive(&mut first, 3).await;
            first.close(None).await.unwrap();
            let mut second = server.socket().await;
            let again = server::receive(&mut second, 3).await;
            server::send(&mut second, r#"["room:W1N1",{"gameTime":7,"objects":{}}]"#).await;
            (lines, again, second)
        }
    );
    let subscribed = ["gzip on", "subscribe server-message", "subscribe room:W1N1"];
    assert_eq!(first, subscribed);
    assert_eq!(second, subscribed);
    assert_eq!(
        events,
        [
            "connected".to_owned(),
            "closed".to_owned(),
            "reconnecting 1 after 20ms".to_owned(),
            "connected".to_owned(),
            format!(
                "{:?}",
                Event::Channel {
                    channel: room,
                    payload: overseer_core::socket::Payload::Room
                }
            ),
        ]
    );
    assert_eq!(socket.room("W1N1").unwrap().game_time(), Some(7));
}

#[tokio::test]
async fn reconnect_attempts_back_off_exponentially() {
    // The first upgrade succeeds, the next three fail, the fifth succeeds.
    let mut server = Server::start(single_world, |index| index == 0 || index >= 4).await;
    let client = Client::builder(server.url())
        .reconnect(fast_reconnect(None))
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    let mut first = server.socket().await;
    first.close(None).await.unwrap();

    let mut connections = 0;
    let events = events_until(&mut socket, |event| {
        connections += u32::from(matches!(event, SocketEvent::Connected));
        connections == 2
    })
    .await;
    let names: Vec<&str> = events.iter().map(|(_, event)| event.as_str()).collect();
    assert_eq!(
        names,
        [
            "connected",
            "closed",
            "reconnecting 1 after 20ms",
            "connect failed",
            "reconnecting 2 after 40ms",
            "connect failed",
            "reconnecting 3 after 50ms",
            "connect failed",
            "reconnecting 4 after 50ms",
            "connected",
        ]
    );
    // Each attempt starts no earlier than its delay after the preceding failure.
    for (failure, delay) in [(1, 20), (3, 40), (5, 50), (7, 50)] {
        let waited = events[failure + 2].0 - events[failure].0;
        assert!(
            waited >= Duration::from_millis(delay),
            "attempt after event {failure} waited {waited:?}"
        );
    }
}

#[tokio::test]
async fn the_socket_ends_when_reconnect_attempts_run_out() {
    let mut server = Server::start(single_world, |index| index == 0).await;
    let client = Client::builder(server.url())
        .reconnect(fast_reconnect(Some(2)))
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    // Kept until the end: dropping the TCP stream early could turn the close into a reset.
    let mut first = server.socket().await;
    first.close(None).await.unwrap();

    let events = descriptions_until(&mut socket, |_| false).await;
    assert_eq!(
        events,
        [
            "connected",
            "closed",
            "reconnecting 1 after 20ms",
            "connect failed",
            "reconnecting 2 after 40ms",
            "connect failed",
        ]
    );
    assert!(socket.next().await.unwrap().is_none());
}

#[tokio::test]
async fn dropped_calls_keep_the_backoff_deadline_and_queued_subscriptions() {
    let mut server = Server::start(single_world, |_| true).await;
    let client = Client::builder(server.url())
        .reconnect(ReconnectPolicy {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(100),
            max_attempts: None,
        })
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    let mut first = server.socket().await;
    first.close(None).await.unwrap();
    let events = descriptions_until(&mut socket, |event| {
        matches!(event, SocketEvent::Reconnecting { .. })
    })
    .await;
    assert_eq!(
        events,
        ["connected", "closed", "reconnecting 1 after 100ms"]
    );

    // Calls cancelled after 10 ms must not restart the 100 ms wait or lose the subscription.
    socket.subscribe_room("W2N2");
    let started = Instant::now();
    let event = loop {
        if let Ok(event) = tokio::time::timeout(Duration::from_millis(10), socket.next()).await {
            break event.unwrap().unwrap();
        }
        assert!(started.elapsed() < PATIENCE, "never reconnected");
    };
    assert!(matches!(event, SocketEvent::Connected), "{event:?}");
    let (_, lines) = tokio::join!(
        tokio::time::timeout(Duration::from_millis(50), socket.next()),
        async {
            let mut second = server.socket().await;
            server::receive(&mut second, 2).await
        }
    );
    assert_eq!(lines, ["gzip on", "subscribe room:W2N2"]);
}

#[tokio::test]
async fn keep_alive_detects_a_silent_server() {
    let mut server = Server::start(single_world, |_| true).await;
    let client = Client::builder(server.url())
        .keep_alive(Some(Duration::from_millis(50)))
        .reconnect(fast_reconnect(Some(0)))
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    // Accepted, then never read: pings stay unanswered.
    let _silent = server.socket().await;

    let events = events_until(&mut socket, |_| false).await;
    let names: Vec<&str> = events.iter().map(|(_, event)| event.as_str()).collect();
    assert_eq!(names, ["connected", "keep-alive timeout"]);
    let silence = events[1].0 - events[0].0;
    assert!(
        silence >= Duration::from_millis(100) && silence < Duration::from_secs(2),
        "dead connection detected after {silence:?}"
    );
}

#[tokio::test]
async fn keep_alive_pings_keep_a_quiet_connection_open() {
    let mut server = Server::start(single_world, |_| true).await;
    let client = Client::builder(server.url())
        .keep_alive(Some(Duration::from_millis(50)))
        .reconnect(fast_reconnect(Some(0)))
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    let mut websocket = server.socket().await;

    let (events, pings) = tokio::join!(
        descriptions_until(&mut socket, |event| matches!(event, SocketEvent::Event(_))),
        async {
            // Reading answers pings; stay quiet for six intervals.
            let mut pings = 0;
            let mut quiet = std::pin::pin!(tokio::time::sleep(Duration::from_millis(300)));
            loop {
                tokio::select! {
                    () = &mut quiet => break,
                    message = websocket.next() => match message {
                        Some(Ok(Message::Ping(_))) => pings += 1,
                        Some(Ok(_)) => {}
                        Some(Err(_)) | None => break,
                    },
                }
            }
            server::send(&mut websocket, "protocol 14").await;
            pings
        }
    );
    assert_eq!(events, ["connected", "Protocol(14)"]);
    assert!(pings >= 2, "{pings} pings");
}

#[tokio::test]
async fn auth_ok_tokens_reach_the_http_client() {
    let mut server = Server::start(
        |request| match (request.path.as_str(), request.token.as_deref()) {
            ("/api/version", _) => (200, VERSION.to_owned()),
            ("/api/auth/me", Some("t1" | "t2")) => (200, ME.to_owned()),
            _ => (401, r#"{"error":"unauthorized"}"#.to_owned()),
        },
        |_| true,
    )
    .await;
    let client = Client::builder(server.url())
        .token(Token::new("t1"))
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    assert_eq!(socket.world().user(), Some("u1"));
    assert_eq!(
        socket.subscribe_cpu().unwrap(),
        Channel::Cpu {
            user: "u1".to_owned()
        }
    );
    let mut websocket = server.socket().await;

    let (events, lines) = tokio::join!(
        descriptions_until(&mut socket, |event| matches!(
            event,
            SocketEvent::Event(Event::Protocol(_))
        )),
        async {
            let mut lines = server::receive(&mut websocket, 2).await;
            server::send(&mut websocket, "auth ok t2").await;
            lines.extend(server::receive(&mut websocket, 1).await);
            server::send(&mut websocket, "protocol 14").await;
            lines
        }
    );
    assert_eq!(events, ["connected", "Authenticated", "Protocol(14)"]);
    assert_eq!(lines, ["gzip on", "auth t1", "subscribe user:u1/cpu"]);
    assert_eq!(client.token(), Some(Token::new("t2")));

    client.send(&endpoints::AuthMe).await.unwrap();
    let last = server.requests().pop().unwrap();
    assert_eq!(
        (last.path.as_str(), last.token.as_deref()),
        ("/api/auth/me", Some("t2"))
    );
}

#[tokio::test]
async fn the_socket_is_a_stream_that_ends_when_reconnects_run_out() {
    let mut server = Server::start(single_world, |index| index == 0).await;
    let client = Client::builder(server.url())
        .reconnect(fast_reconnect(Some(0)))
        .build()
        .unwrap();
    let mut socket = client.connect().await.unwrap();
    let room = socket.subscribe_room("W1N1");

    let (events, (lines, _first)) = tokio::time::timeout(PATIENCE, async {
        tokio::join!(
            async {
                let mut events = Vec::new();
                while let Some(event) = StreamExt::next(&mut socket).await {
                    events.push(describe(&event.unwrap()));
                }
                events
            },
            async {
                let mut first = server.socket().await;
                let lines = server::receive(&mut first, 2).await;
                server::send(&mut first, r#"["room:W1N1",{"gameTime":3,"objects":{}}]"#).await;
                first.close(None).await.unwrap();
                // Kept until the end: dropping the TCP stream early could turn the close into a
                // reset.
                (lines, first)
            }
        )
    })
    .await
    .expect("the stream ends in time");
    assert_eq!(lines, ["gzip on", "subscribe room:W1N1"]);
    assert_eq!(
        events,
        [
            "connected".to_owned(),
            format!(
                "{:?}",
                Event::Channel {
                    channel: room,
                    payload: overseer_core::socket::Payload::Room
                }
            ),
            "closed".to_owned(),
        ]
    );
    assert!(socket.is_terminated());
    assert!(StreamExt::next(&mut socket).await.is_none());
}

#[tokio::test]
async fn watched_rooms_combine_terrain_and_live_state() {
    let mut server = Server::start(crate::flows::terrain_routes, |_| true).await;
    let client = Client::builder(server.url()).build().unwrap();
    let mut socket = client.connect().await.unwrap();
    assert!(socket.room_view("W1N1").is_none());
    let room = socket.watch_room("W1N1").await.unwrap();
    let view = socket.room_view("W1N1").unwrap();
    assert_eq!(view.room(), "W1N1");
    assert_eq!(view.tile(1, 0), Some(overseer_core::api::Tile::Swamp));
    assert!(view.state().is_none());
    assert_eq!(view.objects().count(), 0);

    let (events, _open) = tokio::join!(
        descriptions_until(&mut socket, |event| matches!(
            event,
            SocketEvent::Event(Event::Channel { .. })
        )),
        async {
            let mut websocket = server.socket().await;
            server::receive(&mut websocket, 2).await;
            server::send(
                &mut websocket,
                concat!(
                    r#"["room:W1N1",{"gameTime":9,"#,
                    r#""objects":{"o1":{"_id":"o1","type":"marker","x":3,"y":4,"room":"W1N1","user":"u1"}},"#,
                    r#""users":{"u1":{"_id":"u1","username":"Player"}},"#,
                    r#""flags":"Home~1~2~10~20","#,
                    r#""visual":"{\"t\":\"c\",\"x\":1,\"y\":2}\n"}]"#
                ),
            )
            .await;
            websocket
        }
    );
    assert_eq!(events.len(), 2, "{events:?}");
    let view = socket.room_view("W1N1").unwrap();
    assert_eq!(view.game_time(), Some(9));
    let objects: Vec<_> = view
        .objects()
        .map(|(id, object)| (id, object.unwrap()))
        .collect();
    assert_eq!(objects.len(), 1);
    let (id, object) = &objects[0];
    assert_eq!(*id, "o1");
    assert_eq!(
        (object.kind(), object.x(), object.y(), object.user()),
        ("marker", 3, 4, Some("u1"))
    );
    let users: Vec<_> = view
        .users()
        .map(|(id, user)| (id, user.username.as_str()))
        .collect();
    assert_eq!(users, [("u1", "Player")]);
    let flags = view.flags().unwrap();
    assert_eq!(
        (flags.len(), flags[0].name.as_str(), flags[0].x, flags[0].y),
        (1, "Home", 10, 20)
    );
    assert_eq!(view.visuals().flatten().count(), 1);

    socket.unsubscribe(&room);
    assert!(socket.room_view("W1N1").is_none());
}
