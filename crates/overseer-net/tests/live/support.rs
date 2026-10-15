//! What every live test needs: the server from the environment, fresh accounts and players, and
//! a way to wait for socket events.
//!
//! `cargo xtask e2e` starts a disposable world of `openscreeps/server` and sets
//! `OVERSEER_LIVE_URL` (the server base URL), `OVERSEER_LIVE_USER` and `OVERSEER_LIVE_PASSWORD`
//! (a player without a spawn). Tests register their own accounts so that none depends on
//! another's state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use overseer_net::core::api::Tile;
use overseer_net::core::api::endpoints;
use overseer_net::core::api::types::{
    GameCoordinateInput, GameMapStatsResponse, GamePlaceSpawnRequest, RegisterSubmitRequest,
    UserCodeModules, UserCodeModulesValue,
};
use overseer_net::core::{ServerUrl, Token};
use overseer_net::{Client, Socket, SocketEvent};

/// Upper bound of every wait for the server (ticks run every few hundred milliseconds).
pub const PATIENCE: Duration = Duration::from_secs(30);

/// A small bot: keeps two creeps alive that wander randomly, logs the tick to the console and
/// counts ticks in `Memory.ticks`.
pub const BOT: &str = r"module.exports.loop = function () {
    for (const name in Memory.creeps) if (!Game.creeps[name]) delete Memory.creeps[name];
    for (const spawn of Object.values(Game.spawns)) {
        if (!spawn.spawning && Object.keys(Game.creeps).length < 2) spawn.spawnCreep([MOVE], 'c' + Game.time);
    }
    for (const creep of Object.values(Game.creeps)) creep.move(1 + Math.floor(Math.random() * 8));
    Memory.ticks = (Memory.ticks || 0) + 1;
    console.log('tick ' + Game.time);
};
";

/// The server under test (`OVERSEER_LIVE_URL`).
///
/// # Panics
///
/// When the variable is missing or not a URL: run the live tests with `cargo xtask e2e`.
pub fn url() -> ServerUrl {
    let url = std::env::var("OVERSEER_LIVE_URL")
        .expect("OVERSEER_LIVE_URL is not set: run the live tests with `cargo xtask e2e`");
    ServerUrl::parse(&url).expect("OVERSEER_LIVE_URL is a server URL")
}

/// The harness's own player (`OVERSEER_LIVE_USER`, `OVERSEER_LIVE_PASSWORD`): no spawn.
pub fn admin() -> Account {
    let var = |name: &str| {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set: run `cargo xtask e2e`"))
    };
    let username = var("OVERSEER_LIVE_USER");
    Account {
        email: format!("{username}@e2e.invalid"),
        username,
        password: var("OVERSEER_LIVE_PASSWORD"),
    }
}

/// A registered account (password sign-in).
#[derive(Clone, Debug)]
pub struct Account {
    /// Unique username.
    pub username: String,
    /// Unique e-mail.
    pub email: String,
    /// Password.
    pub password: String,
}

impl Account {
    /// A client signing in with the username and password.
    pub fn client(&self) -> Client {
        Client::builder(url())
            .credentials(self.username.clone(), self.password.clone())
            .build()
            .expect("a client")
    }
}

/// A player: an account with a spawn and [`BOT`] running.
#[derive(Clone, Debug)]
pub struct Player {
    /// The account.
    pub account: Account,
    /// The room of the spawn.
    pub room: String,
    /// The spawn's position.
    pub spawn: (u8, u8),
}

/// A name nobody used yet: `prefix` and a value unique per process and call.
pub fn unique(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mixed = now
        .as_secs()
        .wrapping_mul(1_000_000_007)
        .wrapping_add(u64::from(now.subsec_nanos()))
        ^ u64::from(std::process::id()).wrapping_mul(0x0100_0000_01b3)
        ^ NEXT
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15);
    format!("{prefix}{:010x}", mixed & 0xff_ffff_ffff)
}

/// A fresh account that is not registered yet.
pub fn new_account() -> Account {
    let username = unique("ln");
    Account {
        email: format!("{username}@e2e.invalid"),
        password: format!("pw-{}", unique("")),
        username,
    }
}

/// The registration request of `account` with `modules` as its code.
pub fn registration(account: &Account, modules: Option<UserCodeModules>) -> RegisterSubmitRequest {
    RegisterSubmitRequest {
        email: account.email.clone(),
        modules,
        password: account.password.clone(),
        username: account.username.clone(),
        ..RegisterSubmitRequest::default()
    }
}

/// Registers a new account (`POST /api/register/submit`) with `modules` as its code.
pub async fn register_with(modules: Option<UserCodeModules>) -> Account {
    let account = new_account();
    let body = registration(&account, modules);
    anonymous()
        .send(&endpoints::RegisterSubmit { body })
        .await
        .expect("registration succeeds");
    account
}

/// Registers a new account without code.
pub async fn register() -> Account {
    register_with(None).await
}

/// Registers a new account running [`BOT`] and places its spawn in a free room.
pub async fn player() -> Player {
    let modules = UserCodeModules::from([(
        "main".to_owned(),
        UserCodeModulesValue::String(BOT.to_owned()),
    )]);
    let account = register_with(Some(modules)).await;
    let client = account.client();
    let mut rooms = rooms(&client).await;
    // Start at a different room per player so concurrent tests rarely race for one.
    let offset = usize::try_from(u64::from_str_radix(&unique("")[4..], 16).unwrap_or(0))
        .unwrap_or(0)
        % rooms.len().max(1);
    rooms.rotate_left(offset);
    for room in rooms {
        let Ok(terrain) = client.terrain(None, &room).await else {
            continue;
        };
        for (x, y) in spawn_spots(|x, y| terrain.get(x, y)).into_iter().take(4) {
            let body = GamePlaceSpawnRequest {
                name: Some("Spawn1".to_owned()),
                room: room.clone(),
                shard: None,
                x: GameCoordinateInput::Integer(x.into()),
                y: GameCoordinateInput::Integer(y.into()),
            };
            if client
                .send(&endpoints::GamePlaceSpawn { body })
                .await
                .is_ok()
            {
                return Player {
                    account,
                    room,
                    spawn: (x, y),
                };
            }
        }
    }
    panic!("no room accepts a spawn");
}

/// A client without an account.
pub fn anonymous() -> Client {
    Client::builder(url()).build().expect("a client")
}

/// A client that only holds `token` (no credentials to sign in again).
pub fn with_token(token: Token) -> Client {
    Client::builder(url())
        .token(token)
        .build()
        .expect("a client")
}

/// The current game tick.
pub async fn game_time(client: &Client) -> u64 {
    let time = client
        .send(&endpoints::GameTime { shard: None })
        .await
        .expect("the game time")
        .body
        .time;
    u64::try_from(time).expect("a non-negative game time")
}

/// Waits until the game reaches `tick`. Ticks slow down while many tests load the server, so
/// this allows twice [`PATIENCE`].
///
/// # Panics
///
/// When the tick is not reached in time.
pub async fn wait_for_tick(client: &Client, tick: u64) {
    let wait = async {
        while game_time(client).await < tick {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    tokio::time::timeout(PATIENCE * 2, wait)
        .await
        .unwrap_or_else(|_| panic!("tick {tick} is reached in time"));
}

/// Drives `socket` until `found` returns a value for an event (it can read the socket's state
/// after the event), within [`PATIENCE`].
///
/// # Panics
///
/// When the socket fails (an undecodable frame is contract drift), closes for good, or nothing
/// is found in time.
pub async fn until<T>(
    socket: &mut Socket,
    mut found: impl FnMut(&Socket, &SocketEvent) -> Option<T>,
) -> T {
    let wait = async {
        loop {
            let event = socket
                .next()
                .await
                .expect("the socket decodes every frame")
                .expect("the socket stays open");
            if let Some(value) = found(socket, &event) {
                return value;
            }
        }
    };
    tokio::time::timeout(PATIENCE, wait)
        .await
        .expect("the awaited socket event in time")
}

/// Ownership map statistics of every room the world may hold (rooms `[WE]0..=10[NS]0..=10`
/// cover any world up to 22 rooms across; the server leaves out rooms it does not have).
pub async fn world_map(client: &Client) -> GameMapStatsResponse {
    let mut names = Vec::new();
    for horizontal in ['W', 'E'] {
        for vertical in ['N', 'S'] {
            for x in 0..=10 {
                for y in 0..=10 {
                    names.push(format!("{horizontal}{x}{vertical}{y}"));
                }
            }
        }
    }
    client
        .map_stats(None, names, "owner0")
        .await
        .expect("map stats")
}

/// Every room of the world whose status is `normal` and that nobody owns.
async fn rooms(client: &Client) -> Vec<String> {
    world_map(client)
        .await
        .stats
        .into_iter()
        .filter(|(_, stat)| stat.status.as_deref() == Some("normal") && stat.own.is_none())
        .map(|(room, _)| room)
        .collect()
}

/// Tiles with open ground around them, away from the exits, nearest the centre first.
fn spawn_spots(tile: impl Fn(u8, u8) -> Option<Tile>) -> Vec<(u8, u8)> {
    let open = |x: u8, y: u8| {
        (x - 2..=x + 2).all(|x| (y - 2..=y + 2).all(|y| tile(x, y) == Some(Tile::Plain)))
    };
    let mut spots: Vec<(u8, u8)> = (6..44)
        .flat_map(|y| (6..44).map(move |x| (x, y)))
        .filter(|&(x, y)| open(x, y))
        .collect();
    spots.sort_by_key(|&(x, y)| (i32::from(x) - 25).pow(2) + (i32::from(y) - 25).pow(2));
    spots
}
