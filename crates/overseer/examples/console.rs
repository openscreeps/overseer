//! A console REPL: each line read from stdin runs in the player's runtime, and the live console
//! (output, results, errors and the commands sent) is printed as plain text.
//!
//! ```sh
//! echo 'Game.time' | cargo run -p overseer --example console
//! ```
//!
//! Environment: `SCREEPS_URL` (default: the official server), `SCREEPS_TOKEN` (never printed),
//! or `SCREEPS_EMAIL` + `SCREEPS_PASSWORD` (password sign-in on private servers),
//! `SCREEPS_SERVER_PASSWORD` (password-protected private servers), `SCREEPS_SHARD` (default:
//! the first shard the server lists) and `SCREEPS_LINGER` (seconds to keep printing output after
//! stdin ends, default 3).
use std::error::Error;
use std::time::Duration;

use overseer::prelude::*;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let url = match env("SCREEPS_URL") {
        Some(url) => ServerUrl::parse(&url)?,
        None => ServerUrl::official(),
    };
    let auth = match (
        env("SCREEPS_TOKEN"),
        env("SCREEPS_EMAIL"),
        env("SCREEPS_PASSWORD"),
    ) {
        (Some(token), _, _) => Auth::Token(Token::new(token)),
        (None, Some(email), Some(password)) => Auth::Credentials { email, password },
        _ => return Err("set SCREEPS_TOKEN, or SCREEPS_EMAIL and SCREEPS_PASSWORD".into()),
    };
    let linger = env("SCREEPS_LINGER")
        .and_then(|seconds| seconds.parse().ok())
        .map_or(Duration::from_secs(3), Duration::from_secs_f64);
    let overseer = Overseer::connect(ServerProfile {
        url,
        auth,
        server_password: env("SCREEPS_SERVER_PASSWORD"),
        default_shard: env("SCREEPS_SHARD"),
        server_kind: None,
        plugins: None,
    })
    .await?;
    eprintln!(
        "connected to {} ({} shards); type expressions, end with EOF",
        overseer.client().server(),
        overseer.world().shards().len()
    );

    let console = overseer.console();
    let mut feed = console.feed()?;
    let printer = tokio::spawn(async move {
        while let Ok(entry) = feed.recv().await {
            let prefix = match entry.kind {
                ConsoleKind::Command => "> ",
                ConsoleKind::Log => "",
                ConsoleKind::Result => "< ",
                ConsoleKind::Error => "! ",
            };
            for line in entry.plain_text().lines() {
                println!("{prefix}{line}");
            }
        }
    });
    let mut notices = overseer.notices();
    tokio::spawn(async move {
        while let Ok(notice) = notices.recv().await {
            eprintln!("notice: {notice:?}");
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Err(error) = console.execute(&line, None).await {
            eprintln!("not sent: {error}");
        }
    }
    tokio::time::sleep(linger).await;
    overseer.shutdown().await;
    printer.await?;
    Ok(())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
