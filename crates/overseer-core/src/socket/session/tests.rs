//! Tests of [`SocketSession`]: the sign-in, subscriptions, reconnects, received frames and
//! the decoding of channel events with the contract of each server.

use serde_json::json;

use super::{Event, Payload, SessionState, SocketError, SocketSession};
use crate::Token;
use crate::model::console::ConsoleEvent;
use crate::model::platform::ServerKind;
use crate::model::socket::{UserCpu, UserCpuCpu};
use crate::model::world::MapVisualItem;
use crate::socket::Channel;

/// The contract of the tests that do not depend on it.
const KIND: ServerKind = ServerKind::Official;

const USER: &str = "5a1b";

fn drain(session: &mut SocketSession) -> Vec<String> {
    std::iter::from_fn(|| session.poll_transmit()).collect()
}

fn room(name: &str) -> Channel {
    Channel::Room {
        shard: Some("shard3".into()),
        room: name.into(),
    }
}

fn cpu() -> Channel {
    Channel::Cpu { user: USER.into() }
}

fn signed_in() -> SocketSession {
    signed_in_to(KIND)
}

fn signed_in_to(kind: ServerKind) -> SocketSession {
    let mut session = SocketSession::new(Some(Token::new("tok-1")), kind);
    session.handle_frame("auth ok tok-1").unwrap();
    drain(&mut session);
    session
}

#[test]
fn subscriptions_wait_for_auth_ok_and_adopt_the_rotated_token() {
    let mut session = SocketSession::new(Some(Token::new("tok-1")), KIND);
    assert_eq!(session.state(), SessionState::Authenticating);
    session.subscribe(cpu());
    session.subscribe(room("W8N3"));
    session.subscribe(cpu());
    assert_eq!(drain(&mut session), ["gzip on", "auth tok-1"]);

    let event = session.handle_frame("auth ok tok-2").unwrap();
    assert_eq!(event, Some(Event::Authenticated));
    assert_eq!(session.state(), SessionState::Ready);
    assert_eq!(session.token().map(Token::as_str), Some("tok-2"));
    assert_eq!(
        drain(&mut session),
        ["subscribe user:5a1b/cpu", "subscribe room:shard3/W8N3"]
    );

    session.subscribe(room("W8N3"));
    session.subscribe(Channel::ServerMessage);
    assert_eq!(
        drain(&mut session),
        ["subscribe server-message"],
        "no duplicate subscribe"
    );
    assert_eq!(session.subscriptions().count(), 3);
}

#[test]
fn anonymous_sessions_are_ready_at_once() {
    let mut session = SocketSession::new(None, KIND);
    assert_eq!(session.state(), SessionState::Ready);
    session.subscribe(Channel::ServerMessage);
    assert_eq!(drain(&mut session), ["gzip on", "subscribe server-message"]);
}

#[test]
fn unsubscribe_sends_only_what_was_subscribed() {
    let mut session = SocketSession::new(Some(Token::new("tok-1")), KIND);
    session.subscribe(cpu());
    session.unsubscribe(&cpu());
    session.unsubscribe(&room("W1N1"));
    session.handle_frame("auth ok tok-1").unwrap();
    assert_eq!(drain(&mut session), ["gzip on", "auth tok-1"]);

    session.subscribe(cpu());
    session.unsubscribe(&cpu());
    assert_eq!(
        drain(&mut session),
        ["subscribe user:5a1b/cpu", "unsubscribe user:5a1b/cpu"]
    );
    assert_eq!(session.subscriptions().count(), 0);
}

#[test]
fn auth_failed_sends_nothing_until_authenticated_again() {
    let mut session = SocketSession::new(Some(Token::new("bad")), KIND);
    session.subscribe(cpu());
    drain(&mut session);
    assert_eq!(
        session.handle_frame("auth failed").unwrap(),
        Some(Event::AuthFailed)
    );
    assert_eq!(session.state(), SessionState::AuthFailed);
    session.subscribe(Channel::ServerMessage);
    assert_eq!(drain(&mut session), [] as [String; 0]);

    session.authenticate(Token::new("good"));
    assert_eq!(session.state(), SessionState::Authenticating);
    assert_eq!(drain(&mut session), ["auth good"]);
    session.handle_frame("auth ok good").unwrap();
    assert_eq!(
        drain(&mut session),
        ["subscribe user:5a1b/cpu", "subscribe server-message"]
    );
}

#[test]
fn reconnect_resubscribes_after_the_new_auth_ok() {
    let mut session = signed_in();
    session.subscribe(cpu());
    session.subscribe(room("W8N3"));
    session
        .handle_frame(r#"["room:shard3/W8N3",{"objects":{"a":{"_id":"a"}}}]"#)
        .unwrap();
    assert!(session.room(&room("W8N3")).is_some());
    session.subscribe(Channel::ServerMessage);

    session.reconnect(None);
    assert_eq!(session.state(), SessionState::Authenticating);
    assert!(
        session.room(&room("W8N3")).is_none(),
        "room state is per connection"
    );
    assert_eq!(
        drain(&mut session),
        ["gzip on", "auth tok-1"],
        "queued frames are dropped"
    );
    session.handle_frame("auth ok tok-3").unwrap();
    assert_eq!(
        drain(&mut session),
        [
            "subscribe user:5a1b/cpu",
            "subscribe room:shard3/W8N3",
            "subscribe server-message"
        ]
    );

    session.reconnect(Some(Token::new("tok-4")));
    assert_eq!(drain(&mut session), ["gzip on", "auth tok-4"]);
}

#[test]
fn greeting_and_status_lines() {
    let mut session = SocketSession::new(None, KIND);
    let mut handle = |frame| session.handle_frame(frame).unwrap();
    assert_eq!(
        handle("time 1792035138367"),
        Some(Event::Time(1_792_035_138_367))
    );
    assert_eq!(handle("protocol 14"), Some(Event::Protocol(14)));
    assert_eq!(handle("package 247"), Some(Event::Package(247)));
    assert_eq!(handle("server down"), Some(Event::ServerDown));
    assert_eq!(
        handle("cannot subscribe room:W1N1"),
        Some(Event::CannotSubscribe("cannot subscribe room:W1N1".into()))
    );
    assert_eq!(handle(""), None);
}

#[test]
fn unknown_lines_fail_without_touching_state() {
    let mut session = SocketSession::new(Some(Token::new("tok-1")), KIND);
    session.subscribe(cpu());
    for line in ["hello", "time soon", "auth okay", "auth ok two tokens"] {
        let error = session.handle_frame(line).unwrap_err();
        assert!(
            matches!(error, SocketError::UnrecognizedLine(_)),
            "{line}: {error:?}"
        );
    }
    assert_eq!(session.state(), SessionState::Authenticating);
    assert_eq!(session.token().map(Token::as_str), Some("tok-1"));
    assert_eq!(drain(&mut session), ["gzip on", "auth tok-1"]);
}

#[test]
fn compressed_frames_are_inflated() {
    let mut session = signed_in();
    let channel = room("W8N3");
    session.subscribe(channel.clone());
    session.subscribe(Channel::Console { user: USER.into() });
    // zlib (Python `zlib.compress`) of
    // ["room:shard3/W8N3",{"objects":{"a1":{"_id":"a1","type":"source","energy":3000,
    //  "effects":[{"effect":1001}]}},"users":{"3":{"_id":"3","username":"Source Keeper"}},
    //  "flags":null,"info":{"mode":"world"},"decorations":[]}]
    let frame = "gz:eJxFjcEKwjAQRH9F5hwwJRfJLwhePHgoRWKzqZU2K5sWKSX/LmkFL8swzHtbQ5hHm55OvDneThcDtYIfL2qnBLvCVeXeew9bssK0vAkWiWdpCQoUSboF1mitFSiEnazXX4attK5yk7PCnEg2q/lLDfY+urF4r5v3cCZ6k6BAYXBdgo3zMCj0MXBhR/Zl/WEZPLKCp5bFTT3H8rvJzRcCxUXI";
    assert_eq!(
        session.handle_frame(frame).unwrap(),
        Some(Event::Channel {
            channel: channel.clone(),
            payload: Payload::Room
        })
    );
    let state = session.room(&channel).unwrap();
    assert_eq!(
        state.object("a1").and_then(|object| object.get("effects")),
        Some(&json!({"0": {"effect": 1001}}))
    );
    assert_eq!(
        state.user("3").map(|user| user.username.as_str()),
        Some("Source Keeper")
    );
    assert_eq!(state.mode(), Some("world"));

    // zlib of ["user:5a1b/console",{"messages":{"log":["tick"],"results":[]},"shard":"shard3"}]
    let frame = "gz:eJwlxbEKgCAUBdB/ubMQES3vV8TB7KGRJXh1Ev+9obMci06tsvv1WEJ5WbLCDDxK+qiEDOQSIRbtCjecQVX23Aixbhow+XpC/jdM9wFogxtT";
    let Some(Event::Channel {
        channel: Channel::Console { user },
        payload: Payload::Console(console),
    }) = session.handle_frame(frame).unwrap()
    else {
        panic!("not a console event");
    };
    assert_eq!(user, USER);
    assert_eq!(
        console,
        ConsoleEvent::Output {
            log: vec!["tick".to_owned()],
            results: Vec::new(),
            shard: Some("shard3".to_owned()),
        }
    );
    assert!(matches!(
        session.handle_frame("gz:AAAA"),
        Err(SocketError::Gz(_))
    ));
}

#[test]
fn payloads_decode_with_the_contract_of_the_server() {
    let console = Channel::Console { user: USER.into() };
    let room = Channel::Room {
        shard: None,
        room: "W1N1".into(),
    };
    // Console output names the shard on official servers only.
    let output = r#"["user:5a1b/console",{"messages":{"log":[],"results":["1"]},"shard":"s"}]"#;
    // Only the official contract reads `decorations`; this one does not match it.
    let decorated = r#"["room:W1N1",{"objects":{},"decorations":[{"x":1}],"gameTime":5}]"#;
    for kind in [
        ServerKind::Official,
        ServerKind::OpenScreeps,
        ServerKind::Community,
    ] {
        let mut session = signed_in_to(kind);
        assert_eq!(session.kind(), kind);
        session.subscribe(console.clone());
        session.subscribe(room.clone());
        let official = kind == ServerKind::Official;
        assert_eq!(
            session.handle_frame(output).unwrap(),
            Some(Event::Channel {
                channel: console.clone(),
                payload: Payload::Console(ConsoleEvent::Output {
                    log: Vec::new(),
                    results: vec!["1".to_owned()],
                    shard: official.then(|| "s".to_owned()),
                }),
            }),
            "{kind:?}"
        );
        let decoded = session.handle_frame(decorated);
        if official {
            assert!(
                matches!(decoded, Err(SocketError::Payload { .. })),
                "{decoded:?}"
            );
        } else {
            assert!(matches!(decoded, Ok(Some(_))), "{kind:?}: {decoded:?}");
            let state = session.room(&room).unwrap();
            assert_eq!(state.game_time(), Some(5));
            assert_eq!(state.decorations(), []);
        }
    }
}

#[test]
fn room_errors_route_to_the_subscription_which_stays() {
    let mut session = signed_in();
    let channel = Channel::Room {
        shard: None,
        room: "W8N3".into(),
    };
    session.subscribe(channel.clone());
    assert_eq!(
        session
            .handle_frame(r#"["err@room:W8N3","subscribe limit reached"]"#)
            .unwrap(),
        Some(Event::ChannelError {
            channel: channel.clone(),
            message: "subscribe limit reached".into()
        })
    );
    assert_eq!(session.subscriptions().collect::<Vec<_>>(), [&channel]);
    assert!(session.room(&channel).is_none());
}

#[test]
fn events_of_unsubscribed_channels_are_unsolicited() {
    let mut session = signed_in();
    session.subscribe(cpu());
    session.unsubscribe(&cpu());
    for (frame, name) in [
        (r#"["user:5a1b/cpu",{"cpu":3}]"#, "user:5a1b/cpu"),
        (
            r#"["err@room:shard3/W1N1","subscribe limit reached"]"#,
            "err@room:shard3/W1N1",
        ),
        (
            r#"["user:5a1b/steam-purchase",{}]"#,
            "user:5a1b/steam-purchase",
        ),
    ] {
        assert_eq!(
            session.handle_frame(frame).unwrap(),
            Some(Event::Unsolicited {
                channel: name.into()
            })
        );
    }
}

#[test]
fn channel_payloads_are_typed_and_bad_data_is_a_payload_error() {
    let mut session = signed_in();
    session.subscribe(cpu());
    session.subscribe(Channel::ServerMessage);
    let room = room("W8N3");
    session.subscribe(room.clone());
    assert_eq!(
        session
            .handle_frame(r#"["user:5a1b/cpu",{"cpu":"error","memory":754}]"#)
            .unwrap(),
        Some(Event::Channel {
            channel: cpu(),
            payload: Payload::Cpu(UserCpu {
                cpu: UserCpuCpu::Error,
                memory: Some(754)
            }),
        })
    );
    assert_eq!(
        session
            .handle_frame(r#"["server-message","Maintenance at 10:00"]"#)
            .unwrap(),
        Some(Event::Channel {
            channel: Channel::ServerMessage,
            payload: Payload::ServerMessage("Maintenance at 10:00".into()),
        })
    );

    let map_visual = Channel::MapVisual {
        user: USER.into(),
        shard: Some("shard3".into()),
    };
    session.subscribe(map_visual.clone());
    let frame = r#"["mapVisual:5a1b/shard3","{\"t\":\"c\",\"x\":25,\"y\":25,\"n\":\"W8N3\",\"s\":{\"radius\":5}}\n{\"t\":\"t\",\"text\":\"base\",\"x\":25,\"y\":20,\"n\":\"W8N3\",\"s\":{}}\n"]"#;
    let Some(Event::Channel {
        payload: Payload::MapVisual(items),
        ..
    }) = session.handle_frame(frame).unwrap()
    else {
        panic!("not a map visual event");
    };
    assert!(
        matches!(&items[..], [MapVisualItem::Circle(circle), MapVisualItem::Text(text)]
            if circle.n == "W8N3" && text.text == "base"),
        "{items:?}"
    );
    assert_eq!(
        session
            .handle_frame(r#"["mapVisual:5a1b/shard3",null]"#)
            .unwrap(),
        Some(Event::Channel {
            channel: map_visual.clone(),
            payload: Payload::MapVisual(Vec::new()),
        })
    );
    assert!(matches!(
        session.handle_frame(r#"["mapVisual:5a1b/shard3","{\"t\":\"x\"}"]"#),
        Err(SocketError::Payload { .. })
    ));

    session
        .handle_frame(r#"["room:shard3/W8N3",{"objects":{"a":{"_id":"a","energy":1}}}]"#)
        .unwrap();
    let error = session
        .handle_frame(r#"["room:shard3/W8N3",{"objects":{"a":{"energy":2}},"gameTime":"soon"}]"#)
        .unwrap_err();
    assert!(matches!(&error, SocketError::Payload { channel, .. } if *channel == room));
    assert_eq!(
        session
            .room(&room)
            .unwrap()
            .object("a")
            .unwrap()
            .get("energy"),
        Some(&json!(1)),
        "a rejected update is not applied"
    );
    assert!(matches!(
        session.handle_frame(r#"["user:5a1b/cpu",{"cpu":true}]"#),
        Err(SocketError::Payload { .. })
    ));
    for frame in [
        "[1,2]",
        r#"["server-message"]"#,
        r#"["server-message","a","b"]"#,
        "[broken",
    ] {
        assert!(
            matches!(session.handle_frame(frame), Err(SocketError::Json(_))),
            "{frame}"
        );
    }
}
