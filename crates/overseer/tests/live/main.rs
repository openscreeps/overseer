//! Live feature tests: the `overseer` features against a real disposable server built from
//! `openscreeps/server`.
//!
//! Every test is ignored by default; `cargo xtask e2e` starts a server, sets the environment
//! (see [`support`]) and runs them with `cargo test --workspace --test live -- --ignored`.

mod account;
mod code;
mod console;
mod history;
mod map;
mod memory;
mod portals;
mod rooms;
mod session;
mod support;
mod world_map;
