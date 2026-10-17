//! Live feature tests: the `overseer-net` client against a real disposable private server,
//! built from `openscreeps/server` or the community Screeps server (`--target community`).
//!
//! Every test is ignored by default; `cargo xtask e2e` starts a server, sets the environment
//! (see [`support`]) and runs them with `cargo test --workspace --no-fail-fast --test live --
//! --ignored`.
//!
//! The [`read_only`] tests only read, so they also run against screeps.com with
//! `cargo xtask e2e --target official`.

mod account;
mod idle;
mod read_only;
mod rooms;
mod socket;
mod support;
