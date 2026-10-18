//! Memory paths, segments and Memory watches.

use overseer::Error;
use overseer::core::socket::Channel;
use overseer::memory::{MemoryPath, MemoryValue, SEGMENT_MAX_UNITS, SegmentError};
use serde_json::json;

use crate::server::{self, Server};
use crate::{bodies, connect, queries, receive, routes, soon};

fn path(text: &str) -> MemoryPath {
    MemoryPath::parse(text).unwrap()
}

#[tokio::test]
async fn memory_and_segments_are_read_and_written() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, _websocket) = connect(&mut server).await;
    let memory = overseer.memory();

    assert_eq!(
        memory.get(&path("creeps.h1"), None).await.unwrap(),
        MemoryValue::Json(json!({ "role": "harvester" }))
    );
    memory.get(&MemoryPath::root(), None).await.unwrap();
    memory
        .set(&path("creeps.h1.role"), json!("builder"), None)
        .await
        .unwrap();
    memory.delete(&path("creeps.h1"), None).await.unwrap();
    assert_eq!(queries(&server, "/api/user/memory"), ["path=creeps.h1", ""]);
    assert_eq!(
        bodies(&server, "POST", "/api/user/memory"),
        [
            json!({ "path": "creeps.h1.role", "value": "builder" }),
            json!({ "path": "creeps.h1" }),
        ]
    );

    assert_eq!(
        memory.segment(7, None).await.unwrap().as_deref(),
        Some("segment 7")
    );
    memory.set_segment(99, "data", None).await.unwrap();
    assert!(matches!(
        memory.segment(100, None).await,
        Err(Error::Segment(SegmentError::Id(100)))
    ));
    assert!(matches!(
        memory.set_segment(100, "", None).await,
        Err(Error::Segment(SegmentError::Id(100)))
    ));
    // Data over the limit is not sent; the limit counts UTF-16 units.
    assert!(matches!(
        memory
            .set_segment(5, &"😀".repeat(SEGMENT_MAX_UNITS / 2 + 1), None)
            .await,
        Err(Error::Segment(SegmentError::TooLong { id: 5, .. }))
    ));
    assert_eq!(queries(&server, "/api/user/memory-segment"), ["segment=7"]);
    assert_eq!(
        bodies(&server, "POST", "/api/user/memory-segment"),
        [json!({ "segment": 99, "data": "data" })]
    );
}

#[tokio::test]
async fn watches_of_a_memory_path_share_one_subscription() {
    let mut server = Server::start(routes, |_| true).await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let memory = overseer.memory();

    let mut first = memory.watch(&path("stats.tick"), None).unwrap();
    let second = memory.watch(&path("stats.tick"), None).unwrap();
    let other = memory.watch(&path("stats.cpu"), None).unwrap();
    assert_eq!(
        *first.channel(),
        Channel::Memory {
            user: "u1".to_owned(),
            shard: None,
            path: "stats.tick".to_owned(),
        }
    );
    assert_eq!(
        receive(&mut websocket, 2).await,
        [
            "subscribe user:u1/memory/stats.tick",
            "subscribe user:u1/memory/stats.cpu"
        ]
    );

    server::send(&mut websocket, r#"["user:u1/memory/stats.tick","42"]"#).await;
    assert_eq!(*soon(first.changed()).await.unwrap(), "42");
    // A new watch of the path starts with the latest value.
    let mut third = memory.watch(&path("stats.tick"), None).unwrap();
    assert_eq!(*soon(third.changed()).await.unwrap(), "42");
    assert_eq!(third.latest().as_deref().map(String::as_str), Some("42"));

    drop(other);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe user:u1/memory/stats.cpu"]
    );
    drop(first);
    drop(second);
    drop(third);
    assert_eq!(
        receive(&mut websocket, 1).await,
        ["unsubscribe user:u1/memory/stats.tick"]
    );

    assert!(matches!(
        memory.watch(&MemoryPath::root(), None),
        Err(Error::InvalidInput(_))
    ));
    overseer.shutdown().await;
    assert!(matches!(
        memory.watch(&path("stats.tick"), None),
        Err(Error::Closed)
    ));
}
