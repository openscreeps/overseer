//! Async Screeps client: [`overseer_core`] over [`reqwest`] and [`tokio`].
//!
//! This crate is the one dependency a client application needs: it drives the sans-I/O
//! [`overseer_core`] (re-exported as [`core`], with [`serde_json`] for JSON values) over HTTP and
//! WebSocket and adds the flows a user interface is built from. `use overseer_net::prelude::*;`
//! brings the everyday names into scope.
//!
//! # Connect
//!
//! A [`Client`] holds one account on one server. It is cheap to clone: clones share the token,
//! the sign-in state, the rate limits, the terrain cache and the discovered [`World`].
//!
//! ```no_run
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! use overseer_net::prelude::*;
//!
//! // The official server with an account API token …
//! let official = Client::builder(ServerUrl::official())
//!     .token(Token::new("…"))
//!     .build()?;
//! // … or a private server with password sign-in.
//! let private = Client::builder(ServerUrl::parse("http://localhost:21025/")?)
//!     .credentials("player@example.com", "secret")
//!     .build()?;
//! # let _ = (official, private);
//! # Ok(())
//! # }
//! ```
//!
//! # Discover
//!
//! [`Client::world`] discovers the server once: which [`Subsystem`]s it offers (password
//! sign-in, API tokens, room objects, room history, …), what its `GET /api/version` reports,
//! its shards (none on single-world servers) and, when authenticated, the signed-in user
//! (`GET /api/auth/me`). Code against the subsystems and the operations every server offers
//! alike: whether the server is screeps.com, `OpenScreeps` or a community server with plugins
//! is connection info ([`World::connection`]) for display and diagnostics, discovered by
//! default and forced with [`ClientBuilder::server_kind`] (and [`ClientBuilder::plugins`]).
//! Helpers that take `shard: Option<&str>` use the world's default shard for `None`
//! ([`ClientBuilder::default_shard`], else the first shard listed) and never send one to a
//! single-world server. [`Client::api`] returns the server's operations in Overseer's own types
//! ([`model`]), whichever contract it speaks; see [`api`]. [`Client::send`] sends one endpoint
//! of a generated contract module of [`overseer_core::api`] as is.
//!
//! [`Subsystem`]: overseer_core::model::platform::Subsystem
//!
//! ```no_run
//! # async fn run(client: overseer_net::Client) -> Result<(), overseer_net::Error> {
//! use overseer_net::prelude::*;
//!
//! let world = client.world().await?;
//! println!("shards {:?}, signed in: {}", world.shards(), world.user().is_some());
//! if world.supports(Subsystem::RoomHistory) {
//!     println!("room history in chunks of {} ticks", world.history_chunk_size());
//! }
//! let info = client.api().await?.version().await?;
//! println!("protocol {}, {} users", info.protocol, info.users);
//! # Ok(())
//! # }
//! ```
//!
//! # Subscribe
//!
//! [`Client::connect`] opens a [`Socket`]. Subscribe with its helpers (they name channels for
//! the world), then drive it with [`Socket::next`] or as a [`Stream`](futures_util::Stream) of
//! `Result<SocketEvent, Error>`. Room channels are folded into one
//! [`RoomState`](overseer_core::socket::RoomState) per room.
//!
//! ```no_run
//! # async fn run(client: overseer_net::Client) -> Result<(), overseer_net::Error> {
//! use overseer_net::prelude::*;
//!
//! let mut socket = client.connect().await?;
//! socket.subscribe_cpu()?;
//! socket.subscribe_room("W8N3");
//! // `Socket::next` returns `Result<Option<_>>`; the stream yields `Option<Result<_>>`.
//! while let Some(event) = StreamExt::next(&mut socket).await {
//!     match event? {
//!         SocketEvent::Event(Event::Channel { channel, payload }) => println!("{channel}: {payload:?}"),
//!         other => println!("{other:?}"),
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Render a room
//!
//! [`Socket::watch_room`] subscribes to a room and fetches its decoded terrain once (cached per
//! client by [`Client::terrain`]); [`Socket::room_view`] then combines the terrain with the live
//! state: typed objects ([`GameRoomObject`](overseer_core::model::world::GameRoomObject)), users,
//! room visuals and flags. `examples/room_view.rs` renders a room as ASCII this way. For a
//! one-off snapshot without a socket use
//! [`RoomObjectsSource::room_objects`](api::RoomObjectsSource::room_objects) through
//! [`Api::room_objects_source`] (screeps.com, `OpenScreeps` and community servers with
//! screepsmod-admin-utils).
//!
//! ```no_run
//! # async fn run(client: overseer_net::Client) -> Result<(), overseer_net::Error> {
//! use overseer_net::prelude::*;
//!
//! let mut socket = client.connect().await?;
//! socket.watch_room("W8N3").await?;
//! while let Some(_event) = socket.next().await? {
//!     let Some(view) = socket.room_view("W8N3") else { continue };
//!     let walls = (0..50)
//!         .flat_map(|y| (0..50).map(move |x| (x, y)))
//!         .filter(|&(x, y)| view.tile(x, y) == Some(Tile::Wall))
//!         .count();
//!     for (id, object) in view.objects() {
//!         if let Ok(object) = object {
//!             println!("{id}: {} at {},{}", object.kind(), object.x(), object.y());
//!         }
//!     }
//!     println!("{walls} walls, {} users, tick {:?}", view.users().count(), view.game_time());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Console and Memory
//!
//! [`ConsoleApi::console`](api::ConsoleApi::console) queues an expression; its output arrives
//! on the console channel ([`Socket::subscribe_console`]). [`MemoryApi`](api::MemoryApi) reads
//! and decodes Memory at a path and queues writes that run in the player's runtime on a later
//! tick.
//!
//! ```no_run
//! # async fn run(client: overseer_net::Client) -> Result<(), overseer_net::Error> {
//! use overseer_net::prelude::*;
//! use overseer_net::serde_json::json;
//!
//! let api = client.api().await?;
//! api.console("Game.time", None).await?;
//! if let MemoryValue::Json(value) = api.memory("stats", None).await? {
//!     println!("{value}");
//! }
//! api.set_memory("settings.paused", json!(true), None).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # History
//!
//! [`WorldApi::room_history`](api::WorldApi::room_history) fetches the room-history chunk
//! holding a tick (static files per shard on screeps.com, `GET /room-history` on the other
//! servers) and [`RoomHistory::replay`] rebuilds the room tick by tick with the same diff
//! application as live updates.
//!
//! ```no_run
//! # async fn run(client: overseer_net::Client) -> Result<(), overseer_net::Error> {
//! use overseer_net::prelude::*;
//!
//! let history = client.api().await?.room_history(None, "W8N3", 1_000_050).await?;
//! let mut replay = history.replay();
//! while let Some(tick) = replay.advance() {
//!     println!("tick {tick}: {} objects", replay.state().objects().count());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Reconnects, tokens and rate limits
//!
//! When the connection fails, the server closes it or keep-alive pings go unanswered, the
//! socket reconnects with exponential backoff ([`ReconnectPolicy`]), reports it as
//! [`SocketEvent::Disconnected`] and [`SocketEvent::Reconnecting`], and resubscribes every
//! channel; room states start over with the next update.
//!
//! The socket upgrade shares the HTTP client's connection pool, so it may land on a keep-alive
//! connection the server has just closed for idling. Natively, [`Client::connect`] and every
//! reconnect therefore send an upgrade whose connection is reset or closed before any response
//! arrives once more, on a new connection; an answer of the server (any HTTP status) is never
//! retried. Browsers open a new connection for every `WebSocket` and get no such retry.
//!
//! Private servers rotate session tokens on authenticated replies (`X-Token`) and on the
//! socket's `auth ok`; both replace the client's shared token ([`Client::token`]; persist every
//! value of [`Client::token_updates`] if you store tokens), so HTTP requests and new socket
//! connections always use the latest one. Their tokens expire a minute (by default) after they
//! were issued, so an idle client must send an authenticated request now and then to keep one;
//! [`Client::rotates_tokens`] tells whether the server rotated the token. A client built with
//! [`ClientBuilder::credentials`] signs in when it needs a token and has none, and signs in once
//! more when the server rejects the token (HTTP 401 or the socket's `auth failed`) before
//! retrying. Without credentials these failures are returned as [`ApiError::Unauthorized`] and
//! [`Event::AuthFailed`](overseer_core::socket::Event::AuthFailed).
//!
//! Requests rejected with HTTP 429 are retried once: after the endpoint's window resets when
//! that is within [`RateLimitPolicy::max_wait`], or after [`RateLimitPolicy::global_delay`] for
//! the server-wide limit ([`ClientBuilder::rate_limit`] configures or disables this).
//! [`Client::rate_limit`] and [`Client::rate_limits`] expose the latest limit of each endpoint
//! for display; [`Client::forget_rate_limit`] drops one known to be stale (lifted by the user
//! on screeps.com's reset page) until the next response reports it again.
//!
//! No total request timeout is applied (the socket upgrade shares the client); wrap calls in
//! [`rt::timeout`] where needed.
//!
//! # Failed operations
//!
//! [`Error::outcome`] tells whether a failed operation can have changed state on the server.
//! [`Outcome::NotApplied`] means that no attempt of the request left the client, sign-ins and
//! retries included. Every other failure is [`Outcome::Unknown`]: the server can have applied
//! the operation in full, in part or not at all. The outcome causes no retries: the application
//! decides whether to read the state, wait or try again.
//!
//! # Native and web
//!
//! The crate builds natively and for browsers (`wasm32-unknown-unknown`); [`rt`] provides the
//! clocks, timers and tasks of either. Natively it runs inside a tokio runtime (with the time
//! driver) and its futures and handles are `Send`. In browsers it runs on the browser's event
//! loop, its futures are not `Send`, and the browser owns part of the transport:
//!
//! - HTTP uses `fetch`: redirects (followed), timeouts, proxies and TLS are the browser's, so
//!   the native client's options (HTTP/1.1 only, no redirects, 10 s connect timeout) do not
//!   apply, and `User-Agent` is not sent ([`ClientBuilder::user_agent`] is ignored; browsers
//!   forbid it). The server must allow the page's origin (CORS) and expose `X-Token`, or
//!   rotated session tokens are lost; response headers it does not expose (such as
//!   `X-RateLimit-*`) are invisible, so [`Client::rate_limits`] stays empty and HTTP 429 is
//!   retried as the server-wide limit. Private servers built from `openscreeps/server` allow
//!   every origin and expose `X-Token`. The screeps.com HTTP API sends no CORS headers, so
//!   browsers block it (its socket accepts any origin): a web build reaches screeps.com through
//!   a same-origin proxy.
//! - The socket uses the browser's `WebSocket`, which answers protocol pings itself but cannot
//!   send them: [`ClientBuilder::keep_alive`] is ignored and a dead connection is noticed when
//!   the browser reports it closed. Reconnects, resubscriptions and token sharing work the same.

pub mod api;
mod client;
mod history;
pub mod rt;
mod socket;
mod world;

use overseer_core::api::{ApiError, Refusal, RequestError};
use overseer_core::model::memory::MemoryError;
use overseer_core::model::platform::Subsystem;
use overseer_core::model::world::TerrainError;
use overseer_core::socket::SocketError;

pub use overseer_core as core;
pub use overseer_core::model;
pub use serde_json;

pub use api::Api;
pub use client::{
    Client, ClientBuilder, EndpointRateLimit, GLOBAL_RATE_LIMIT_WAIT, Priority, REQUEST_BUDGET,
    REQUEST_WINDOW, RateLimitPolicy,
};
pub use history::{HistoryReplay, RoomHistory};
pub use socket::{DisconnectReason, ReconnectPolicy, RoomView, Socket, SocketEvent};
pub use world::World;

/// The names a client application uses every day: `use overseer_net::prelude::*;`.
pub mod prelude {
    pub use futures_util::{Stream, StreamExt};
    pub use overseer_core::api::{ApiError, Endpoint, RateLimit, Reply};
    pub use overseer_core::model::memory::MemoryValue;
    pub use overseer_core::model::platform::{Subsystem, Subsystems};
    pub use overseer_core::model::world::{GameRoomObject, Terrain, Tile};
    pub use overseer_core::socket::{self, Channel, Event, Flag, Payload, RoomState};
    pub use overseer_core::{ServerUrl, Token};

    pub use crate::api::{
        AccountApi as _, CodeApi as _, ConsoleApi as _, MarketApi as _, MemoryApi as _,
        MessagesApi as _, PlatformApi as _, PowerCreepsApi as _, RoomObjectsSource as _,
        UsersApi as _, WorldApi as _,
    };
    pub use crate::{
        Api, Client, ClientBuilder, DisconnectReason, Error, HistoryReplay, RateLimitPolicy,
        ReconnectPolicy, RoomHistory, RoomView, Socket, SocketEvent, World,
    };
}

/// User agent sent with every request unless [`ClientBuilder::user_agent`] replaces it.
pub const USER_AGENT: &str = concat!("overseer/", env!("CARGO_PKG_VERSION"));

/// A transport or protocol failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The endpoint request could not be built.
    #[error(transparent)]
    Request(#[from] RequestError),
    /// The server answered with an error.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The HTTP exchange failed (connection, TLS, body).
    #[error("HTTP transport failed")]
    Http(#[from] reqwest::Error),
    /// The socket upgrade or connection failed.
    #[error("socket transport failed")]
    WebSocket(#[source] Box<reqwest_websocket::Error>),
    /// A received socket frame could not be decoded; the session state is intact.
    #[error(transparent)]
    Socket(#[from] SocketError),
    /// A user channel was requested, but the [`World`] has no signed-in user.
    #[error("the channel needs a signed-in user")]
    Anonymous,
    /// A terrain reply is not 2500 terrain digits.
    #[error(transparent)]
    Terrain(#[from] TerrainError),
    /// A Memory value does not decode.
    #[error(transparent)]
    Memory(#[from] MemoryError),
    /// A reply decoded but lacks what the helper needs.
    #[error("unexpected reply: {0}")]
    UnexpectedReply(&'static str),
    /// The server does not offer the operation's subsystem (a plugin it does not run, or one
    /// its contract lacks), for example password sign-in. Nothing was sent.
    #[error("the server does not offer {0}")]
    Unsupported(Subsystem),
    /// A password change was refused: the current password given is not the account's.
    #[error("the current password is wrong")]
    WrongPassword,
    /// The server answered the request with HTTP 401 or 429 (`answer`), and the retry then
    /// failed before it sent anything more (`source`): the sign-in that the retry needs, or the
    /// retry itself, did not leave the client. [`Client::send`] returns it in place of `source`,
    /// because the first attempt reached the server.
    #[error("{answer}; the retry was not sent")]
    RetryNotSent {
        /// The answer to the first attempt: [`ApiError::Unauthorized`] or
        /// [`ApiError::RateLimited`].
        answer: ApiError,
        /// Why the retry was not sent.
        #[source]
        source: Box<Error>,
    },
}

impl Error {
    /// The server's own error string ([`ApiError::server_message`]), when it sent one.
    #[must_use]
    pub fn server_message(&self) -> Option<&str> {
        match self {
            Self::Api(error) => error.server_message(),
            _ => None,
        }
    }

    /// Why the server refused the client's access ([`ApiError::refusal`]), when it did.
    #[must_use]
    pub fn refusal(&self) -> Option<Refusal> {
        match self {
            Self::Api(error) => error.refusal(),
            _ => None,
        }
    }

    /// What the failed operation can have done on the server. The value covers every attempt
    /// of the operation: the sign-in before the request, the retry after HTTP 401 and the retry
    /// after HTTP 429 ([`Client::send`]).
    ///
    /// It is [`Outcome::NotApplied`] only when no attempt of the request left the client:
    ///
    /// - [`Self::Request`], [`Self::Unsupported`] and [`Self::Anonymous`]: the client did not
    ///   build the request, or the server does not offer the operation.
    /// - [`Self::Http`] when `reqwest` could not build the request
    ///   ([`reqwest::Error::is_builder`]) or, natively, could not connect
    ///   (`reqwest::Error::is_connect`): no byte of the request was written.
    ///
    /// Every other error is [`Outcome::Unknown`]. This includes:
    ///
    /// - Every answer of the server. No contract states that the server refuses HTTP 401 or 429
    ///   before it runs the request. An `{error}` reply is no proof either: community servers
    ///   also report exceptions that a request handler throws halfway this way.
    /// - A timeout, a connection lost after the request was written, and a reply that does not
    ///   decode.
    /// - In browsers, every failure of `fetch`: the browser reports a network error and a reply
    ///   that CORS blocks alike.
    ///
    /// An operation of the domain API ([`Api`]) can read first, for example the discovery, the
    /// profile or a free name. [`Outcome::NotApplied`] then tells only that its change was not
    /// sent. Session upkeep is not part of the operation: a sign-in of the client and a token
    /// that a reply rotates change server state that the operation does not own.
    ///
    /// Caution: with an HTTP client from [`ClientBuilder::http_client`], a failed connection
    /// proves nothing if that client resends requests that the server can have received. The
    /// default `reqwest` client resends only requests that an HTTP/2 server refused before it
    /// processed them.
    #[must_use]
    pub fn outcome(&self) -> Outcome {
        match self {
            Self::Request(_) | Self::Unsupported(_) | Self::Anonymous => Outcome::NotApplied,
            Self::Http(error) if unsent(error) => Outcome::NotApplied,
            Self::Http(_)
            | Self::Api(_)
            | Self::WebSocket(_)
            | Self::Socket(_)
            | Self::Terrain(_)
            | Self::Memory(_)
            | Self::UnexpectedReply(_)
            | Self::WrongPassword
            | Self::RetryNotSent { .. } => Outcome::Unknown,
        }
    }

    /// `self` as the failure of a retry after the server answered `answer`: wrapped in
    /// [`Self::RetryNotSent`] when `self` alone reports [`Outcome::NotApplied`].
    pub(crate) fn after_answer(self, answer: ApiError) -> Self {
        match self.outcome() {
            Outcome::NotApplied => Self::RetryNotSent {
                answer,
                source: Box::new(self),
            },
            Outcome::Unknown => self,
        }
    }
}

impl From<reqwest_websocket::Error> for Error {
    fn from(error: reqwest_websocket::Error) -> Self {
        Self::WebSocket(Box::new(error))
    }
}

/// What a failed operation can have done on the server ([`Error::outcome`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The server did not apply the operation: no attempt of its request left the client.
    NotApplied,
    /// The server can have applied the operation in full, in part or not at all.
    Unknown,
}

/// Whether `reqwest` failed before it wrote a byte of the request: it could not build the
/// request or connect.
#[cfg(not(target_arch = "wasm32"))]
fn unsent(error: &reqwest::Error) -> bool {
    error.is_builder() || error.is_connect()
}

/// Whether `reqwest` failed before it called `fetch`: it could not build the request. A failed
/// `fetch` tells nothing, as the browser reports a lost connection and a blocked reply alike.
#[cfg(target_arch = "wasm32")]
fn unsent(error: &reqwest::Error) -> bool {
    error.is_builder()
}
