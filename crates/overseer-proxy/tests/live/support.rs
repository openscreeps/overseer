//! What every live proxy test needs: the upstream server from the environment, a proxy in front
//! of it, fresh accounts and players, and raw HTTP access.
//!
//! `cargo xtask e2e` starts a disposable world of `openscreeps/server` and sets
//! `OVERSEER_E2E_TARGET=openscreeps`, `OVERSEER_LIVE_URL` (the server base URL),
//! `OVERSEER_LIVE_USER` and `OVERSEER_LIVE_PASSWORD` (a player without a spawn). Tests register
//! their own accounts (through the proxy) so that none depends on another's state.
//!
//! `cargo xtask e2e --target community` runs them against a disposable community world instead
//! (the Screeps server with screepsmod-auth and screepsmod-admin-utils, which lists the shard
//! name `shard0` although it serves a single world), with `OVERSEER_E2E_TARGET=community` and
//! the same variables but no admin API. Where the two servers legitimately differ, tests assert
//! each one's behaviour ([`community`]).
//!
//! `cargo xtask e2e --target official` runs only the `read_only` tests against screeps.com, with
//! `OVERSEER_E2E_TARGET=official`, `OVERSEER_LIVE_URL` and `OVERSEER_LIVE_TOKEN` (an API token of
//! an account that owns rooms); [`reader`] is that account on every target.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use overseer_net::core::api::Tile;
use overseer_net::core::api::endpoints;
use overseer_net::core::api::types::{
    GameCoordinateInput, GamePlaceSpawnRequest, RegisterSubmitRequest, UserCodeModules,
    UserCodeModulesValue,
};
use overseer_net::core::{ServerUrl, Token};
use overseer_net::{Client, Socket, SocketEvent};
use overseer_proxy::{Proxy, ProxyConfig};
use url::Url;

/// Upper bound of every wait for the server (ticks run every few hundred milliseconds).
pub const PATIENCE: Duration = Duration::from_secs(30);

/// A small bot: keeps two creeps alive that wander randomly and logs the tick to the console.
pub const BOT: &str = r"module.exports.loop = function () {
    for (const name in Memory.creeps) if (!Game.creeps[name]) delete Memory.creeps[name];
    for (const spawn of Object.values(Game.spawns)) {
        if (!spawn.spawning && Object.keys(Game.creeps).length < 2) spawn.spawnCreep([MOVE], 'c' + Game.time);
    }
    for (const creep of Object.values(Game.creeps)) creep.move(1 + Math.floor(Math.random() * 8));
    console.log('tick ' + Game.time);
};
";

/// The server under test (`OVERSEER_LIVE_URL`), the proxy's upstream.
///
/// # Panics
///
/// When the variable is missing or not a URL: run the live tests with `cargo xtask e2e`.
pub fn upstream() -> Url {
    let url = std::env::var("OVERSEER_LIVE_URL")
        .expect("OVERSEER_LIVE_URL is not set: run the live tests with `cargo xtask e2e`");
    Url::parse(&url).expect("OVERSEER_LIVE_URL is a URL")
}

/// The upstream as a client's server, bypassing the proxy.
pub fn direct() -> ServerUrl {
    ServerUrl::parse(upstream().as_str()).expect("a server URL")
}

/// Starts a proxy in front of [`upstream`] on a free loopback port.
pub async fn start(static_dir: Option<PathBuf>, allowed_origins: &[&str]) -> Proxy {
    Proxy::start(ProxyConfig {
        upstream: upstream(),
        listen: "127.0.0.1:0".parse().expect("a socket address"),
        static_dir,
        allowed_origins: allowed_origins
            .iter()
            .map(|&origin| origin.to_owned())
            .collect(),
    })
    .await
    .expect("the proxy starts")
}

/// The proxy as a client's server, known as [`direct`].
pub fn via(proxy: &Proxy) -> ServerUrl {
    ServerUrl::proxied(proxy.url().as_str(), &direct()).expect("a server URL")
}

/// A client without an account.
pub fn anonymous(server: &ServerUrl) -> Client {
    Client::builder(server.clone()).build().expect("a client")
}

/// Whether the tests run against screeps.com (`OVERSEER_E2E_TARGET=official`): a sharded world
/// where [`reader`] owns rooms.
pub fn official() -> bool {
    std::env::var("OVERSEER_E2E_TARGET").is_ok_and(|target| target == "official")
}

/// Whether the tests run against a community server (`OVERSEER_E2E_TARGET=community`): the
/// Screeps server (`@screeps/backend` 3.3.0) with screepsmod-auth 2.10.1 and
/// screepsmod-admin-utils 1.37.1.
pub fn community() -> bool {
    std::env::var("OVERSEER_E2E_TARGET").is_ok_and(|target| target == "community")
}

/// The length of the server's session tokens (hex digits): 40 on a community server, whose
/// tokens are SHA-1 HMACs (`@screeps/backend` 3.3.0 `lib/authlib.js` `genToken`,
/// <https://github.com/screeps/backend-local/blob/9d079282303ec04e577ac2bb97f64312b25e4ccd/lib/authlib.js#L7-L8>),
/// 64 on `openscreeps/server` (32 random bytes).
pub fn token_length() -> usize {
    if community() { 40 } else { 64 }
}

/// A path the upstream answers itself, outside the paths the proxy forwards: `health` on
/// `openscreeps/server`, the server statistics `stats` of screepsmod-admin-utils on a community
/// server (`lib/services/stats/backend.js`,
/// <https://unpkg.com/screepsmod-admin-utils@1.37.1/lib/services/stats/backend.js>).
pub fn upstream_own_path() -> &'static str {
    if community() { "stats" } else { "health" }
}

/// The account of the `read_only` tests on `server`: a client holding the API token
/// `OVERSEER_LIVE_TOKEN` when it is set (it never signs in), else the harness's player
/// (`OVERSEER_LIVE_USER`, `OVERSEER_LIVE_PASSWORD`: no spawn).
///
/// # Panics
///
/// Without a token, when the harness's player is not set: run `cargo xtask e2e`.
pub fn reader(server: &ServerUrl) -> Client {
    let builder = Client::builder(server.clone());
    let builder = match std::env::var("OVERSEER_LIVE_TOKEN") {
        Ok(token) if !token.is_empty() => builder.token(Token::new(token)),
        _ => {
            let var = |name: &str| {
                std::env::var(name)
                    .unwrap_or_else(|_| panic!("{name} is not set: run `cargo xtask e2e`"))
            };
            builder.credentials(var("OVERSEER_LIVE_USER"), var("OVERSEER_LIVE_PASSWORD"))
        }
    };
    builder.build().expect("a client")
}

/// The start room of `client`'s account (`GET /api/user/world-start-room`) as `(shard, room)`:
/// sharded servers name it `shard/room`, single-world servers only the room.
///
/// # Panics
///
/// When the request fails or the server names no room.
pub async fn start_room(client: &Client) -> (Option<String>, String) {
    let rooms = client
        .send(&endpoints::UserWorldStartRoom { shard: None })
        .await
        .expect("the start room")
        .body
        .room;
    let room = rooms
        .into_iter()
        .next()
        .expect("the server names a start room");
    match room.split_once('/') {
        Some((shard, room)) => (Some(shard.to_owned()), room.to_owned()),
        None => (None, room),
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
    /// A client of `server` signing in with the username and password.
    pub fn client(&self, server: &ServerUrl) -> Client {
        Client::builder(server.clone())
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

/// Registers a new account on `server` (`POST /api/register/submit`) with `modules` as its code.
pub async fn register_with(server: &ServerUrl, modules: Option<UserCodeModules>) -> Account {
    let username = unique("lp");
    let account = Account {
        email: format!("{username}@e2e.invalid"),
        password: format!("pw-{}", unique("")),
        username,
    };
    let body = RegisterSubmitRequest {
        email: account.email.clone(),
        modules,
        password: account.password.clone(),
        username: account.username.clone(),
        ..RegisterSubmitRequest::default()
    };
    anonymous(server)
        .send(&endpoints::RegisterSubmit { body })
        .await
        .expect("registration succeeds");
    account
}

/// Registers a new account running [`BOT`] on `server` and places its spawn in a free room.
pub async fn player(server: &ServerUrl) -> Player {
    let modules = UserCodeModules::from([(
        "main".to_owned(),
        UserCodeModulesValue::String(BOT.to_owned()),
    )]);
    let account = register_with(server, Some(modules)).await;
    let client = account.client(server);
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
                return Player { account, room };
            }
        }
    }
    panic!("no room accepts a spawn");
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
/// When the socket fails, closes for good, or nothing is found in time.
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

/// A plain HTTP client that neither follows redirects nor uses a system proxy.
pub fn http() -> reqwest::Client {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .expect("an HTTP client")
}

/// The value of response header `name`.
pub fn header<'a>(response: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    response
        .headers()
        .get(name)
        .map(|value| value.to_str().expect("a text header"))
}

/// A fresh directory under the test target directory, removed on drop.
#[derive(Debug)]
pub struct TempDir(pub PathBuf);

impl TempDir {
    /// Creates the directory.
    pub fn new(name: &str) -> Self {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(unique(&format!("live-{name}-")));
        std::fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }

    /// Writes the file `name`.
    pub fn write(&self, name: &str, contents: &str) {
        std::fs::File::create(self.0.join(name))
            .and_then(|mut file| file.write_all(contents.as_bytes()))
            .expect("a written file");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Every room of the world whose status is `normal` and that nobody owns.
async fn rooms(client: &Client) -> Vec<String> {
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
    let stats = client
        .map_stats(None, names, "owner0")
        .await
        .expect("map stats");
    stats
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
