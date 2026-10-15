//! Overseer, a Screeps client: the user interface, compiled to WebAssembly and run in a
//! browser or in the Tauri shell's webview (`app/src-tauri`).
//!
//! The UI is a Leptos client-side app; sessions, live data and every API call come from the
//! `overseer` library running in the page, rooms are drawn by `overseer-view` into a canvas.
//! [`platform`] hides where it runs: the Tauri shell (proxy, OS keychain, external links) or a
//! plain browser (direct connections or the page's own `overseer-proxy`, `localStorage`).
//!
//! Natively the crate only builds for the workspace checks and unit tests.
//!
//! # Build and run
//!
//! Needs `trunk` and `tauri-cli` (`cargo binstall trunk tauri-cli`):
//!
//! - `mise run app-dev`: the desktop app with the trunk dev server (`cargo tauri dev`).
//! - `mise run app-build`: the desktop app and its installers (`cargo tauri build`).
//! - `mise run app-web`: the web build into `app/dist/`. Private servers that allow other
//!   origins (an `openscreeps/server` instance does) work from any static server; screeps.com
//!   needs the page served by the proxy:
//!   `cargo run -p overseer-proxy -- --upstream https://screeps.com/ --static app/dist`.

mod connect;
mod levels;
mod platform;
mod profiles;
mod register;
mod session;
mod state;
mod ui;

fn main() {
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();
    wasm_bindgen_futures::spawn_local(async {
        let platform = platform::Platform::detect().await;
        leptos::mount::mount_to_body(move || ui::App(ui::AppProps { platform }));
    });
}
