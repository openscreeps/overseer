//! Desktop shell of the Overseer app: a Tauri 2 webview running the app's WebAssembly UI
//! (`app/`), plus what a page cannot do itself, exposed as commands:
//!
//! - `proxy_url(upstream)`: starts (once per server) an in-process `overseer-proxy` on
//!   loopback and returns its URL; the UI connects to every server through it.
//! - `secret_get(key)`, `secret_set(key, value)`, `secret_delete(key)`: tokens and server
//!   passwords in the OS credential store (service `overseer`).
//! - External links open in the system browser through `tauri-plugin-opener`.

// No console window behind the app in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod proxies;
mod secrets;

use tauri::{Manager, RunEvent};

use crate::proxies::Proxies;

fn main() -> Result<(), tauri::Error> {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Proxies::default())
        .invoke_handler(tauri::generate_handler![
            proxies::proxy_url,
            secrets::secret_get,
            secrets::secret_set,
            secrets::secret_delete,
        ])
        .build(tauri::generate_context!())?;
    app.run(|app, event| {
        if matches!(event, RunEvent::Exit) {
            tauri::async_runtime::block_on(app.state::<Proxies>().shutdown());
        }
    });
    Ok(())
}
