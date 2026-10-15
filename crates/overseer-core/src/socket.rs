//! Socket protocol: commands to send, events received, subscriptions and room state.
//!
//! One connection carries unframed text: protocol lines (`time`, `protocol`, `package`,
//! `auth ok <token>`, `auth failed`, `server down`, `cannot subscribe…`), client commands
//! (`gzip on`, `auth <token>`, `subscribe <channel>`, `unsubscribe <channel>`) and JSON channel
//! events `[channel, data]`, possibly compressed as `gz:` frames. [`SocketSession`] owns the
//! protocol state of one connection; the driver moves frames between it and the transport.
//! Event data are the [`types`] generated from the socket reference, re-exported here.

mod channel;
mod flag;
mod payload;
pub mod room;
mod session;
pub mod types;
mod visual;

pub use channel::Channel;
pub use flag::{Flag, FlagError};
pub use room::RoomState;
pub use session::{Event, Payload, SessionState, SocketError, SocketSession};
pub use types::*;
