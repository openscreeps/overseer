//! Private messages: request shapes and the message feeds.

use overseer::core::api::types::UserMessageType;
use overseer::core::socket::{self as socket_types, Channel};

use crate::server::{self, Request, Server};
use crate::{connect, receive, routes, soon};

/// The message routes over [`routes`].
fn message_routes(request: &Request) -> (u16, String) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/api/user/messages/index") => (
            200,
            r#"{"ok":1,"messages":[{"_id":"u2","message":{"_id":"m2","date":"2026-10-15T00:00:00.000Z","respondent":"u2","text":"hi","type":"in","unread":true,"user":"u1"}}],"users":{"u2":{"_id":"u2","username":"Bob"}}}"#
                .to_owned(),
        ),
        ("GET", "/api/user/messages/list") => (
            200,
            r#"{"ok":1,"messages":[{"_id":"m1","date":"2026-10-14T00:00:00.000Z","respondent":"u2","text":"hello","type":"out","unread":false,"user":"u1"},{"_id":"m2","date":"2026-10-15T00:00:00.000Z","respondent":"u2","text":"hi","type":"in","unread":true,"user":"u1"}]}"#
                .to_owned(),
        ),
        ("GET", "/api/user/messages/unread-count") => (200, r#"{"ok":1,"count":3}"#.to_owned()),
        ("POST", "/api/user/messages/send") => (200, r#"{"ok":1}"#.to_owned()),
        ("POST", "/api/user/messages/mark-read") => (200, r#"{"ok":1,"0":1,"1":1}"#.to_owned()),
        _ => routes(request),
    }
}

#[tokio::test]
async fn message_requests_have_the_documented_shape() {
    let mut server = Server::start(message_routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let messages = overseer.messages();

    let index = messages.conversations().await.unwrap();
    assert_eq!(index.messages.len(), 1);
    assert_eq!(index.messages[0].message.text, "hi");
    assert_eq!(index.users["u2"].username.as_deref(), Some("Bob"));
    let conversation = messages.conversation("u2").await.unwrap();
    let kinds: Vec<_> = conversation.iter().map(|message| message.r#type).collect();
    assert_eq!(kinds, [UserMessageType::Out, UserMessageType::In]);
    assert_eq!(messages.unread_count().await.unwrap(), 3);
    messages.send("u2", "a \"quoted\" reply").await.unwrap();
    messages.mark_read("m2").await.unwrap();

    let sent: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.path.starts_with("/api/user/messages/"))
        .map(|request| (request.method, request.path, request.query, request.body))
        .collect();
    let expected = [
        ("GET", "index", "", ""),
        ("GET", "list", "respondent=u2", ""),
        ("GET", "unread-count", "", ""),
        (
            "POST",
            "send",
            "",
            r#"{"respondent":"u2","text":"a \"quoted\" reply"}"#,
        ),
        ("POST", "mark-read", "", r#"{"id":"m2"}"#),
    ]
    .map(|(method, path, query, body)| {
        (
            method.to_owned(),
            format!("/api/user/messages/{path}"),
            query.to_owned(),
            body.to_owned(),
        )
    });
    assert_eq!(sent, expected);
}

#[tokio::test]
async fn new_and_conversation_messages_reach_their_feeds() {
    let mut server = Server::start(message_routes, |_| true).await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let messages = overseer.messages();

    let mut incoming = messages.feed().unwrap();
    assert_eq!(
        *incoming.channel(),
        Channel::NewMessage {
            user: "u1".to_owned()
        }
    );
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe user:u1/newMessage"]
    );
    let mut conversation = messages.conversation_feed("u2").unwrap();
    let mut same = messages.conversation_feed("u2").unwrap();
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe user:u1/message:u2"]
    );

    server::send(
        &mut websocket,
        r#"["user:u1/newMessage",{"message":{"_id":"m3","date":"2026-10-15T01:00:00.000Z","outMessage":"m4","respondent":"u2","text":"ping","type":"in","unread":true,"user":"u1"}}]"#,
    )
    .await;
    server::send(
        &mut websocket,
        r#"["user:u1/message:u2",{"message":{"_id":"m3","date":"2026-10-15T01:00:00.000Z","outMessage":"m4","respondent":"u2","text":"ping","type":"in","unread":true,"user":"u1"}}]"#,
    )
    .await;
    server::send(
        &mut websocket,
        r#"["user:u1/message:u2",{"message":{"_id":"m3","unread":false}}]"#,
    )
    .await;

    let received = soon(incoming.recv()).await.unwrap();
    assert_eq!(received.id, "m3");
    assert_eq!(received.text.as_deref(), Some("ping"));
    assert_eq!(received.r#type, Some(socket_types::UserMessageType::In));
    assert_eq!(received.out_message.as_deref(), Some("m4"));
    assert_eq!(soon(conversation.recv()).await.unwrap(), received);
    let read = socket_types::UserMessage {
        id: "m3".to_owned(),
        unread: Some(false),
        ..socket_types::UserMessage::default()
    };
    assert_eq!(soon(conversation.recv()).await.unwrap(), read);
    // Each handle of the channel receives every item.
    assert_eq!(soon(same.recv()).await.unwrap(), received);
    assert_eq!(soon(same.recv()).await.unwrap(), read);

    drop(conversation);
    drop(same);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe user:u1/message:u2"]
    );
    drop(incoming);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe user:u1/newMessage"]
    );
}
