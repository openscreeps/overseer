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
//! [`Client::world`] fetches `GET /api/version` (and `GET /api/auth/me` when authenticated)
//! once: the shards (none on single-world private servers), the server's capabilities and the
//! signed-in user. Helpers that take `shard: Option<&str>` use the world's default shard for
//! `None` ([`ClientBuilder::default_shard`], else the first shard listed) and never send one to
//! a single-world server. Every endpoint of the reference is available as a typed struct in
//! [`endpoints`](overseer_core::api::endpoints), executed with [`Client::send`].
//!
//! ```no_run
//! # async fn run(client: overseer_net::Client) -> Result<(), overseer_net::Error> {
//! use overseer_net::prelude::*;
//!
//! let world = client.world().await?;
//! println!("shards {:?}, signed in: {}", world.shards(), world.user().is_some());
//! let time = client.send(&endpoints::GameTime { shard: world.shard().map(str::to_owned) }).await?;
//! println!("tick {}", time.body.time);
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
//! state: typed objects ([`GameRoomObject`](overseer_core::api::types::GameRoomObject)), users,
//! room visuals and flags. `examples/room_view.rs` renders a room as ASCII this way. For a
//! one-off snapshot without a socket use [`Client::room_objects`].
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
//! [`Client::console`] queues an expression; its output arrives on the console channel
//! ([`Socket::subscribe_console`]). [`Client::memory`] reads and decodes Memory at a path;
//! [`Client::set_memory`] and [`Client::delete_memory`] queue writes that run in the player's
//! runtime on a later tick.
//!
//! ```no_run
//! # async fn run(client: overseer_net::Client) -> Result<(), overseer_net::Error> {
//! use overseer_net::prelude::*;
//! use overseer_net::serde_json::json;
//!
//! client.console(None, "Game.time").await?;
//! if let MemoryValue::Json(value) = client.memory(None, Some("stats")).await? {
//!     println!("{value}");
//! }
//! client.set_memory(None, "settings.paused", json!(true)).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # History
//!
//! [`Client::room_history`] fetches the room-history chunk holding a tick (official static
//! files on sharded servers, `GET /room-history` on single-world servers) and
//! [`RoomHistory::replay`] rebuilds the room tick by tick with the same diff application as
//! live updates.
//!
//! ```no_run
//! # async fn run(client: overseer_net::Client) -> Result<(), overseer_net::Error> {
//! let history = client.room_history(None, "W8N3", 1_000_050).await?;
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
//! Private servers rotate session tokens on authenticated replies (`X-Token`) and on the
//! socket's `auth ok`; both replace the client's shared token ([`Client::token`]; persist it if
//! you store tokens), so HTTP requests and new socket connections always use the latest one. A
//! client built with [`ClientBuilder::credentials`] signs in when it needs a token and has none,
//! and signs in once more when the server rejects the token (HTTP 401 or the socket's
//! `auth failed`) before retrying. Without credentials these failures are returned as
//! [`ApiError::Unauthorized`] and [`Event::AuthFailed`](overseer_core::socket::Event::AuthFailed).
//!
//! Requests rejected with HTTP 429 are retried once: after the endpoint's window resets when
//! that is within [`RateLimitPolicy::max_wait`], or after [`RateLimitPolicy::global_delay`] for
//! the server-wide limit ([`ClientBuilder::rate_limit`] configures or disables this).
//! [`Client::rate_limit`] and [`Client::rate_limits`] expose the latest limit of each endpoint
//! for display.
//!
//! No total request timeout is applied (the socket upgrade shares the client); wrap calls in
//! [`tokio::time::timeout`] where needed.

mod client;
mod history;
mod socket;
mod world;

use overseer_core::api::{ApiError, MemoryError, RequestError, TerrainError};
use overseer_core::socket::SocketError;

pub use overseer_core as core;
pub use serde_json;

pub use client::{Client, ClientBuilder, EndpointRateLimit, RateLimitPolicy};
pub use history::{HistoryReplay, RoomHistory};
pub use socket::{DisconnectReason, ReconnectPolicy, RoomView, Socket, SocketEvent};
pub use world::World;

/// The names a client application uses every day: `use overseer_net::prelude::*;`.
pub mod prelude {
    pub use futures_util::{Stream, StreamExt};
    pub use overseer_core::api::types::GameRoomObject;
    pub use overseer_core::api::{
        ApiError, Endpoint, MemoryValue, RateLimit, Reply, Terrain, Tile, endpoints, types,
    };
    pub use overseer_core::socket::{self, Channel, Event, Flag, Payload, RoomState};
    pub use overseer_core::{ServerUrl, Token};

    pub use crate::{
        Client, ClientBuilder, DisconnectReason, Error, HistoryReplay, RateLimitPolicy,
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
}

impl From<reqwest_websocket::Error> for Error {
    fn from(error: reqwest_websocket::Error) -> Self {
        Self::WebSocket(Box::new(error))
    }
}
