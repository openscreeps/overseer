//! The application layer of a Screeps client: everything but the user interface.
//!
//! An app depends only on this crate. [`Overseer`] holds one account on one server: the
//! [`net::Client`] for every typed endpoint, the discovered [`net::World`] and one background
//! task owning the socket. Feature handles ([`Overseer::rooms`], [`Overseer::console`], …) group
//! the operations of each screen of the official client; live data arrives through
//! reference-counted [`Watch`]es (latest value of a state channel) and [`Feed`]s (every item of
//! an event channel). `use overseer::prelude::*;` brings the everyday names into scope.
//!
//! # Connect
//!
//! [`Overseer::connect`] builds the client from a [`ServerProfile`], discovers the world
//! (signing in with credentials), opens the socket and spawns the task that drives it. The
//! handle is cheap to clone.
//!
//! ```no_run
//! # async fn run() -> Result<(), overseer::Error> {
//! use overseer::prelude::*;
//!
//! let profile = ServerProfile {
//!     url: ServerUrl::parse("http://localhost:21025/").expect("a valid URL"),
//!     auth: Auth::Credentials { email: "player@example.com".into(), password: "secret".into() },
//!     server_password: None,
//!     default_shard: None,
//! };
//! let overseer = Overseer::connect(profile).await?;
//! let mut tokens = overseer.tokens();
//! // `rt::spawn` is `tokio::spawn` natively and the browser's event loop on the web.
//! overseer::net::rt::spawn(async move {
//!     // Persist every rotated session token.
//!     while tokens.changed().await.is_ok() {
//!         let _token = tokens.borrow_and_update().clone();
//!     }
//! });
//! println!("{:?}", *overseer.status().borrow());
//! # Ok(())
//! # }
//! ```
//!
//! # Native and web
//!
//! The crate builds natively and for browsers (`wasm32-unknown-unknown`), so one app can run on
//! desktop, mobile and the web:
//!
//! - Natively, [`Overseer::connect`] must be called inside a tokio runtime with the time driver;
//!   the background task is a tokio task, and handles and operation futures are `Send`.
//! - In browsers there is no tokio runtime: call it from a future the browser runs (an exported
//!   `async fn` of `wasm-bindgen`, `wasm_bindgen_futures::spawn_local` or
//!   [`net::rt::spawn`]). The background task runs on the browser's event loop and timers on
//!   `setTimeout`. Operation futures are not `Send` there (browser I/O is thread-bound).
//! - Browser differences: no code directory sync ([`Code::pull_dir`] and friends are
//!   native-only), the server must allow the page's origin (CORS) and expose `X-Token`, socket
//!   keep-alive pings are the browser's, and redirects, timeouts and `User-Agent` are the
//!   browser's (see [`net`]'s "Native and web"). [`ConsoleEntry::received`] is a
//!   [`net::rt::SystemTime`], [`std::time::SystemTime`] natively.
//!
//! # Live data
//!
//! Each [`Watch`] and [`Feed`] holds a reference to its socket channel: the first one subscribes,
//! the last one dropped unsubscribes, and the subscriptions survive reconnects. Both are
//! [`Stream`](futures_util::Stream)s.
//!
//! ```no_run
//! # async fn run(overseer: overseer::Overseer) -> Result<(), overseer::Error> {
//! let mut room = overseer.rooms().watch("W8N3").await?;
//! while let Ok(snapshot) = room.changed().await {
//!     println!("tick {:?}: {} objects", snapshot.game_time, snapshot.objects.len());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Console
//!
//! ```no_run
//! # async fn run(overseer: overseer::Overseer) -> Result<(), overseer::Error> {
//! let console = overseer.console();
//! let mut feed = console.feed()?;
//! console.execute("Game.time", None).await?;
//! while let Ok(entry) = feed.recv().await {
//!     println!("{:?}: {}", entry.kind, entry.plain_text());
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Notices and status
//!
//! [`Overseer::notices`] reports what an app shows outside any screen: server announcements,
//! maintenance, rejected tokens, channel errors such as the room subscription limit, undecodable
//! frames and items a slow [`Feed`] missed. [`Overseer::status`] follows the connection through
//! reconnects.
//!
//! # Features
//!
//! | Handle | Covers |
//! |---|---|
//! | [`Overseer::account`] | the signed-in account: profile, statistics, world status, respawn, CPU and resources, badge, notification settings, power creeps, API tokens |
//! | [`Overseer::console`] | live console output with history, console expressions, command recall |
//! | [`Overseer::code`] | branches, JavaScript and WebAssembly modules, activation, live code changes, local directory sync |
//! | [`Overseer::memory`] | Memory by path, segments, live path watches |
//! | [`Overseer::rooms`] | live typed room snapshots, room information and actions: flags, construction sites, intents, spawn placement |
//! | [`Overseer::history`] | room history playback |
//! | [`Overseer::map`] | world map viewport, map statistics, batch terrain, room status, map visuals |
//! | [`Overseer::messages`] | conversations, sending, read state, live new messages |
//! | [`Overseer::market`] | orders, own orders, price statistics, money history |
//! | [`Overseer::users`] | user lookup, rooms of a user, leaderboards, badges |
//!
//! Every typed endpoint remains available through [`Overseer::client`].

pub mod account;
pub mod code;
pub mod console;
mod error;
pub mod history;
mod hub;
pub mod live;
pub mod map;
pub mod market;
pub mod memory;
pub mod messages;
mod profile;
pub mod rooms;
mod session;
pub mod users;

pub use overseer_net as net;
pub use overseer_net::core;

pub use account::{Account, PowerCreeps, StartRoom};
pub use code::{Code, CodeTarget, Modules};
pub use console::{Console, ConsoleEntry, ConsoleKind};
pub use error::Error;
pub use history::{History, HistoryPlayer, Seek};
pub use live::{Closed, Feed, Watch};
pub use map::{Map, MapStat, MapView};
pub use market::Market;
pub use memory::Memory;
pub use messages::Messages;
pub use profile::{Auth, ServerProfile};
pub use rooms::{FlagColor, RoomSnapshot, Rooms, StatInterval};
pub use session::{ConnectionStatus, Notice, Overseer};
pub use users::Users;

/// The names an app uses every day: `use overseer::prelude::*;`.
///
/// Includes [`overseer_net::prelude`] (typed endpoints and types, socket names, the client);
/// [`Error`] is this crate's error.
pub mod prelude {
    pub use overseer_net::prelude::*;

    pub use crate::{
        Account, Auth, Closed, Code, ConnectionStatus, Console, ConsoleEntry, ConsoleKind, Error,
        Feed, History, Map, Market, Memory, Messages, Notice, Overseer, RoomSnapshot, Rooms,
        ServerProfile, Users, Watch,
    };
    pub use crate::{CodeTarget, Modules};
    pub use crate::{FlagColor, HistoryPlayer, MapStat, MapView, Seek, StatInterval};
}
