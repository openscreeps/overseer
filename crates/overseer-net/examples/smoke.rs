//! Read-only end-to-end check of `overseer-core` + `overseer-net` against a real server.
//!
//! ```sh
//! cargo run -p overseer-net --example smoke
//! ```
//!
//! Environment: `SCREEPS_URL` (default: the official server), `SCREEPS_TOKEN` (optional; never
//! printed), `SCREEPS_EMAIL` + `SCREEPS_PASSWORD` (password sign-in on private servers),
//! `SCREEPS_SERVER_PASSWORD` (password-protected private servers), `SCREEPS_SHARD` (default: the
//! first shard the server lists), `SCREEPS_ROOM` (default: the account's start room, else `W1N1`)
//! and `SCREEPS_SECONDS` (socket observation time, default 10). Prints outcomes and counts only:
//! no tokens, user ids, Memory values, console output or the account's room names. Every request
//! is read-only (code branches are not listed: that request may create a `default` branch).
use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;
use std::time::Duration;

use overseer_net::core::api::official;
use overseer_net::model::account::UserRoomStatName;
use overseer_net::prelude::*;
use tokio::time::{Instant, timeout};

/// The room the socket phase observes.
struct Target {
    room: String,
    /// How to print the room: its name, unless it is the account's start room.
    label: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let server = match std::env::var("SCREEPS_URL") {
        Ok(url) => ServerUrl::parse(&url)?,
        Err(_) => ServerUrl::official(),
    };
    let mut builder = Client::builder(server);
    if let Some(token) = env("SCREEPS_TOKEN") {
        builder = builder.token(Token::new(token));
    }
    if let (Some(email), Some(password)) = (env("SCREEPS_EMAIL"), env("SCREEPS_PASSWORD")) {
        builder = builder.credentials(email, password);
    }
    if let Some(password) = env("SCREEPS_SERVER_PASSWORD") {
        builder = builder.server_password(password);
    }
    if let Some(shard) = env("SCREEPS_SHARD") {
        builder = builder.default_shard(shard);
    }
    let client = builder.build()?;
    let seconds = env("SCREEPS_SECONDS")
        .and_then(|value| value.parse().ok())
        .unwrap_or(10);
    let target = check_http(&client).await?;
    observe_socket(&client, &target, Duration::from_secs(seconds)).await
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

async fn check_http(client: &Client) -> Result<Target, Box<dyn Error>> {
    let world = client.world().await?;
    let signed_in = world.user().is_some();
    println!(
        "server {} ({}): {} shards, socket channels {}, protocol {}, history chunks of {} ticks",
        client.server(),
        if signed_in { "signed in" } else { "anonymous" },
        world.shards().len(),
        if world.is_sharded() {
            "shard-qualified"
        } else {
            "shardless"
        },
        world.version().protocol,
        world.history_chunk_size(),
    );
    let shard = world.shard().map(str::to_owned);
    let time = client.api().await?.game_time(shard.as_deref()).await?;
    println!("game time {time}");

    let mut target = env("SCREEPS_ROOM").map(|room| Target {
        label: room.clone(),
        room,
    });
    if signed_in {
        let api = client.api().await?;
        api.me().await?;
        println!(
            "auth/me ok (rate limit remaining {:?})",
            client
                .rate_limit::<official::endpoints::AuthMe>()
                .map(|limit| limit.remaining)
        );
        api.world_status().await?;
        println!("world status ok (not shown)");
        if target.is_none() {
            let start = api.start_room(shard.as_deref()).await?;
            target = start.map(|start| Target {
                room: start.room,
                label: "start room".to_owned(),
            });
        }
        let kind = match api.memory("", None).await? {
            MemoryValue::Undefined => "undefined",
            MemoryValue::IncorrectPath => "incorrect path",
            MemoryValue::Json(_) => "JSON value",
        };
        println!("memory: {kind} (content not shown)");
        let overview = api.overview(8, UserRoomStatName::EnergyHarvested).await?;
        println!(
            "overview: {} rooms (content not shown)",
            overview.rooms.len()
        );
    }
    let target = target.unwrap_or_else(|| Target {
        room: "W1N1".to_owned(),
        label: "W1N1".to_owned(),
    });

    let terrain = client.terrain(None, &target.room).await?;
    let cached = client.terrain(None, &target.room).await?;
    println!(
        "terrain {}: {:?} (cached: {})",
        target.label,
        tile_counts(&terrain),
        Arc::ptr_eq(&terrain, &cached)
    );
    let api = client.api().await?;
    match api.room_objects_source() {
        Some(source) => {
            let objects = source.room_objects(None, &target.room).await?;
            println!(
                "room objects {}: {:?}, {} users",
                target.label,
                kind_counts(objects.objects.iter()),
                objects.users.len()
            );
        }
        None => println!("room objects {}: not offered by the server", target.label),
    }
    if signed_in {
        let stats = api
            .map_stats(None, vec![target.room.clone()], "owner0")
            .await?;
        println!(
            "map stats {}: {} rooms, {} users",
            target.label,
            stats.stats.len(),
            stats.users.len()
        );
    }
    check_history(client, &target, time).await?;
    println!(
        "rate limits known for {} endpoints",
        client.rate_limits().len()
    );
    Ok(target)
}

/// Replays the history chunk three chunks back; a chunk the server does not keep is reported,
/// not an error.
async fn check_history(client: &Client, target: &Target, time: u64) -> Result<(), Box<dyn Error>> {
    let chunk = client.world().await?.history_chunk_size();
    let tick = time.saturating_sub(3 * chunk);
    match client
        .api()
        .await?
        .room_history(None, &target.room, tick)
        .await
    {
        Ok(history) => {
            let base = history.base();
            let mut replay = history.replay();
            let mut ticks = 0_u32;
            while replay.advance().is_some() {
                ticks += 1;
            }
            println!(
                "history {} from {base}: {ticks} ticks replayed, {} objects at the end",
                target.label,
                replay.state().objects().count()
            );
        }
        Err(overseer_net::Error::Api(ApiError::Status { status, .. })) => {
            println!("history {}: chunk not kept (HTTP {status})", target.label);
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn tile_counts(terrain: &Terrain) -> BTreeMap<&'static str, u32> {
    let mut counts = BTreeMap::new();
    for y in 0..50 {
        for x in 0..50 {
            let kind = match terrain.get(x, y) {
                Some(Tile::Plain) => "plain",
                Some(Tile::Swamp) => "swamp",
                Some(Tile::Wall) => "wall",
                None => continue,
            };
            *counts.entry(kind).or_insert(0) += 1;
        }
    }
    counts
}

fn kind_counts<'a>(objects: impl Iterator<Item = &'a GameRoomObject>) -> BTreeMap<&'a str, u32> {
    let mut counts = BTreeMap::new();
    for object in objects {
        *counts.entry(object.kind()).or_insert(0) += 1;
    }
    counts
}

async fn observe_socket(
    client: &Client,
    target: &Target,
    duration: Duration,
) -> Result<(), Box<dyn Error>> {
    let mut socket = client.connect().await?;
    socket.subscribe_server_messages();
    if socket.world().user().is_some() {
        socket.watch_room(&target.room).await?;
        socket.subscribe_room_map(&target.room);
        socket.subscribe_map_visual()?;
        socket.subscribe_cpu()?;
        socket.subscribe_console()?;
        socket.subscribe_resources()?;
    }

    let deadline = Instant::now() + duration;
    let mut counts = BTreeMap::<&str, u32>::new();
    let mut decode_errors = 0_u32;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        // The socket is a `Stream`; `Socket::next` would do as well.
        let event = match timeout(remaining, StreamExt::next(&mut socket)).await {
            Err(_elapsed) => break,
            Ok(Some(Ok(event))) => event,
            Ok(None) => {
                println!("socket closed for good");
                break;
            }
            Ok(Some(Err(overseer_net::Error::Socket(error)))) => {
                println!("undecodable frame: {error}");
                decode_errors += 1;
                continue;
            }
            Ok(Some(Err(error))) => return Err(error.into()),
        };
        if let SocketEvent::Disconnected(reason) = &event {
            println!("disconnected: {}", disconnect_kind(reason));
        }
        *counts.entry(event_kind(&event)).or_insert(0) += 1;
    }
    let mut untyped = 0_u32;
    if let Some(view) = socket.room_view(&target.room)
        && let Some(state) = view.state()
    {
        let mut kinds = BTreeMap::<String, u32>::new();
        for (_, object) in view.objects() {
            // Decode errors can quote object values, so only count them.
            match object {
                Ok(object) => *kinds.entry(object.kind().to_owned()).or_insert(0) += 1,
                Err(_) => untyped += 1,
            }
        }
        let visuals = view.visuals().flatten().count();
        let flags = view.flags().map_or(0, |flags| flags.len());
        println!(
            "room {}: {} objects ({untyped} not matching the reference), {} users, {visuals} \
             visuals, {flags} flags, game time {:?}",
            target.label,
            state.objects().count(),
            view.users().count(),
            view.game_time()
        );
        println!("typed room objects: {kinds:?}");
    }
    println!("socket events: {counts:?}");
    socket.close().await?;
    if decode_errors > 0 || untyped > 0 {
        return Err(format!(
            "{decode_errors} undecodable socket frames, {untyped} untyped room objects"
        )
        .into());
    }
    Ok(())
}

const fn disconnect_kind(reason: &DisconnectReason) -> &'static str {
    match reason {
        DisconnectReason::Closed { .. } => "closed by the server",
        DisconnectReason::Ended => "connection ended",
        DisconnectReason::Transport(_) => "transport failed",
        DisconnectReason::KeepAliveTimeout => "keep-alive timeout",
        DisconnectReason::ConnectFailed(_) => "connection attempt failed",
    }
}

const fn event_kind(event: &SocketEvent) -> &'static str {
    let event = match event {
        SocketEvent::Connected => return "connected",
        SocketEvent::Disconnected(_) => return "disconnected",
        SocketEvent::Reconnecting { .. } => return "reconnecting",
        SocketEvent::Event(event) => event,
    };
    match event {
        Event::Time(_) => "time",
        Event::Protocol(_) => "protocol",
        Event::Package(_) => "package",
        Event::Authenticated => "authenticated",
        Event::AuthFailed => "auth failed",
        Event::ServerDown => "server down",
        Event::CannotSubscribe(_) => "cannot subscribe",
        Event::ChannelError { .. } => "channel error",
        Event::Unsolicited { .. } => "unsolicited",
        Event::Channel { payload, .. } => match payload {
            Payload::ServerMessage(_) => "server message",
            Payload::Room => "room",
            Payload::RoomMap(_) => "room map",
            Payload::MapVisual(_) => "map visual",
            Payload::Cpu(_) => "cpu",
            Payload::Console(_) => "console",
            Payload::Memory(_) => "memory",
            Payload::Resources(_) => "resources",
            Payload::Code(_) => "code",
            Payload::ActiveBranch(_) => "active branch",
            Payload::Message(_) => "message",
        },
    }
}
