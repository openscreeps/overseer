//! The console feed, its history, commands and lag.

use overseer::core::socket::Channel;
use overseer::live::FEED_CAPACITY;
use overseer::{Closed, ConsoleKind, Error, Notice};

use crate::server::{self, Server};
use crate::{connect, receive, routes, soon};

#[tokio::test]
async fn commands_and_output_reach_the_feed_and_the_history() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let console = overseer.console();
    let mut feed = console.feed().unwrap();
    assert_eq!(
        *feed.channel(),
        Channel::Console {
            user: "u1".to_owned()
        }
    );
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe user:u1/console"]
    );

    console.execute("Game.time", None).await.unwrap();
    console.execute("Memory.x", None).await.unwrap();
    console.execute("Game.time", None).await.unwrap();
    assert!(matches!(
        console.execute("  ", None).await,
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(console.recent_commands(), ["Memory.x", "Game.time"]);
    let posted: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.method == "POST" && request.path == "/api/user/console")
        .map(|request| request.body)
        .collect();
    assert_eq!(
        posted,
        [
            r#"{"expression":"Game.time"}"#,
            r#"{"expression":"Memory.x"}"#,
            r#"{"expression":"Game.time"}"#
        ]
    );

    server::send(
        &mut websocket,
        r#"["user:u1/console",{"messages":{"log":["<b>tick</b> &amp; more"],"results":["42"]}}]"#,
    )
    .await;
    server::send(
        &mut websocket,
        r#"["user:u1/console",{"error":"Error: boom"}]"#,
    )
    .await;
    let mut lines = Vec::new();
    for _ in 0..6 {
        let entry = soon(feed.recv()).await.unwrap();
        lines.push((entry.kind, entry.plain_text()));
    }
    let expected = [
        (ConsoleKind::Command, "Game.time"),
        (ConsoleKind::Command, "Memory.x"),
        (ConsoleKind::Command, "Game.time"),
        (ConsoleKind::Log, "tick & more"),
        (ConsoleKind::Result, "42"),
        (ConsoleKind::Error, "Error: boom"),
    ]
    .map(|(kind, text)| (kind, text.to_owned()));
    assert_eq!(lines, expected);
    let history: Vec<_> = console
        .history()
        .into_iter()
        .map(|entry| (entry.kind, entry.plain_text()))
        .collect();
    assert_eq!(history, expected);

    // The history lasts while a console feed exists.
    drop(feed);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe user:u1/console"]
    );
    assert_eq!(console.history().len(), 0);

    overseer.shutdown().await;
    assert!(matches!(console.feed(), Err(Error::Closed)));
    assert!(matches!(
        console.execute("Game.time", None).await,
        Err(Error::Closed)
    ));
}

#[tokio::test]
async fn a_slow_feed_skips_lost_lines_with_a_notice() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let mut notices = overseer.notices();
    let mut feed = overseer.console().feed().unwrap();
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["subscribe user:u1/console"]
    );
    let lost = 44;
    for index in 0..FEED_CAPACITY + lost {
        server::send(
            &mut websocket,
            &format!(r#"["user:u1/console",{{"messages":{{"log":["{index}"],"results":[]}}}}]"#),
        )
        .await;
    }
    // Frames are handled in order: once the announcement is out, every line was delivered.
    server::send(&mut websocket, r#"["server-message","done"]"#).await;
    assert_eq!(
        soon(notices.recv()).await.unwrap(),
        Notice::ServerMessage("done".to_owned())
    );

    let first = soon(feed.recv()).await.unwrap();
    assert_eq!(first.text, lost.to_string());
    assert_eq!(
        soon(notices.recv()).await.unwrap(),
        Notice::Lagged {
            channel: feed.channel().clone(),
            skipped: u64::try_from(lost).unwrap()
        }
    );
    // The history keeps every line.
    assert_eq!(overseer.console().history().len(), FEED_CAPACITY + lost);

    overseer.shutdown().await;
    let mut remaining = 0;
    while feed.recv().await.is_ok() {
        remaining += 1;
    }
    assert_eq!(remaining, FEED_CAPACITY - 1);
    assert_eq!(feed.recv().await, Err(Closed));
}
