//! Live feature tests: `overseer-proxy` in front of a real disposable server built from
//! `openscreeps/server`, driven by the `overseer-net` client and raw HTTP.
//!
//! Every test is ignored by default; `cargo xtask e2e` starts a server, sets the environment
//! (see [`support`]) and runs them with `cargo test --workspace --test live -- --ignored`.

mod api;
mod site;
mod socket;
mod support;
