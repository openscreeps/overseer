//! The saved Screeps servers, the sign-ins on them and their secrets. The Overseer app, its
//! desktop shell and the CLI share this storage.
//!
//! - [`profiles`]: the saved servers and who is signed in on them, as one JSON document. All
//!   targets.
//! - `native` (not on `wasm32`): the profiles file in the configuration directory of the app,
//!   and the secrets in the OS credential store.
//! - `web` (feature `web`): the profiles document and the secrets in the `localStorage` of a
//!   browser page.
//!
//! The profiles document holds no secrets. The tokens and server passwords are in the secret
//! store, under the keys [`profiles::token_key`] and [`profiles::server_password_key`].

mod error;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
pub mod profiles;
#[cfg(feature = "web")]
pub mod web;

pub use error::Error;
