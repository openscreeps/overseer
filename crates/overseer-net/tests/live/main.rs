//! Live feature tests: the `overseer-net` client against a real disposable server built from
//! `openscreeps/server`.
//!
//! Every test is ignored by default; `cargo xtask e2e` starts a server, sets the environment
//! (see [`support`]) and runs them with `cargo test --workspace --test live -- --ignored`.

mod account;
mod idle;
mod rooms;
mod socket;
mod support;
