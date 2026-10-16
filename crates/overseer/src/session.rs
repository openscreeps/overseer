//! The session: client, world, background task and the feature handles.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use overseer_net::core::Token;
use overseer_net::core::api::types::AuthMeResponse;
use overseer_net::core::api::{Auth as EndpointAuth, Endpoint};
use overseer_net::core::socket::{Channel, Payload};
use overseer_net::rt::JoinHandle;
use overseer_net::{Client, World};
use tokio::sync::{broadcast, mpsc, watch};

use crate::live::{self, Command, Feed, Registry, Watch};
use crate::{
    Account, Auth, Code, Console, Error, History, Map, Market, Memory, Messages, Rooms,
    ServerProfile, Users, hub, refresh,
};

/// State of the socket connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// The first connection is being set up.
    Connecting,
    /// A connection is open; subscriptions are (re)sent after authentication.
    Connected,
    /// The connection was lost; the next attempt starts after `delay`. Subscriptions are kept
    /// and sent again once connected.
    Reconnecting {
        /// Consecutive attempt number, from 1.
        attempt: u32,
        /// Wait before the attempt.
        delay: Duration,
    },
    /// Shut down: [`Overseer::shutdown`] was called (or reconnecting was given up).
    Closed,
}

/// Something to tell the user outside any screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// A server announcement (`server-message`), or a `cannot subscribe…` line of the official
    /// server.
    ServerMessage(String),
    /// The server is under maintenance (`server down`, official server).
    ServerDown,
    /// The server rejected the token and no sign-in could replace it (`auth failed`, or a failed
    /// sign-in with credentials). Live data stops until a new session is connected.
    AuthFailed,
    /// The server refused a channel, for example `subscribe limit reached` for a room
    /// (`err@<channel>`).
    ChannelError {
        /// The channel.
        channel: Channel,
        /// The server's error text.
        message: String,
    },
    /// A received frame (or a part of a room update) could not be decoded; the connection
    /// stays usable.
    Undecodable(String),
    /// A [`Feed`] fell more than [`FEED_CAPACITY`](crate::live::FEED_CAPACITY) items behind and
    /// skipped `skipped` items.
    Lagged {
        /// The feed's channel.
        channel: Channel,
        /// Number of skipped items.
        skipped: u64,
    },
}

/// One account on one server: the HTTP client, the world, live data and every feature.
///
/// Cheap to clone; every clone (including those inside feature handles) shares the session.
/// Created by [`Self::connect`]. Dropping the last clone shuts the session down like
/// [`Self::shutdown`], without waiting for the socket to close. See the crate's
/// [session lifecycle](crate#session-lifecycle).
#[derive(Clone)]
pub struct Overseer {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    world: World,
    registry: Arc<Registry>,
    status: watch::Receiver<ConnectionStatus>,
    /// The background tasks (socket hub, token refresh), until [`Overseer::shutdown`] takes
    /// them.
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Console command recall, newest last.
    commands: Mutex<VecDeque<String>>,
    /// The signed-in user's profile once fetched ([`Account::me`]); the async lock makes
    /// concurrent first reads share one request.
    me: tokio::sync::Mutex<Option<Arc<AuthMeResponse>>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.registry.close();
        self.registry.command(Command::Shutdown);
    }
}

impl Overseer {
    /// Connects a session: builds the client for `profile`, discovers the server's world
    /// (`GET /api/version`, and `GET /api/auth/me` with a token or after signing in with
    /// credentials), opens the socket and spawns the background tasks: the one driving the
    /// socket and, unless anonymous, the one keeping rotating session tokens alive (see the
    /// crate's [session lifecycle](crate#session-lifecycle)).
    ///
    /// Natively, must be called inside a tokio runtime with the time driver enabled (for
    /// reconnect backoff, keep-alive, token refresh and rate-limit waits); the background tasks
    /// are tokio tasks. In browsers (`wasm32-unknown-unknown`) no runtime is needed: call it
    /// from a future the browser runs (`wasm_bindgen_futures::spawn_local`, an exported
    /// `async fn` or [`overseer_net::rt::spawn`]); the background tasks run on the browser's
    /// event loop and timers on `setTimeout`. See [`overseer_net`]'s "Native and web" for what
    /// the browser handles itself (CORS, keep-alive, redirects, `User-Agent`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the client cannot be built, discovery or sign-in fails, or
    /// the socket cannot be opened.
    pub async fn connect(profile: ServerProfile) -> Result<Self, Error> {
        let mut builder = Client::builder(profile.url);
        let signed_in = !matches!(profile.auth, Auth::Anonymous);
        match profile.auth {
            Auth::Anonymous => {}
            Auth::Token(token) => builder = builder.token(token),
            Auth::Credentials { email, password } => {
                builder = builder.credentials(email, password);
            }
        }
        if let Some(password) = profile.server_password {
            builder = builder.server_password(password);
        }
        if let Some(shard) = profile.default_shard {
            builder = builder.default_shard(shard);
        }
        let client = builder.build()?;
        // The profile discovery fetched is the first `Account::me`: not requested twice.
        let (world, me) = client.discover().await?;
        let socket = client.connect().await?;
        let (commands, receiver) = mpsc::unbounded_channel();
        let registry = Arc::new(Registry::new(commands));
        let (status_sender, status) = watch::channel(ConnectionStatus::Connecting);
        let mut tasks = vec![hub::spawn(
            socket,
            Arc::clone(&registry),
            receiver,
            status_sender,
        )];
        if signed_in {
            tasks.push(refresh::spawn(
                client.clone(),
                status.clone(),
                Arc::clone(&registry),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                world,
                registry,
                status,
                tasks: Mutex::new(tasks),
                commands: Mutex::new(VecDeque::new()),
                me: tokio::sync::Mutex::new(me.map(Arc::new)),
            }),
        })
    }

    /// The HTTP client: every typed endpoint ([`Client::send`]) and the client flows.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.inner.client
    }

    /// The server's shards and capabilities and the signed-in user, as discovered on connect.
    #[must_use]
    pub fn world(&self) -> &World {
        &self.inner.world
    }

    /// The signed-in user's id (`_id`); `None` when anonymous.
    #[must_use]
    pub fn user_id(&self) -> Option<&str> {
        self.inner.world.user()
    }

    /// The connection state, updated through reconnects.
    #[must_use]
    pub fn status(&self) -> watch::Receiver<ConnectionStatus> {
        self.inner.status.clone()
    }

    /// The current token and every change of it: tokens rotated by the server (`X-Token`,
    /// `auth ok`) and issued by sign-ins. Persist each value if the app stores tokens.
    #[must_use]
    pub fn tokens(&self) -> watch::Receiver<Option<Token>> {
        self.inner.client.token_updates()
    }

    /// The notices published from now on.
    #[must_use]
    pub fn notices(&self) -> broadcast::Receiver<Notice> {
        self.inner.registry.notices()
    }

    /// Closes the socket and ends the background tasks. Every [`Watch`] and [`Feed`] then ends
    /// with [`Closed`](crate::Closed), and operations of the feature handles fail with
    /// [`Error::Closed`]. The HTTP client stays usable through [`Self::client`], but the
    /// session no longer keeps its token alive.
    pub async fn shutdown(&self) {
        self.inner.registry.close();
        self.inner.registry.command(Command::Shutdown);
        let tasks = std::mem::take(&mut *lock(&self.inner.tasks));
        // The hub ends first and closes the session, which ends the others.
        for task in tasks {
            // Tasks only end by returning; a panic would already have been reported.
            let _ = task.await;
        }
    }

    /// Account, profile and game state of the signed-in user.
    #[must_use]
    pub fn account(&self) -> Account {
        Account::new(self.clone())
    }

    /// The live console and console commands.
    #[must_use]
    pub fn console(&self) -> Console {
        Console::new(self.clone())
    }

    /// Code branches and modules.
    #[must_use]
    pub fn code(&self) -> Code {
        Code::new(self.clone())
    }

    /// Memory and Memory segments.
    #[must_use]
    pub fn memory(&self) -> Memory {
        Memory::new(self.clone())
    }

    /// Live rooms.
    #[must_use]
    pub fn rooms(&self) -> Rooms {
        Rooms::new(self.clone())
    }

    /// Room history playback.
    #[must_use]
    pub fn history(&self) -> History {
        History::new(self.clone())
    }

    /// The world map.
    #[must_use]
    pub fn map(&self) -> Map {
        Map::new(self.clone())
    }

    /// Private messages.
    #[must_use]
    pub fn messages(&self) -> Messages {
        Messages::new(self.clone())
    }

    /// The market.
    #[must_use]
    pub fn market(&self) -> Market {
        Market::new(self.clone())
    }

    /// Other players and leaderboards.
    #[must_use]
    pub fn users(&self) -> Users {
        Users::new(self.clone())
    }

    /// A [`Watch`] of `channel`, converting each payload with `extract` (payloads it returns
    /// `None` for are skipped). A handle created after [`Self::shutdown`] is already closed.
    pub(crate) fn watch<T: Send + Sync + 'static>(
        &self,
        channel: Channel,
        extract: fn(Payload) -> Option<T>,
    ) -> Watch<T> {
        live::watch(&self.inner.registry, channel, extract)
    }

    /// A [`Feed`] of `channel`, converting each payload with `extract` (payloads it returns
    /// `None` for are skipped). A handle created after [`Self::shutdown`] is already closed.
    pub(crate) fn feed<T: Clone + Send + 'static>(
        &self,
        channel: Channel,
        extract: fn(Payload) -> Option<T>,
    ) -> Feed<T> {
        live::feed(&self.inner.registry, channel, extract)
    }

    /// The shard an operation names: `shard`, else the default shard; `None` on single-world
    /// servers whatever was asked.
    pub(crate) fn resolve_shard(&self, shard: Option<&str>) -> Option<String> {
        let world = &self.inner.world;
        if world.is_sharded() {
            shard.or(world.shard()).map(str::to_owned)
        } else {
            None
        }
    }

    /// The live-data registry.
    pub(crate) fn registry(&self) -> &Arc<Registry> {
        &self.inner.registry
    }

    /// Fails with [`Error::Closed`] after [`Self::shutdown`].
    pub(crate) fn ensure_open(&self) -> Result<(), Error> {
        if self.inner.registry.is_closed() {
            Err(Error::Closed)
        } else {
            Ok(())
        }
    }

    /// The signed-in user's id, or [`Error::Anonymous`].
    pub(crate) fn require_user(&self) -> Result<&str, Error> {
        self.user_id().ok_or(Error::Anonymous)
    }

    /// Sends `endpoint` and returns the reply body: [`Error::Closed`] after [`Self::shutdown`],
    /// [`Error::Anonymous`] up front for an endpoint that needs a signed-in user, else the
    /// client's result.
    pub(crate) async fn send<E: Endpoint + Sync>(
        &self,
        endpoint: &E,
    ) -> Result<E::Response, Error> {
        self.ensure_open()?;
        if E::AUTH == EndpointAuth::Required {
            self.require_user()?;
        }
        Ok(self.inner.client.send(endpoint).await?.body)
    }

    /// Console command recall, newest last.
    pub(crate) fn console_commands(&self) -> MutexGuard<'_, VecDeque<String>> {
        lock(&self.inner.commands)
    }

    /// The cache of the signed-in user's profile.
    pub(crate) fn me_cache(&self) -> &tokio::sync::Mutex<Option<Arc<AuthMeResponse>>> {
        &self.inner.me
    }
}

impl fmt::Debug for Overseer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Overseer")
            .field("server", self.inner.client.server())
            .field("status", &*self.inner.status.borrow())
            .finish_non_exhaustive()
    }
}

/// Locks a mutex whose critical sections cannot panic halfway through an update.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use crate::{
        Account, Code, Console, ConsoleEntry, Feed, History, Map, Market, Memory, Messages,
        Overseer, RoomSnapshot, Rooms, ServerProfile, Users, Watch,
    };

    /// Apps keep handles in shared state and move them and the futures of their operations
    /// into spawned tasks; checked at compile time.
    #[test]
    fn handles_and_operation_futures_are_send_and_sync() {
        fn shared<T: Send + Sync>() {}
        fn send<T: Send>(_: &T) {}
        shared::<Overseer>();
        shared::<Watch<RoomSnapshot>>();
        shared::<Feed<ConsoleEntry>>();
        shared::<(
            Account,
            Code,
            Console,
            History,
            Map,
            Market,
            Memory,
            Messages,
        )>();
        shared::<(Rooms, Users)>();
        let _: fn(ServerProfile) = |profile| send(&Overseer::connect(profile));
        let _: fn(&Overseer) = |overseer| {
            send(&overseer.shutdown());
            send(&overseer.rooms().watch("W1N1"));
            send(&overseer.console().execute("Game.time", None));
        };
        let _: fn(&mut Watch<RoomSnapshot>, &mut Feed<ConsoleEntry>) = |watch, feed| {
            send(&watch.changed());
            send(&feed.recv());
        };
    }
}
