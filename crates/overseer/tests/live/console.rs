//! Live tests: the console feed, console expressions and command recall.

use overseer::{ConsoleEntry, ConsoleKind, Error, Feed};
use tokio::time::timeout;

use crate::support::{self, PATIENCE};

/// The first entry of `feed` that `wanted` accepts; fails the test naming `what` otherwise.
async fn next(
    what: &str,
    feed: &mut Feed<ConsoleEntry>,
    wanted: impl Fn(&ConsoleEntry) -> bool,
) -> ConsoleEntry {
    timeout(PATIENCE, async {
        loop {
            let entry = feed.recv().await.expect("the session is open");
            if wanted(&entry) {
                break entry;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} did not arrive in time"))
}

/// The bot's `console.log` lines arrive on the feed, one per tick, and are kept in the
/// history.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn bot_logs_arrive() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let console = overseer.console();
    let mut feed = console.feed().expect("the console feed");

    let tick = |entry: &ConsoleEntry| {
        if entry.kind != ConsoleKind::Log {
            return None;
        }
        entry
            .plain_text()
            .strip_prefix("tick ")
            .and_then(|time| time.parse::<u64>().ok())
    };
    let first = next("a tick line", &mut feed, |entry| tick(entry).is_some()).await;
    assert_eq!(first.shard, None, "single-world servers name no shard");
    let first = tick(&first).expect("a tick line");
    let later = next("a later tick line", &mut feed, |entry| {
        tick(entry).is_some_and(|time| time > first)
    })
    .await;
    assert!(tick(&later).is_some_and(|time| time > first));
    assert!(
        console
            .history()
            .iter()
            .filter(|entry| tick(entry).is_some())
            .count()
            >= 2,
        "the history keeps the lines"
    );
}

/// An expression runs in the player's runtime: the command is echoed and its result arrives on
/// the feed as HTML (results are not escaped), plain text after decoding; commands are recalled.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn expression_result_arrives() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let console = overseer.console();
    let mut feed = console.feed().expect("the console feed");
    support::subscribed(&overseer).await;

    console
        .execute("1+1", None)
        .await
        .expect("the expression is accepted");
    let command = next("the command", &mut feed, |entry| {
        entry.kind == ConsoleKind::Command
    })
    .await;
    assert_eq!(command.text, "1+1");
    let result = next("the result of 1+1", &mut feed, |entry| {
        entry.kind == ConsoleKind::Result
    })
    .await;
    assert_eq!(result.plain_text(), "2");

    let html = "'<b>time</b> &lt; ' + Game.time";
    console
        .execute(html, None)
        .await
        .expect("the expression is accepted");
    let result = next("the HTML result", &mut feed, |entry| {
        entry.kind == ConsoleKind::Result
    })
    .await;
    assert!(result.text.starts_with("<b>time</b> &lt; "), "{result:?}");
    let text = result.plain_text();
    let time = text
        .strip_prefix("time < ")
        .unwrap_or_else(|| panic!("unexpected plain text {text:?} of {result:?}"));
    assert!(time.parse::<u64>().is_ok(), "not a game time: {time}");
    assert_eq!(
        console.recent_commands(),
        vec!["1+1".to_owned(), html.to_owned()]
    );

    assert!(matches!(
        console.execute("  ", None).await,
        Err(Error::InvalidInput(_))
    ));
}

/// An expression that throws reports the error on the feed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a live private server: cargo xtask e2e"]
async fn throwing_expression_reports_the_error() {
    let player = support::player().await;
    let overseer = player.account.connect().await;
    let console = overseer.console();
    let mut feed = console.feed().expect("the console feed");
    support::subscribed(&overseer).await;

    console
        .execute("throw new Error('live-boom')", None)
        .await
        .expect("the expression is accepted");
    let error = next("the error", &mut feed, |entry| {
        entry.kind != ConsoleKind::Command && entry.plain_text().contains("live-boom")
    })
    .await;
    assert_ne!(error.kind, ConsoleKind::Log, "{error:?}");
}
