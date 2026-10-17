//! The domain API: one generated client per contract behind Overseer's own types.
//!
//! A server speaks one contract ([`ServerKind`]): the official one (screeps.com), the
//! `OpenScreeps` one, or the community one (the stock Screeps server), which plugins extend
//! ([`Plugins`]). [`Client::discover`] decides which (or [`ClientBuilder::server_kind`] forces
//! it), and [`Client::api`] returns the matching [`Api`]. Its operations take and return the
//! types of [`overseer_core::model`] and fail with [`Error`](crate::Error), whatever the
//! contract: each per-contract API ([`Official`], [`OpenScreeps`], [`Community`]) sends its
//! contract's endpoints and converts the replies, and [`Api`] dispatches to the one the server
//! speaks (statically, through an enum).
//!
//! Callers stay generic: they never match on the [`Api`] variants or branch on the kind, but
//! ask which [`Subsystem`]s the server offers ([`Api::subsystems`], [`Api::supports`], or
//! [`World::subsystems`](crate::World::subsystems)). Operations only some servers offer are
//! reached through subsystem traits ([`PasswordSignIn`], [`PasswordAccount`], [`ApiTokens`],
//! [`RoomObjectsSource`]) and their accessors ([`Api::password_sign_in`],
//! [`Api::password_account`], [`Api::api_tokens`], [`Api::room_objects_source`]), which answer
//! `None` when the server lacks the subsystem; where a subsystem's flow differs by server, it
//! says so as data ([`Api::rate_limit_lift`]: a web page or a request; [`Api::account_website`];
//! [`PasswordSignIn::accepts_username`]). An area operation of an absent subsystem
//! ([`WorldApi::room_decorations`], [`WorldApi::room_history`], …) fails with
//! [`Error::Unsupported`](crate::Error::Unsupported) without a request.
//!
//! The operations are grouped by area, one trait per area: [`PlatformApi`] (`GET /api/version`),
//! [`AccountApi`] (the signed-in user's profile and preferences), [`CodeApi`], [`MemoryApi`],
//! [`ConsoleApi`], [`MarketApi`], [`MessagesApi`], [`PowerCreepsApi`], [`UsersApi`] and
//! [`WorldApi`] (terrain, rooms, the map, the game tick, room history and room actions). Bring
//! the trait into scope to call its operations: `use overseer_net::api::AccountApi as _;`.
//!
//! [`ClientBuilder::server_kind`]: crate::ClientBuilder::server_kind
//!
//! # Adding an area
//!
//! An area (code, Memory, world, market, messages, power creeps, console, users, socket) is
//! added in its own files, without touching the others:
//!
//! 1. **Domain types** in `overseer_core::model::<area>`. A type every contract defines
//!    identically (a component or an inline schema; plugin types too) is generated once, in
//!    `api::official::types`, and re-exported by the other contract modules (codegen proves them
//!    identical): `pub use` it from `crate::api::official::types` with a doc comment saying so,
//!    and never convert it per contract (two `From` impls for one type conflict). Where the
//!    contracts differ, hand-write a type with the fields Overseer and the app read, plus a
//!    `From` conversion from each contract's type (a small `macro_rules!` when the conversions
//!    are field-for-field alike), and test each conversion with a real reply.
//! 2. **The area trait** in `overseer_net::api::<area>` (`src/api/<area>.rs`, declared and
//!    re-exported here): `fn op(&self, …) -> impl Future<Output = Result<T, Error>>` per
//!    operation, documented with its endpoint and errors.
//! 3. **Delegation**: `impl <Area>Api for Api`, each method `delegate!(self, api =>
//!    api.op(…).await)` (the crate's `delegate!` macro).
//! 4. **Per-contract impls** in the same file: `impl <Area>Api for Official`, `… for
//!    OpenScreeps`, `… for Community`, each sending its own contract's endpoints
//!    (`overseer_core::api::{official, openscreeps, community}::endpoints`) through
//!    `self.client` and converting the replies. [`Client::send`] signs in as needed; an
//!    operation the sign-in itself needs (no token taken) uses `Client::send_without_sign_in`,
//!    which avoids recursing into the sign-in. An operation of a [`Subsystem`] the server
//!    lacks fails with [`Error::Unsupported`](crate::Error::Unsupported) and sends nothing.
//! 5. **Subsystem traits** for operations a plugin provides (community) or only some
//!    contracts offer: a [`Subsystem`] variant (in `overseer_core::model::platform`, offered
//!    per kind and plugins by `Subsystems::of`), a trait (`PasswordAccount`), an enum of the
//!    per-contract APIs offering it (`PasswordAccountApi`, implementing the trait by
//!    delegation) and an accessor on [`Api`] (`Api::password_account`) returning it when the
//!    server has the subsystem. Plugin operations go through the plugin's API
//!    ([`ScreepsmodAuth`], or a new one next to it for another plugin, obtained from
//!    [`Community`] only when the plugin was detected) and the plugin's generated module
//!    (`overseer_core::api::plugins::<plugin>`). Detect a new plugin in [`Client::discover`]
//!    (`api/platform.rs`) as its contract says and add it to [`Plugins`].
//! 6. **Migrate the callers** in `overseer` and the app (and the client flows of this crate)
//!    onto the trait and the model types, so no public signature keeps a type of a contract
//!    module and no caller branches on the server kind, and update their tests.

mod account;
mod code;
mod console;
mod market;
mod memory;
mod messages;
mod platform;
mod power_creeps;
mod users;
mod world;

use overseer_core::model::platform::{Plugins, ServerKind, Subsystem, Subsystems};

pub use account::{
    AccountApi, AccountWebsite, ApiTokens, ApiTokensApi, LIFT_TOKEN_PREFIX, LiftPage, LiftRequest,
    PasswordAccount, PasswordAccountApi, PasswordSignIn, PasswordSignInApi, RateLimitLift,
};
pub use code::CodeApi;
pub use console::ConsoleApi;
pub use market::MarketApi;
pub use memory::MemoryApi;
pub use messages::MessagesApi;
pub use platform::PlatformApi;
pub(crate) use platform::discover;
pub use power_creeps::PowerCreepsApi;
pub use users::UsersApi;
pub use world::{RoomObjectsSource, RoomObjectsSourceApi, WorldApi, WorldOperation};

use crate::Client;

/// The operations of the server, whichever contract it speaks; from [`Client::api`]. Cheap to
/// clone (it holds a [`Client`]).
///
/// The variants are the static dispatch to the contract's client: callers use the area traits
/// and the subsystem accessors, never the variant, to decide what to do.
#[derive(Clone, Debug)]
pub enum Api {
    /// screeps.com and its seasonal world.
    Official(Official),
    /// An `OpenScreeps` server.
    OpenScreeps(OpenScreeps),
    /// The stock Screeps server, possibly with plugins.
    Community(Community),
}

/// The official contract's operations (screeps.com).
#[derive(Clone, Debug)]
pub struct Official {
    client: Client,
}

/// The `OpenScreeps` contract's operations.
#[derive(Clone, Debug)]
pub struct OpenScreeps {
    client: Client,
}

/// The community contract's operations (the stock Screeps server), and those of the plugins it
/// runs.
#[derive(Clone, Debug)]
pub struct Community {
    client: Client,
    plugins: Plugins,
}

/// The operations screepsmod-auth adds to a community server: password sign-in, registration,
/// password changes, API tokens and lifting their rate limits. Reached through the subsystem
/// accessors of [`Api`] on a community server running it.
#[derive(Clone, Copy, Debug)]
pub struct ScreepsmodAuth<'a> {
    client: &'a Client,
}

/// The operations screepsmod-admin-utils adds to or adapts on a community server: room
/// objects, the first spawn and the start room. Reached through [`Api::room_objects_source`]
/// on a community server running it.
#[derive(Clone, Copy, Debug)]
pub struct ScreepsmodAdminUtils<'a> {
    client: &'a Client,
}

impl Api {
    /// The API of `client`'s server, which speaks `kind`'s contract and, on a community server,
    /// runs `plugins` (ignored for the other kinds). [`Client::api`] uses the discovered (or
    /// forced) kind and plugins.
    #[must_use]
    pub(crate) const fn new(client: Client, kind: ServerKind, plugins: Plugins) -> Self {
        match kind {
            ServerKind::Official => Self::Official(Official { client }),
            ServerKind::OpenScreeps => Self::OpenScreeps(OpenScreeps { client }),
            ServerKind::Community => Self::Community(Community { client, plugins }),
        }
    }

    /// The subsystems the server offers ([`Subsystems::of`] its kind and plugins); the same as
    /// [`World::subsystems`](crate::World::subsystems).
    #[must_use]
    pub const fn subsystems(&self) -> Subsystems {
        match self {
            Self::Official(_) => Subsystems::of(ServerKind::Official, Plugins::NONE),
            Self::OpenScreeps(_) => Subsystems::of(ServerKind::OpenScreeps, Plugins::NONE),
            Self::Community(api) => Subsystems::of(ServerKind::Community, api.plugins),
        }
    }

    /// Whether the server offers `subsystem`.
    #[must_use]
    pub const fn supports(&self, subsystem: Subsystem) -> bool {
        self.subsystems().contains(subsystem)
    }

    /// The transport.
    #[must_use]
    pub const fn client(&self) -> &Client {
        match self {
            Self::Official(api) => &api.client,
            Self::OpenScreeps(api) => &api.client,
            Self::Community(api) => &api.client,
        }
    }
}

impl Community {
    /// screepsmod-auth's operations, when the server runs it.
    pub(crate) const fn screepsmod_auth(&self) -> Option<ScreepsmodAuth<'_>> {
        if self.plugins.screepsmod_auth {
            Some(ScreepsmodAuth {
                client: &self.client,
            })
        } else {
            None
        }
    }

    /// screepsmod-admin-utils' operations, when the server runs it.
    pub(crate) const fn screepsmod_admin_utils(&self) -> Option<ScreepsmodAdminUtils<'_>> {
        if self.plugins.screepsmod_admin_utils {
            Some(ScreepsmodAdminUtils {
                client: &self.client,
            })
        } else {
            None
        }
    }
}

/// Evaluates `$call` with `$api` bound to the per-contract API of `$self` (an [`Api`]): the body
/// of every area trait method of [`Api`], for example
/// `delegate!(self, api => api.me().await)`.
macro_rules! delegate {
    ($self:expr, $api:ident => $call:expr) => {
        match $self {
            $crate::api::Api::Official($api) => $call,
            $crate::api::Api::OpenScreeps($api) => $call,
            $crate::api::Api::Community($api) => $call,
        }
    };
}

pub(crate) use delegate;
