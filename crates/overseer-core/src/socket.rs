//! Socket protocol: commands to send, events received, subscriptions and room state.
//!
//! One connection carries unframed text: protocol lines (`time`, `protocol`, `package`,
//! `auth ok <token>`, `auth failed`, `server down`, `cannot subscribe…`), client commands
//! (`gzip on`, `auth <token>`, `subscribe <channel>`, `unsubscribe <channel>`) and JSON channel
//! events `[channel, data]`, possibly compressed as `gz:` frames. [`SocketSession`] owns the
//! protocol state of one connection; the driver moves frames between it and the transport.
//!
//! Each contract has its own generated event payloads ([`official`], [`openscreeps`],
//! [`community`]). The session decodes every event with the contract of the server it is
//! connected to and hands out the [domain types](crate::model::socket) they convert into.

mod channel;
pub mod community;
mod flag;
pub mod official;
pub mod openscreeps;
pub mod room;
mod session;
mod visual;

pub use channel::Channel;
pub use flag::{Flag, FlagError};
pub use room::RoomState;
pub use session::{Event, Payload, SessionState, SocketError, SocketSession};
