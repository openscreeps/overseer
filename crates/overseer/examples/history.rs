//! Plays a room's history: seeks to a tick, then steps forward printing the objects of each
//! tick. Read-only.
//!
//! ```sh
//! SCREEPS_ROOM=W8N3 cargo run -p overseer --example history
//! ```
//!
//! Environment: `SCREEPS_ROOM` (required), `SCREEPS_TICK` (default: three history chunks
//! before the current tick), `SCREEPS_TICKS` (ticks to play, default 10), `SCREEPS_URL`
//! (default: the official server), `SCREEPS_TOKEN` (never printed), or `SCREEPS_EMAIL` +
//! `SCREEPS_PASSWORD` (password sign-in on private servers), `SCREEPS_SERVER_PASSWORD`
//! (password-protected private servers) and `SCREEPS_SHARD` (default: the first shard the
//! server lists).
use std::error::Error;

use overseer::prelude::*;

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
    let room = env("SCREEPS_ROOM").ok_or("set SCREEPS_ROOM")?;
    let ticks: u64 = env("SCREEPS_TICKS").map_or(Ok(10), |ticks| ticks.parse())?;
    let overseer = Overseer::connect(ServerProfile {
        url,
        auth,
        server_password: env("SCREEPS_SERVER_PASSWORD"),
        default_shard: env("SCREEPS_SHARD"),
        server_kind: None,
        plugins: None,
    })
    .await?;

    let start = if let Some(tick) = env("SCREEPS_TICK") {
        tick.parse()?
    } else {
        let now = overseer.client().api().await?.game_time(None).await?;
        let chunk = overseer.world().history_chunk_size();
        now.saturating_sub(3 * chunk)
    };
    let mut player = overseer.history().player(&room, None)?;
    if player.seek(start).await? == Seek::NotKept {
        return Err(format!("no history of {room} at tick {start}").into());
    }
    for _ in 0..ticks {
        if let Some(snapshot) = player.snapshot() {
            let mut kinds = std::collections::BTreeMap::<&str, usize>::new();
            for object in &snapshot.objects {
                *kinds.entry(object.kind()).or_default() += 1;
            }
            println!(
                "{room} tick {}: {} objects {kinds:?}, {} undecoded",
                snapshot.game_time.unwrap_or_default(),
                snapshot.objects.len(),
                snapshot.undecoded.len()
            );
        }
        if player.step().await? == Seek::NotKept {
            println!(
                "no history after tick {}",
                player.tick().unwrap_or_default()
            );
            break;
        }
    }
    overseer.shutdown().await;
    Ok(())
}

/// A non-empty environment variable.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}
