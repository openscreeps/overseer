//! The tick speed of a shard ([`overseer::Map::tick_speed`]): the server's fresh duration
//! first, else the duration measured from live room updates or from polls of the game time.

use std::time::Duration;

use futures_util::StreamExt;
use overseer::map::{TickSource, TickSpeed};

use crate::server::{self, Request, Server, Ws};
use crate::{PATIENCE, connect, receive, routes, soon};

/// The requests of `path` served so far.
fn served(server: &Server, path: &str) -> usize {
    server
        .requests()
        .iter()
        .filter(|request| request.path == path)
        .count()
}

/// Reads `websocket` in the background, so keep-alive pings are answered.
fn answer_pings(mut websocket: Ws) {
    tokio::spawn(async move { while websocket.next().await.is_some() {} });
}

/// Runs the other tasks, without moving the paused clock, until `done` holds.
async fn settle(mut done: impl FnMut() -> bool) {
    let start = std::time::Instant::now();
    while !done() {
        assert!(start.elapsed() < PATIENCE, "settled in time");
        tokio::task::yield_now().await;
    }
}

/// Lets the other tasks react to what just happened (and answer any request started) for
/// `real` time, without moving the paused clock.
async fn run(real: Duration) {
    let start = std::time::Instant::now();
    while start.elapsed() < real {
        tokio::task::yield_now().await;
    }
}

/// Moves the paused clock by `duration` once the other tasks have reacted to what happened
/// before, then lets them react to the new time: a runtime that never idles never advances
/// paused time on its own.
async fn advance(duration: Duration) {
    run(Duration::from_millis(200)).await;
    tokio::time::advance(duration).await;
    run(Duration::from_millis(200)).await;
}

/// Moves the paused clock a second at a time until `done` holds, at most `limit` seconds:
/// the seconds it took.
async fn advance_until(mut done: impl FnMut() -> bool, limit: u64) -> u64 {
    for second in 0..=limit {
        if done() {
            return second;
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        run(Duration::from_millis(50)).await;
    }
    panic!("not done within {limit} s");
}

#[tokio::test]
async fn the_servers_duration_wins_while_fresh_then_polled_game_time_measures_it() {
    let start = tokio::time::Instant::now();
    // The server knows its duration; its game time advances a tick a second (of the paused
    // clock).
    let mut server = Server::start(
        move |request: &Request| match request.path.as_str() {
            "/api/game/tick" => (200, r#"{"ok":1,"tick":3150.5}"#.to_owned()),
            "/api/game/time" => {
                let time = 1000 + start.elapsed().as_secs();
                (200, format!(r#"{{"ok":1,"time":{time}}}"#))
            }
            _ => routes(request),
        },
        |_| true,
    )
    .await;
    let (overseer, websocket) = connect(&mut server).await;
    answer_pings(websocket);
    tokio::time::pause();

    let watch = overseer.map().tick_speed(None).unwrap();
    // Another consumer shares the state and the requests.
    let other = overseer.map().tick_speed(Some("shard9")).unwrap();
    assert_eq!(other.shard(), None, "single-world servers have no shards");
    let reported = Some(TickSpeed {
        ms: 3150.5,
        source: TickSource::Server,
    });
    settle(|| watch.latest() == reported).await;
    assert_eq!(other.latest(), reported);
    assert_eq!(served(&server, "/api/game/tick"), 1);
    // The next poll, 30 s later, gets no answer: the duration stays fresh for 90 s after the
    // reply, and no game time is asked for meanwhile.
    server.hold(Some("/api/game/tick"));
    advance(Duration::from_secs(85)).await;
    assert_eq!(watch.latest(), reported);
    assert_eq!(served(&server, "/api/game/time"), 0);
    let fresh = 85 + advance_until(|| watch.latest() != reported, 30).await;
    assert!((90..=91).contains(&fresh), "fresh for {fresh} s");
    assert_eq!(served(&server, "/api/game/tick"), 2);
    assert_eq!(watch.latest(), None);
    // Then the game time is polled every 15 s (on the polls' own beat, which started at the
    // first follower), and two intervals measure it: three polls, within three periods.
    let polls = || served(&server, "/api/game/time");
    assert_eq!(polls(), 0);
    let measured = |speed: Option<TickSpeed>| {
        speed.is_some_and(|speed| {
            speed.source == TickSource::Measured && (speed.ms - 1000.0).abs() < 1.0
        })
    };
    let measuring = advance_until(|| measured(watch.latest()), 60).await;
    assert!(measuring <= 48, "measured after {measuring} s");
    assert_eq!(polls(), 3);
    assert_eq!(other.latest(), watch.latest());
    // The last handle dropped stops the requests.
    drop((watch, other));
    let requests = server.requests().len();
    advance(Duration::from_secs(60)).await;
    assert_eq!(server.requests().len(), requests);
}

#[tokio::test]
async fn live_room_updates_measure_the_tick_when_the_server_does_not_know_it() {
    let mut server = Server::start(
        |request: &Request| match request.path.as_str() {
            "/api/game/tick" => (200, r#"{"ok":1,"tick":null}"#.to_owned()),
            _ => routes(request),
        },
        |_| true,
    )
    .await;
    let (overseer, mut websocket) = connect(&mut server).await;
    let mut watch = overseer.map().tick_speed(None).unwrap();
    // Any live room of the shard counts, here one outside the map.
    let _room = overseer.rooms().watch("W1N1").await.unwrap();
    assert_eq!(receive(&mut websocket, 1).await, ["subscribe room:W1N1"]);
    for tick in 1..=4 {
        let update = format!(r#"["room:W1N1",{{"gameTime":{tick},"objects":{{}}}}]"#);
        server::send(&mut websocket, &update).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let measured = soon(async {
        loop {
            if let Some(speed) = watch.changed().await.unwrap() {
                break speed;
            }
        }
    })
    .await;
    assert_eq!(measured.source, TickSource::Measured);
    assert!((150.0..=600.0).contains(&measured.ms), "{}", measured.ms);
    assert_eq!(served(&server, "/api/game/time"), 0, "live updates arrive");
    // A closed session ends the watch.
    overseer.shutdown().await;
    assert!(soon(watch.changed()).await.is_err());
}
