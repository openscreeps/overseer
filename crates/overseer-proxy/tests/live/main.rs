//! Live feature tests: `overseer-proxy` in front of a real disposable private server, built
//! from `openscreeps/server` or the community Screeps server (`--target community`), driven by
//! the `overseer-net` client and raw HTTP.
//!
//! Every test is ignored by default; `cargo xtask e2e` starts a server, sets the environment
//! (see [`support`]) and runs them with `cargo test --workspace --no-fail-fast --test live --
//! --ignored`.
//!
//! The [`read_only`] tests only read, so they also run against screeps.com with
//! `cargo xtask e2e --target official`.

mod api;
mod read_only;
mod site;
mod socket;
mod support;
