//! Live tests of the CLI: real `overseer` processes against a disposable private server.
//!
//! The tests are ignored by default. `cargo xtask e2e` starts a server, sets the environment
//! (see [`support`]) and runs them with all other live tests:
//! `cargo test --workspace --no-fail-fast --test live -- --ignored`. Use
//! `--target community` to run them against the community server.
//!
//! The tests change the world, so no test of this crate is in a `read_only` module. On
//! `--target official` they do not run.
//!
//! The spawn selector is `selector.js` in this directory. Node.js runs it, so `node` must be on
//! the `PATH`.

mod observer;
mod scenario;
mod support;
mod workspace;
