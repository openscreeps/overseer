//! Sans-I/O core of the Overseer Screeps client.
//!
//! The crate turns Screeps protocol operations into values and back without performing I/O,
//! reading clocks or spawning tasks:
//!
//! - [`api::Session`] builds [`http::Request`]s for typed [`api::Endpoint`]s and interprets the
//!   replies: error envelopes delivered with HTTP 200, rotated tokens and rate limits.
//! - [`socket::SocketSession`] produces the text frames to send on the socket connection, turns
//!   received frames into typed [`socket::Event`]s and keeps subscription and room state.
//!
//! A driver owns the transport, the clock and any retry policy. The protocol follows the
//! `openscreeps/openapi` reference: `openapi.json` (HTTP), `asyncapi.json` (socket) and the
//! standalone profile for private servers, which name socket channels without a shard.

pub mod api;
pub mod gz;
pub mod server;
pub mod socket;
pub mod token;

pub use server::{ServerUrl, ServerUrlError};
pub use token::Token;
