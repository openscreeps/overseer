//! A small terminal client: renders one room as ASCII (terrain and objects by type) and streams
//! the signed-in user's console, redrawing on every room update. A skeleton of a real client
//! built on `overseer-net` alone.
//!
//! ```sh
//! cargo run -p overseer-net --example room_view -- W1N1 --ticks 5 --console "Game.time"
//! ```
//!
//! Arguments: the room (default: the account's start room), `--ticks N` to exit after `N` room
//! updates (default: run until interrupted), `--console EXPR` to send one console expression
//! once connected, and `--no-clear` to append frames instead of redrawing in place.
//!
//! Environment: `SCREEPS_URL` (default `http://localhost:21025/`), `SCREEPS_TOKEN` or
//! `SCREEPS_EMAIL` + `SCREEPS_PASSWORD`, `SCREEPS_SERVER_PASSWORD` and `SCREEPS_SHARD`.
//!
//! This prints the room's content and the account's console output: run it in your own
//! terminal, not where logs are collected.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt::Write as _;

use overseer_net::model::console::ConsoleEvent;
use overseer_net::prelude::*;

/// Console lines kept below the map.
const CONSOLE_LINES: usize = 12;

struct Options {
    room: Option<String>,
    ticks: Option<u32>,
    console: Option<String>,
    clear: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_options()?;
    let client = build_client()?;
    let world = client.world().await?;
    if world.user().is_none() {
        return Err("set SCREEPS_TOKEN or SCREEPS_EMAIL + SCREEPS_PASSWORD".into());
    }
    let room = match options.room {
        Some(room) => room,
        None => start_room(&client).await?,
    };

    let mut socket = client.connect().await?;
    socket.watch_room(&room).await?;
    socket.subscribe_console()?;
    if let Some(expression) = &options.console {
        client.api().await?.console(expression, None).await?;
    }

    let mut console = VecDeque::new();
    let mut updates = 0_u32;
    while let Some(event) = StreamExt::next(&mut socket).await {
        let event = match event {
            Ok(event) => event,
            Err(overseer_net::Error::Socket(error)) => {
                push(&mut console, format!("[undecodable frame: {error}]"));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        match event {
            SocketEvent::Event(Event::Channel {
                payload: Payload::Room,
                ..
            }) => {
                if let Some(view) = socket.room_view(&room) {
                    print!("{}", render(&view, &console, options.clear));
                }
                updates += 1;
                if options.ticks.is_some_and(|ticks| updates >= ticks) {
                    break;
                }
            }
            SocketEvent::Event(Event::Channel {
                payload: Payload::Console(output),
                ..
            }) => match output {
                ConsoleEvent::Output { log, results, .. } => {
                    for line in log {
                        push(&mut console, line);
                    }
                    for line in results {
                        push(&mut console, format!("> {line}"));
                    }
                }
                ConsoleEvent::Error { error, .. } => {
                    push(&mut console, format!("! {error}"));
                }
            },
            SocketEvent::Disconnected(reason) => {
                push(&mut console, format!("[disconnected: {reason:?}]"));
            }
            SocketEvent::Reconnecting { attempt, delay } => {
                push(
                    &mut console,
                    format!("[reconnecting, attempt {attempt} in {delay:?}]"),
                );
            }
            _ => {}
        }
    }
    socket.close().await?;
    Ok(())
}

fn parse_options() -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        room: None,
        ticks: None,
        console: None,
        clear: true,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ticks" => {
                options.ticks = Some(args.next().ok_or("--ticks needs a number")?.parse()?);
            }
            "--console" => {
                options.console = Some(args.next().ok_or("--console needs an expression")?);
            }
            "--no-clear" => options.clear = false,
            room if !room.starts_with("--") && options.room.is_none() => {
                options.room = Some(room.to_owned());
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    Ok(options)
}

fn build_client() -> Result<Client, Box<dyn Error>> {
    let env = |name| {
        std::env::var(name)
            .ok()
            .filter(|value: &String| !value.is_empty())
    };
    let url = env("SCREEPS_URL").unwrap_or_else(|| "http://localhost:21025/".to_owned());
    let mut builder = Client::builder(ServerUrl::parse(&url)?);
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
    Ok(builder.build()?)
}

async fn start_room(client: &Client) -> Result<String, Box<dyn Error>> {
    let shard = client.world().await?.shard().map(str::to_owned);
    let start = client.api().await?.start_room(shard.as_deref()).await?;
    Ok(start.ok_or("no start room")?.room)
}

fn push(console: &mut VecDeque<String>, line: String) {
    if console.len() == CONSOLE_LINES {
        console.pop_front();
    }
    console.push_back(line);
}

/// The character of an object type and its drawing priority (higher hides lower).
fn glyph(object: &GameRoomObject) -> (char, u8) {
    match object {
        GameRoomObject::Creep(_) => ('c', 9),
        GameRoomObject::PowerCreep(_) => ('p', 9),
        GameRoomObject::Spawn(_) => ('S', 8),
        GameRoomObject::Controller(_) => ('K', 8),
        GameRoomObject::Source(_) => ('$', 8),
        GameRoomObject::Mineral(_) => ('M', 8),
        GameRoomObject::Deposit(_) => ('D', 8),
        GameRoomObject::Extension(_) => ('e', 7),
        GameRoomObject::Tower(_) => ('T', 7),
        GameRoomObject::Storage(_) => ('O', 7),
        GameRoomObject::Terminal(_) => ('X', 7),
        GameRoomObject::Link(_) => ('L', 7),
        GameRoomObject::Lab(_) => ('l', 7),
        GameRoomObject::Container(_) => ('u', 7),
        GameRoomObject::ConstructedWall(_) => ('W', 6),
        GameRoomObject::Rampart(_) => ('R', 5),
        GameRoomObject::ConstructionSite(_) => ('?', 4),
        GameRoomObject::Tombstone(_) | GameRoomObject::Ruin(_) => ('x', 3),
        GameRoomObject::Resource(_) => ('*', 3),
        GameRoomObject::Road(_) => ('+', 2),
        _ => ('o', 1),
    }
}

/// One frame: the map, the legend, users and flags, and the latest console lines.
fn render(view: &RoomView<'_>, console: &VecDeque<String>, clear: bool) -> String {
    let mut grid = vec![[(' ', 0_u8); 50]; 50];
    for (y, row) in grid.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            let tile = u8::try_from(x)
                .ok()
                .zip(u8::try_from(y).ok())
                .and_then(|(x, y)| view.tile(x, y));
            cell.0 = match tile {
                Some(Tile::Wall) => '#',
                Some(Tile::Swamp) => '~',
                Some(Tile::Plain) | None => '.',
            };
        }
    }
    let mut untyped = 0;
    for (_, object) in view.objects() {
        let Ok(object) = object else {
            untyped += 1;
            continue;
        };
        let (Ok(x), Ok(y)) = (usize::try_from(object.x()), usize::try_from(object.y())) else {
            continue;
        };
        if let Some(cell) = grid.get_mut(y).and_then(|row| row.get_mut(x)) {
            let (glyph, priority) = glyph(&object);
            if priority >= cell.1 {
                *cell = (glyph, priority);
            }
        }
    }

    let mut frame = String::new();
    if clear {
        frame.push_str("\x1b[2J\x1b[H");
    }
    let _ = writeln!(
        frame,
        "{} — tick {}",
        view.room(),
        view.game_time()
            .map_or_else(|| "?".to_owned(), |tick| tick.to_string())
    );
    for row in &grid {
        frame.extend(row.iter().map(|(glyph, _)| glyph));
        frame.push('\n');
    }
    frame.push_str(
        "# wall ~ swamp c creep S spawn K controller $ source M mineral e extension T tower \
         O storage + road W wall R rampart ? site * resource o other\n",
    );
    let users: Vec<&str> = view
        .users()
        .map(|(_, user)| user.username.as_str())
        .collect();
    let _ = writeln!(frame, "users: {}", users.join(", "));
    match view.flags() {
        Ok(flags) => {
            let flags: Vec<String> = flags
                .iter()
                .map(|flag| format!("{}@{},{}", flag.name, flag.x, flag.y))
                .collect();
            let _ = writeln!(frame, "flags: {}", flags.join(", "));
        }
        Err(error) => {
            let _ = writeln!(frame, "flags: {error}");
        }
    }
    let visuals = view.visuals().flatten().count();
    let _ = writeln!(
        frame,
        "visuals: {visuals}, objects not matching the reference: {untyped}"
    );
    frame.push_str("--- console ---\n");
    for line in console {
        frame.push_str(line);
        frame.push('\n');
    }
    frame
}
