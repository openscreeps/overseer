//! Desktop shell of the Overseer app: a Tauri 2 webview running the app's WebAssembly UI
//! (`app/`), plus what a page cannot do itself, exposed as commands:
//!
//! - `proxy_url(upstream)`: starts (once per server) an in-process `overseer-proxy` on
//!   loopback and returns its URL; the UI connects to every server through it.
//! - `secret_get(key)`, `secret_set(key, value)`, `secret_delete(key)`: tokens and server
//!   passwords in the OS credential store (service `overseer`; builds with another app
//!   identifier use their own service).
//! - `profiles_load(legacy)`, `profiles_change(change)`: the saved servers in the profiles file
//!   that the CLI (`overseer servers`) shares. The shell applies each change under the lock of
//!   the file and returns the document of the file.
//! - `open_window(route, title)`: the app at a route in a window of its own (a popped-out
//!   room), one window per route.
//! - External links open in the system browser through `tauri-plugin-opener`.
//!
//! `tauri-plugin-window-state` saves the size, position and maximized state of every window
//! when it moves, resizes or closes and restores them when a window with the same label opens.
//!
//! `tauri.e2e.conf.json` is the overlay of the test build that `cargo xtask e2e --desktop`
//! drives: another identifier (its own data and credentials) and window controls for the tests.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod profiles;
mod proxies;
mod secrets;
mod windows;

use overseer_storage::native::Storage;
use tauri::{AppHandle, Manager, RunEvent, Runtime};

use crate::proxies::Proxies;

/// The storage of `app`: the profiles file and the secrets of its identifier.
fn storage<R: Runtime>(app: &AppHandle<R>) -> Result<Storage, String> {
    Storage::open(&app.config().identifier).map_err(|error| error.to_string())
}

fn main() -> Result<(), tauri::Error> {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(Proxies::default())
        .invoke_handler(tauri::generate_handler![
            proxies::proxy_url,
            profiles::profiles_load,
            profiles::profiles_change,
            secrets::secret_get,
            secrets::secret_set,
            secrets::secret_delete,
            windows::open_window,
        ])
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        if matches!(event, RunEvent::Exit) {
            tauri::async_runtime::block_on(app.state::<Proxies>().shutdown());
        }
    });
    Ok(())
}
