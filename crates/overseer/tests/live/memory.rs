//! Live tests: Memory by path, Memory segments and live path watches.

use std::time::Duration;

use overseer::core::api::{ApiError, MemoryValue};
use overseer::{Error, Memory, Watch, net};
use serde_json::{Value, json};
use tokio::time::{sleep, timeout};

use crate::support::{self, PATIENCE};

/// The pause between two reads of a value written on a later tick.
const POLL: Duration = Duration::from_millis(100);

/// The value at `path` once `wanted` accepts it (writes apply on a later tick).
async fn eventually(
    memory: &Memory,
    path: &str,
    wanted: impl Fn(&MemoryValue) -> bool,
) -> MemoryValue {
    timeout(PATIENCE, async {
        loop {
            let value = memory.get(path, None).await.expect("Memory reads");
            if wanted(&value) {
                break value;
            }
            sleep(POLL).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Memory.{path} did not reach the expected value in time"))
}

/// `Memory.ticks`, counted by the bot.
fn ticks(value: &MemoryValue) -> Option<u64> {
    match value {
        MemoryValue::Json(Value::Number(number)) => number.as_u64(),
        _ => None,
    }
}

/// The next value of `watch` that `wanted` accepts; fails the test naming `what` otherwise.
async fn watched(what: &str, watch: &mut Watch<String>, wanted: impl Fn(&str) -> bool) -> String {
    timeout(PATIENCE, async {
        loop {
            let value = watch.changed().await.expect("the session is open");
            if wanted(&value) {
                break value.as_str().to_owned();
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} did not arrive in time"))
}

/// The bot's `Memory.ticks` grows, read by path and as part of all of Memory.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn ticks_increase() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let memory = overseer.memory();

    let first = eventually(&memory, "ticks", |value| ticks(value).is_some()).await;
    let first = ticks(&first).expect("a count");
    let later = eventually(&memory, "ticks", |value| {
        ticks(value).is_some_and(|count| count > first)
    })
    .await;
    assert!(ticks(&later).is_some_and(|count| count > first));

    let MemoryValue::Json(all) = memory.get("", None).await.expect("all of Memory") else {
        panic!("Memory is an object");
    };
    assert!(
        all.get("ticks")
            .and_then(Value::as_u64)
            .is_some_and(|count| count > first),
        "{all}"
    );
}

/// A value set at a path appears on a later tick, deeper paths read into it, and deleting it
/// removes it; unreachable and malformed paths are reported as the server does.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn set_and_delete_a_path() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let memory = overseer.memory();
    let value = json!({"answer": 42, "name": "live", "list": [1, "two", null]});

    memory
        .set("live.data", value.clone(), None)
        .await
        .expect("the write is queued");
    eventually(&memory, "live.data", |read| {
        *read == MemoryValue::Json(value.clone())
    })
    .await;
    assert_eq!(
        memory
            .get("live.data.answer", None)
            .await
            .expect("Memory reads"),
        MemoryValue::Json(json!(42))
    );
    assert_eq!(
        memory
            .get("live.missing", None)
            .await
            .expect("Memory reads"),
        MemoryValue::Undefined
    );
    assert_eq!(
        memory
            .get("live.missing.deeper", None)
            .await
            .expect("Memory reads"),
        MemoryValue::IncorrectPath
    );

    memory
        .delete("live.data", None)
        .await
        .expect("the deletion is queued");
    eventually(&memory, "live.data", |read| *read == MemoryValue::Undefined).await;
    assert_eq!(
        memory.get("live", None).await.expect("Memory reads"),
        MemoryValue::Json(json!({}))
    );

    let error = memory
        .set("live..data", json!(1), None)
        .await
        .expect_err("an empty path part is refused");
    assert!(
        matches!(&error, Error::Net(net::Error::Api(ApiError::Server(message))) if message == "Incorrect memory path"),
        "unexpected error: {error:?}"
    );
}

/// Segments are written and read back; unwritten segments are empty and ids above 99 are
/// refused before anything is sent.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn segments_round_trip() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let memory = overseer.memory();
    let data = format!("{{\"live\":\"{}\"}}", support::unique("s"));

    memory
        .set_segment(7, &data, None)
        .await
        .expect("the segment is written");
    assert_eq!(
        memory.segment(7, None).await.expect("the segment"),
        Some(data)
    );
    assert_eq!(memory.segment(8, None).await.expect("the segment"), None);
    memory
        .set_segment(7, "", None)
        .await
        .expect("the segment is cleared");
    assert_eq!(
        memory.segment(7, None).await.expect("the segment"),
        Some(String::new())
    );
    assert!(matches!(
        memory.segment(100, None).await,
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        memory.set_segment(100, "x", None).await,
        Err(Error::InvalidInput(_))
    ));
}

/// Watched paths report their JavaScript string form after every run: the growing tick count,
/// `undefined` for an absent value, the server's text for a path through an absent value, and
/// values set later (`[object Object]` for objects).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live OpenScreeps server: cargo xtask e2e"]
async fn watches_paths() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let memory = overseer.memory();
    let mut count = memory.watch("ticks", None).expect("the watch");
    let mut parent = memory.watch("watched", None).expect("the watch");
    let mut flag = memory.watch("watched.flag", None).expect("the watch");

    let first: u64 = watched("a count", &mut count, |value| value.parse::<u64>().is_ok())
        .await
        .parse()
        .expect("a count");
    watched("a later count", &mut count, |value| {
        value.parse::<u64>().is_ok_and(|later| later > first)
    })
    .await;
    assert_eq!(
        watched("the absent value", &mut parent, |_| true).await,
        "undefined"
    );
    assert_eq!(
        watched("the path through it", &mut flag, |_| true).await,
        "Incorrect memory path"
    );

    memory
        .set("watched.flag", json!("on"), None)
        .await
        .expect("the write is queued");
    watched("the set flag", &mut flag, |value| value == "on").await;
    watched("the new object", &mut parent, |value| {
        value == "[object Object]"
    })
    .await;
    assert!(matches!(
        memory.watch("", None),
        Err(Error::InvalidInput(_))
    ));
}
