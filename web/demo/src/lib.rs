//! Browser demo of `overseer` on `wasm32-unknown-unknown`.
//!
//! `index.html` asks for a server URL and credentials (or a token), then calls the exported
//! `run`: it connects with `Overseer::connect`, logs the connection status and notices, watches
//! one room (object counts by type per update), follows the console feed and runs one console
//! expression, writing every line to the page and to the browser console.
//!
//! Build the static bundle into `target/web-demo/` with `cargo xtask web-demo` (it needs the
//! `wasm-bindgen` CLI of the same version as the `wasm-bindgen` crate in `Cargo.lock`:
//! `cargo install wasm-bindgen-cli --version <that version>`) and serve that directory over
//! HTTP, for example `python -m http.server 8000 --directory target/web-demo`. The server must
//! allow the page's origin (CORS); the `openscreeps` server allows any origin. Served by
//! `overseer-proxy` (`mise run web-demo-proxy`), the page fills in the proxy's upstream and
//! reaches it through its own origin, as that server (`ServerUrl::proxied`).
//!
//! On native targets the crate is empty, so workspace commands build it.

#[cfg(target_arch = "wasm32")]
mod demo;
