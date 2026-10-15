//! History playback over fixture chunks: seeking, stepping, chunk caching and missing chunks.

use overseer::{Error, HistoryPlayer, Seek};

use crate::server::{Request, Server};
use crate::{connect, query, soon, terrain_document};

/// A marker object (typed as `Other`) of `room`.
fn marker(id: &str, hits: u64) -> String {
    format!(r#"{{"_id":"{id}","type":"marker","x":1,"y":1,"room":"W1N1","hits":{hits}}}"#)
}

/// Chunks of 20 ticks for W1N1: none at 0 (HTTP 404); at 20 `a` appears with 9 hits, drops
/// to 7 at 22 and `b` appears at 25; every later chunk sets `a`'s hits to its base.
fn routes(request: &Request) -> (u16, String) {
    match request.path.as_str() {
        "/api/game/room-terrain" => (
            200,
            format!(r#"{{"ok":1,"terrain":[{}]}}"#, terrain_document("W1N1")),
        ),
        "/room-history" => match query(request, "time").and_then(|time| time.parse().ok()) {
            Some(20_u64) => (
                200,
                format!(
                    r#"{{"base":20,"timestamp":1,"ticks":{{"20":{{"a":{}}},"22":{{"a":{{"hits":7}}}},"25":{{"b":{}}}}}}}"#,
                    marker("a", 9),
                    marker("b", 1)
                ),
            ),
            Some(base) if base > 20 => (
                200,
                format!(
                    r#"{{"base":{base},"timestamp":1,"ticks":{{"{base}":{{"a":{}}}}}}}"#,
                    marker("a", base)
                ),
            ),
            _ => (404, r#"{"error":"not found"}"#.to_owned()),
        },
        _ => (401, r#"{"error":"unauthorized"}"#.to_owned()),
    }
}

/// The `time` of every chunk request so far.
fn chunk_requests(server: &Server) -> Vec<String> {
    server
        .requests()
        .iter()
        .filter(|request| request.path == "/room-history")
        .map(|request| query(request, "time").unwrap_or_default().to_owned())
        .collect()
}

/// `(hits of a, whether b exists)` at the player's tick.
fn room(player: &HistoryPlayer) -> (u64, bool) {
    let state = player.state().unwrap();
    let hits = state.object("a").unwrap()["hits"].as_u64().unwrap();
    (hits, state.object("b").is_some())
}

#[tokio::test]
async fn seek_and_step_replay_chunks_from_their_base() {
    let (server, overseer, _websocket) = connect(routes).await;
    let mut player = overseer.history().player("W1N1", None).unwrap();
    assert_eq!(player.tick(), None);
    let error = player.step().await.unwrap_err();
    assert!(matches!(error, Error::InvalidInput(_)), "{error:?}");

    assert_eq!(soon(player.seek(23)).await.unwrap(), Seek::Moved);
    assert_eq!((player.tick(), room(&player)), (Some(23), (7, false)));
    let snapshot = player.snapshot().unwrap();
    assert_eq!(snapshot.game_time, Some(23));
    assert_eq!(snapshot.room, "W1N1");
    assert_eq!(snapshot.objects.len() + snapshot.undecoded.len(), 1);

    assert_eq!(player.step().await.unwrap(), Seek::Moved);
    assert_eq!((player.tick(), room(&player)), (Some(24), (7, false)));
    player.step().await.unwrap();
    assert_eq!((player.tick(), room(&player)), (Some(25), (7, true)));
    // Stepping back replays the chunk from its base.
    player.step_back().await.unwrap();
    assert_eq!((player.tick(), room(&player)), (Some(24), (7, false)));
    player.seek(21).await.unwrap();
    assert_eq!((player.tick(), room(&player)), (Some(21), (9, false)));

    // Stepping off a chunk's end loads the next chunk.
    player.seek(39).await.unwrap();
    assert_eq!(room(&player), (7, true));
    player.step().await.unwrap();
    assert_eq!((player.tick(), room(&player)), (Some(40), (40, false)));
    player.step_back().await.unwrap();
    assert_eq!((player.tick(), room(&player)), (Some(39), (7, true)));

    // Each chunk was fetched once; the terrain once.
    assert_eq!(chunk_requests(&server), ["20", "40"]);
    let terrain = server
        .requests()
        .iter()
        .filter(|request| request.path == "/api/game/room-terrain")
        .count();
    assert_eq!(terrain, 1);
}

#[tokio::test]
async fn recent_chunks_are_cached_and_older_ones_fetched_again() {
    let (server, overseer, _websocket) = connect(routes).await;
    let mut player = overseer.history().player("W1N1", None).unwrap();
    for tick in [20, 45, 65, 85, 25, 105, 30] {
        assert_eq!(player.seek(tick).await.unwrap(), Seek::Moved);
    }
    // Four chunks are kept: 20 was used again before 105 evicted the least recently used (40).
    assert_eq!(chunk_requests(&server), ["20", "40", "60", "80", "100"]);
    player.seek(41).await.unwrap();
    assert_eq!(room(&player), (40, false));
    assert_eq!(
        chunk_requests(&server),
        ["20", "40", "60", "80", "100", "40"]
    );
}

#[tokio::test]
async fn chunks_that_are_not_kept_leave_the_player_in_place() {
    let (server, overseer, _websocket) = connect(routes).await;
    let mut player = overseer.history().player("W1N1", None).unwrap();
    assert_eq!(player.seek(5).await.unwrap(), Seek::NotKept);
    assert_eq!(player.tick(), None);
    player.seek(20).await.unwrap();
    assert_eq!(player.step_back().await.unwrap(), Seek::NotKept);
    assert_eq!((player.tick(), room(&player)), (Some(20), (9, false)));
    // A missing chunk is asked for again: it may be written later.
    player.seek(19).await.unwrap();
    assert_eq!(chunk_requests(&server), ["0", "20", "0", "0"]);
}
