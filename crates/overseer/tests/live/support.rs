//! What every live test needs: the server from the environment, fresh accounts and players, and
//! the server's administration (admin API or CLI) where a test sets the world up.
//!
//! `cargo xtask e2e` starts a disposable world of `openscreeps/server` and sets
//! `OVERSEER_E2E_TARGET=openscreeps`, `OVERSEER_LIVE_URL` (the server base URL),
//! `OVERSEER_LIVE_USER` and `OVERSEER_LIVE_PASSWORD` (a player without a spawn) and
//! `OVERSEER_LIVE_ADMIN_TOKEN` (its admin API, see [`admin_api`]). Tests register their own
//! accounts so that none depends on another's state.
//!
//! `cargo xtask e2e --target community` runs them against a disposable community world instead
//! (the Screeps server with screepsmod-auth and screepsmod-admin-utils, which lists the shard
//! name `shard0` although it serves a single world), with `OVERSEER_E2E_TARGET=community`, the
//! same server and player variables and `OVERSEER_LIVE_CLI_URL` (the server's CLI, see [`cli`])
//! instead of the admin API. Where the two servers legitimately differ, tests assert each one's
//! behaviour ([`community`]).
//!
//! `cargo xtask e2e --target official` runs only the `read_only` tests against screeps.com, with
//! `OVERSEER_E2E_TARGET=official`, `OVERSEER_LIVE_URL` and `OVERSEER_LIVE_TOKEN` (an API token of
//! an account that owns rooms); [`reader`] is that account on every target.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::OnceCell;

use overseer::core::{ServerUrl, Token};
use overseer::model::account::Registration;
use overseer::model::code::Modules;
use overseer::net::Client;
use overseer::net::api::{PasswordAccount as _, WorldApi as _};
use overseer::net::model::world::{MapStats, Tile};
use overseer::{Auth, Overseer, ServerProfile};

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

/// Whether the tests run against screeps.com (`OVERSEER_E2E_TARGET=official`): a sharded world
/// where [`reader`] owns rooms.
pub fn official() -> bool {
    std::env::var("OVERSEER_E2E_TARGET").is_ok_and(|target| target == "official")
}

/// Whether the tests run against a community server (`OVERSEER_E2E_TARGET=community`): the
/// Screeps server (`@screeps/backend` 3.3.0) with screepsmod-auth 2.10.1 and
/// screepsmod-admin-utils 1.37.1, administered through [`cli`].
pub fn community() -> bool {
    std::env::var("OVERSEER_E2E_TARGET").is_ok_and(|target| target == "community")
}

/// Sends `body` to `path` on the server at `address` (`host:port`) as `POST` over HTTP/1.0 (so
/// the reply is never chunked) with the extra header lines `headers`; returns the status line
/// and the body.
async fn post(address: &str, path: &str, headers: &str, body: &str) -> (String, String) {
    let mut stream = TcpStream::connect(address)
        .await
        .expect("the server accepts connections");
    let request = format!(
        "POST {path} HTTP/1.0\r\nHost: {address}\r\n{headers}Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("the request is sent");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("the response arrives");
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let status = head.lines().next().unwrap_or_default().to_owned();
    (status, body.to_owned())
}

/// Whether the status line `status` is a success (HTTP 2xx).
fn succeeded(status: &str) -> bool {
    status
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code.starts_with('2'))
}

/// Runs `action` (a JSON object) on the admin API of `openscreeps/server` (`POST /admin` with
/// `OVERSEER_LIVE_ADMIN_TOKEN`) and returns the response body.
///
/// # Panics
///
/// When the token is not set or the admin API refuses the action.
pub async fn admin_api(action: &str) -> String {
    let token = std::env::var("OVERSEER_LIVE_ADMIN_TOKEN")
        .expect("OVERSEER_LIVE_ADMIN_TOKEN is not set: run `cargo xtask e2e`");
    let url = url().join("admin").expect("the admin URL");
    let host = url.host_str().expect("a host");
    let port = url.port_or_known_default().expect("a port");
    let headers = format!("Authorization: Bearer {token}\r\nContent-Type: application/json\r\n");
    let (status, body) = post(&format!("{host}:{port}"), url.path(), &headers, action).await;
    assert!(
        succeeded(&status),
        "the admin API refused {action}: {status}\n{body}"
    );
    body
}

/// Runs `command` (JavaScript) in the community server's CLI (`OVERSEER_LIVE_CLI_URL`, the
/// community counterpart of the admin API) and returns its result as the CLI prints it: strings
/// as they are, other values through `util.inspect`, promises once settled.
///
/// # Panics
///
/// When the variable is not set or the command fails (the CLI then prints `Error: …`).
pub async fn cli(command: &str) -> String {
    let cli = std::env::var("OVERSEER_LIVE_CLI_URL")
        .expect("OVERSEER_LIVE_CLI_URL is not set: run `cargo xtask e2e --target community`");
    let (address, path) = cli
        .strip_prefix("http://")
        .and_then(|rest| rest.split_once('/'))
        .expect("OVERSEER_LIVE_CLI_URL is http://<host>:<port>/<path>");
    let (status, body) = post(address, &format!("/{path}"), "", command).await;
    let result = body.trim_end().to_owned();
    assert!(
        succeeded(&status) && !result.starts_with("Error: "),
        "the CLI failed to run {command}: {status}\n{result}"
    );
    result
}

/// The profile of the `read_only` tests' account: the API token `OVERSEER_LIVE_TOKEN` when it
/// is set (it never signs in), else the harness's player ([`admin`]).
///
/// # Panics
///
/// Without a token, when the harness's player is not set (see [`admin`]).
pub fn reader() -> ServerProfile {
    match std::env::var("OVERSEER_LIVE_TOKEN") {
        Ok(token) if !token.is_empty() => ServerProfile {
            url: url(),
            auth: Auth::Token(Token::new(token)),
            server_password: None,
            default_shard: None,
            server_kind: None,
            plugins: None,
        },
        _ => admin().profile(),
    }
}

/// The start room of the session's account (`GET /api/user/world-start-room`) as
/// `(shard, room)`: sharded servers name its shard, single-world servers none.
///
/// # Panics
///
/// When the request fails or the server names no room.
pub async fn start_room(overseer: &Overseer) -> (Option<String>, String) {
    let start = overseer
        .account()
        .start_room(None)
        .await
        .expect("the start room")
        .expect("the server names a start room");
    (start.shard, start.room)
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
    /// A profile signing in with the username and password.
    pub fn profile(&self) -> ServerProfile {
        ServerProfile {
            url: url(),
            auth: Auth::Credentials {
                email: self.username.clone(),
                password: self.password.clone(),
            },
            server_password: None,
            default_shard: None,
            server_kind: None,
            plugins: None,
        }
    }

    /// A client signing in with the username and password.
    pub fn client(&self) -> Client {
        Client::builder(url())
            .credentials(self.username.clone(), self.password.clone())
            .build()
            .expect("a client")
    }

    /// A session of this account.
    pub async fn connect(&self) -> Overseer {
        Overseer::connect(self.profile())
            .await
            .expect("the account signs in")
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

/// Registers a new account (`POST /api/register/submit`) with `modules` as its code.
pub async fn register_with(modules: Option<Modules>) -> Account {
    let username = unique("lv");
    let account = Account {
        email: format!("{username}@e2e.invalid"),
        password: format!("pw-{}", unique("")),
        username,
    };
    let registration = Registration {
        username: account.username.clone(),
        email: account.email.clone(),
        password: account.password.clone(),
        modules,
    };
    anonymous()
        .api()
        .await
        .expect("the server's operations")
        .password_account()
        .expect("the server registers accounts")
        .register(registration)
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
    let mut modules = Modules::new();
    modules.insert_js("main", BOT);
    let account = register_with(Some(modules)).await;
    let client = account.client();
    let api = client.api().await.expect("the server's API");
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
            let placed = api.place_spawn(None, &room, (x, y), Some("Spawn1")).await;
            if placed.is_ok() {
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

/// Waits until the server has processed every subscription `overseer` sent so far.
///
/// Feeds and watches subscribe asynchronously over the socket, and the server publishes only
/// to subscribed channels: an event caused right after creating a feed may be missed. The
/// server answers a `user:<user>/resources` subscription at once and processes subscriptions in
/// order, so its first value proves the earlier ones are in place.
pub async fn subscribed(overseer: &Overseer) {
    let mut resources = overseer.account().resources().expect("a signed-in session");
    tokio::time::timeout(PATIENCE, resources.changed())
        .await
        .expect("the subscriptions are processed in time")
        .expect("the session is open");
}

/// The `owner0` map statistics of every room the server knows (it answers only for those),
/// searching up to 10 rooms from the centre in each direction.
pub async fn world(client: &Client) -> MapStats {
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
        .api()
        .await
        .expect("the server's API")
        .map_stats(None, names, "owner0")
        .await
        .expect("map stats")
}

/// The CLI command that makes sure a community world has a stronghold and prints its room: an
/// existing one, else one spawned in `W4N4` (a keeper room of the default world's sector around
/// `W5N5`, the kind of room the backend's `genStrongholds` picks).
const ENSURE_STRONGHOLD: &str = "storage.db['rooms.objects']
    .findOne({type: 'invaderCore', level: {$gt: 0}})
    .then(core => core ? core.room : strongholds.spawn('W4N4').then(() => 'W4N4'))";

/// The room of the world's stronghold: the one an invader core (user `2`) owns.
///
/// An `openscreeps/server` world is created with one. A community world gets its first from the
/// backend's `genStrongholds` cron job only some time after it starts (`@screeps/backend` 3.3.0
/// `lib/cronjobs.js`, `lib/strongholds.js`), so on `community` the first call makes sure of one
/// through the [`cli`] ([`ENSURE_STRONGHOLD`]) and every call names that one.
pub async fn stronghold(client: &Client) -> String {
    static ENSURED: OnceCell<String> = OnceCell::const_new();
    if community() {
        return ENSURED.get_or_init(|| cli(ENSURE_STRONGHOLD)).await.clone();
    }
    world(client)
        .await
        .stats
        .into_iter()
        .find(|(_, stat)| stat.own.as_ref().and_then(|own| own.user.as_deref()) == Some("2"))
        .map(|(room, _)| room)
        .expect("the world has a stronghold")
}

/// Every room of the world whose status is `normal` and that nobody owns.
async fn rooms(client: &Client) -> Vec<String> {
    world(client)
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
