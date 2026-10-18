//! Overseer's domain model: the values the rest of Overseer and the app work with, whichever
//! contract the server speaks.
//!
//! Every contract ([`api::official`](crate::api::official), [`api::openscreeps`](crate::api::openscreeps),
//! [`api::community`](crate::api::community) and the [`api::plugins`](crate::api::plugins)) has
//! its own generated types. A type every contract defines identically (a component, or an inline
//! schema named after its location) is generated once, in the official module, and re-exported by
//! the others (codegen proves the identity); the model re-exports such a type as it is. Where the
//! contracts differ, the model has a hand-written type with the fields Overseer reads and a
//! `From` conversion from each contract's type. Callers use
//! these modules, never a contract module.
//!
//! One module per area:
//!
//! - [`platform`]: the server kind, its plugins, the subsystems they make up, the connection
//!   info and `GET /api/version`.
//! - [`account`]: the signed-in user's profile, notification preferences and API tokens.
//! - [`code`]: branches and code modules.
//! - [`memory`]: Memory and Memory segments.
//! - [`world`]: map, rooms, terrain, room history, room objects and the tick duration measured
//!   from game ticks.
//! - [`market`]: orders, market statistics and the credit history.
//! - [`messages`]: private messages.
//! - [`power_creeps`]: the account's power creeps.
//! - [`console`]: console expressions and output.
//! - [`users`]: other players, badges and leaderboards.
//! - [`socket`]: the socket's live payloads.

pub mod account;
pub mod code;
pub mod console;
pub mod market;
pub mod memory;
pub mod messages;
pub mod platform;
pub mod power_creeps;
pub mod socket;
pub mod users;
pub mod world;
