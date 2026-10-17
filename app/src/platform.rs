//! Where the app runs: the Tauri shell or a plain browser.
//!
//! | | Tauri shell | Browser |
//! |---|---|---|
//! | Servers | through the shell's proxy (`proxy_url`) | directly, or the page's own `overseer-proxy` ([`crate::connect`]) |
//! | Secrets (tokens, server passwords) | OS credential store (`secret_*` commands) | `localStorage` under `overseer.secret.<key>`, readable by anything running on the page's origin |
//! | External links | system browser (`tauri-plugin-opener`) | new tab |
//! | screeps.com's rate-limit reset page ([`Platform::open_reset_page`]) | system browser; the app rechecks when its window regains focus | popup that keeps this page as its opener; the app resumes on the page's success message |
//! | Popped-out rooms | a native window per route (`open_window`, size and position remembered) | `window.open` popup per route |
//!
//! Saved profiles ([`crate::profiles`]) and state ([`crate::saved`]) are in `localStorage`
//! on both, the terrain cache ([`crate::terrain_cache`]) in `IndexedDB`.

use js_sys::{Function, Object, Promise, Reflect};
use overseer::core::ServerUrl;
use url::Url;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::connect::{ConnectTarget, PageProxy, connect_target};
use crate::profiles::{self, Profiles};
use crate::saved::{self, Saved};

/// `localStorage` prefix of secrets in browsers.
const SECRET_PREFIX: &str = "overseer.secret.";

/// The platform the page runs on.
#[derive(Clone, Debug)]
pub struct Platform {
    /// Inside the Tauri shell (`window.__TAURI__` exists).
    tauri: bool,
    /// The `overseer-proxy` serving the page, if any.
    page_proxy: Option<PageProxy>,
}

impl Platform {
    /// Detects the platform; in a browser asks the page's origin whether it is an
    /// `overseer-proxy` (`GET /overseer-proxy.json`).
    pub async fn detect() -> Self {
        let tauri = tauri_core().is_some();
        let page_proxy = if tauri { None } else { page_proxy().await };
        Self { tauri, page_proxy }
    }

    /// Inside the Tauri shell.
    pub const fn is_tauri(&self) -> bool {
        self.tauri
    }

    /// The address to connect to for the server with base URL `server`: through a proxy, the
    /// proxy's address, still known as `server` ([`ServerUrl::proxied`]).
    ///
    /// # Errors
    ///
    /// Returns a message when the URL is invalid or the shell cannot start its proxy.
    pub async fn server_url(&self, server: &str) -> Result<ServerUrl, String> {
        let url = Url::parse(server).map_err(|error| format!("Invalid server URL: {error}"))?;
        let upstream = ServerUrl::parse(server).map_err(|error| error.to_string())?;
        let proxy = match connect_target(self.tauri, self.page_proxy.as_ref(), &url) {
            ConnectTarget::TauriProxy => {
                let args = object(&[("upstream", server)]);
                invoke("proxy_url", &args)
                    .await?
                    .as_string()
                    .ok_or_else(|| "proxy_url returned no URL".to_owned())?
            }
            ConnectTarget::PageOrigin(url) => url.into(),
            ConnectTarget::Direct(_) => return Ok(upstream),
        };
        ServerUrl::proxied(&proxy, &upstream).map_err(|error| error.to_string())
    }

    /// The secret stored under `key`.
    ///
    /// # Errors
    ///
    /// Returns a message when the secret store fails.
    pub async fn secret_get(&self, key: &str) -> Result<Option<String>, String> {
        if self.tauri {
            Ok(invoke("secret_get", &object(&[("key", key)]))
                .await?
                .as_string())
        } else {
            Ok(local_storage().and_then(|storage| {
                storage
                    .get_item(&format!("{SECRET_PREFIX}{key}"))
                    .ok()
                    .flatten()
            }))
        }
    }

    /// Stores `value` under `key`.
    ///
    /// # Errors
    ///
    /// Returns a message when the secret store fails.
    pub async fn secret_set(&self, key: &str, value: &str) -> Result<(), String> {
        if self.tauri {
            invoke("secret_set", &object(&[("key", key), ("value", value)])).await?;
            Ok(())
        } else {
            local_storage()
                .ok_or_else(|| "localStorage is unavailable".to_owned())?
                .set_item(&format!("{SECRET_PREFIX}{key}"), value)
                .map_err(|error| js_message(&error))
        }
    }

    /// Deletes the secret under `key`.
    ///
    /// # Errors
    ///
    /// Returns a message when the secret store fails.
    pub async fn secret_delete(&self, key: &str) -> Result<(), String> {
        if self.tauri {
            invoke("secret_delete", &object(&[("key", key)])).await?;
            Ok(())
        } else {
            local_storage()
                .ok_or_else(|| "localStorage is unavailable".to_owned())?
                .remove_item(&format!("{SECRET_PREFIX}{key}"))
                .map_err(|error| js_message(&error))
        }
    }

    /// Shows the app at `route` (a URL hash) in a window of its own titled `title`: a native
    /// window in the shell, a popup in a browser (the same route reuses its window).
    ///
    /// # Errors
    ///
    /// Returns a message when the window cannot be opened (a blocked popup).
    pub async fn open_window(&self, route: &str, title: &str) -> Result<(), String> {
        if self.tauri {
            invoke(
                "open_window",
                &object(&[("route", route), ("title", title)]),
            )
            .await?;
            return Ok(());
        }
        let window = web_sys::window().ok_or_else(|| "no window".to_owned())?;
        let location = window.location();
        let page = format!(
            "{}{}",
            location.origin().map_err(|error| js_message(&error))?,
            location.pathname().map_err(|error| js_message(&error))?
        );
        let name: String = route
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let opened = window
            .open_with_url_and_target_and_features(
                &format!("{page}{route}"),
                &format!("overseer{name}"),
                "popup,width=640,height=640",
            )
            .map_err(|error| js_message(&error))?;
        match opened {
            Some(opened) => {
                let _ = opened.focus();
                Ok(())
            }
            None => Err("the browser blocked the window; allow popups for this page".to_owned()),
        }
    }

    /// Opens `url` outside the app: the system browser in the shell, a new tab otherwise.
    ///
    /// # Errors
    ///
    /// Returns a message when it cannot be opened.
    pub async fn open_external(&self, url: &str) -> Result<(), String> {
        if self.tauri {
            invoke("plugin:opener|open_url", &object(&[("url", url)])).await?;
            return Ok(());
        }
        let window = web_sys::window().ok_or_else(|| "no window".to_owned())?;
        window
            .open_with_url_and_target_and_features(url, "_blank", "noopener,noreferrer")
            .map(drop)
            .map_err(|error| js_message(&error))
    }

    /// Opens screeps.com's rate-limit reset page `url` ([`crate::rate_limits::reset_url`]):
    /// in the shell, the system browser (the page cannot reach the app; recheck when the
    /// window regains focus); in a browser, a popup that keeps this page as its opener, so
    /// the page's success message reaches this window.
    ///
    /// # Errors
    ///
    /// Returns a message when it cannot be opened (a blocked popup).
    pub async fn open_reset_page(&self, url: &str) -> Result<(), String> {
        if self.tauri {
            return self.open_external(url).await;
        }
        let window = web_sys::window().ok_or_else(|| "no window".to_owned())?;
        let opened = window
            .open_with_url_and_target_and_features(
                url,
                "overseer_rate_limit_reset",
                "popup,width=520,height=720",
            )
            .map_err(|error| js_message(&error))?;
        match opened {
            Some(opened) => {
                let _ = opened.focus();
                Ok(())
            }
            None => Err("the browser blocked the window; allow popups for this page".to_owned()),
        }
    }
}

/// The saved profiles.
pub fn load_profiles() -> Profiles {
    let json =
        local_storage().and_then(|storage| storage.get_item(profiles::STORAGE_KEY).ok().flatten());
    Profiles::parse(json.as_deref())
}

/// The saved state.
pub fn load_saved() -> Saved {
    let json =
        local_storage().and_then(|storage| storage.get_item(saved::STORAGE_KEY).ok().flatten());
    Saved::parse(json.as_deref())
}

/// Saves the state.
///
/// # Errors
///
/// Returns a message when `localStorage` is unavailable or full.
pub fn save_saved(saved: &Saved) -> Result<(), String> {
    local_storage()
        .ok_or_else(|| "localStorage is unavailable".to_owned())?
        .set_item(saved::STORAGE_KEY, &saved.to_json())
        .map_err(|error| js_message(&error))
}

/// The `localStorage` item `key` (documents of their own, such as the console's command
/// history).
pub fn load_item(key: &str) -> Option<String> {
    local_storage().and_then(|storage| storage.get_item(key).ok().flatten())
}

/// Saves the `localStorage` item `key`.
///
/// # Errors
///
/// Returns a message when `localStorage` is unavailable or full.
pub fn save_item(key: &str, value: &str) -> Result<(), String> {
    local_storage()
        .ok_or_else(|| "localStorage is unavailable".to_owned())?
        .set_item(key, value)
        .map_err(|error| js_message(&error))
}

/// The page's URL hash (`#…`, or empty).
pub fn location_hash() -> String {
    web_sys::window()
        .and_then(|window| window.location().hash().ok())
        .unwrap_or_default()
}

/// Sets the URL hash as a new history entry (no `hashchange` reaches the app: it follows
/// the route itself).
pub fn push_hash(hash: &str) {
    if let Some(history) = web_sys::window().and_then(|window| window.history().ok()) {
        let _ = history.push_state_with_url(&JsValue::NULL, "", Some(hash));
    }
}

/// Replaces the URL hash of the current history entry.
pub fn replace_hash(hash: &str) {
    if let Some(history) = web_sys::window().and_then(|window| window.history().ok()) {
        let _ = history.replace_state_with_url(&JsValue::NULL, "", Some(hash));
    }
}

/// Saves the profiles.
///
/// # Errors
///
/// Returns a message when `localStorage` is unavailable or full.
pub fn save_profiles(profiles: &Profiles) -> Result<(), String> {
    local_storage()
        .ok_or_else(|| "localStorage is unavailable".to_owned())?
        .set_item(profiles::STORAGE_KEY, &profiles.to_json())
        .map_err(|error| js_message(&error))
}

/// The page's `localStorage`.
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

/// `window.__TAURI__.core`, inside the Tauri shell.
fn tauri_core() -> Option<JsValue> {
    let window = web_sys::window()?;
    let tauri = Reflect::get(&window, &JsValue::from_str("__TAURI__")).ok()?;
    if tauri.is_undefined() || tauri.is_null() {
        return None;
    }
    Reflect::get(&tauri, &JsValue::from_str("core"))
        .ok()
        .filter(JsValue::is_object)
}

/// Calls the shell command `command` with `args`; rejections become their message.
async fn invoke(command: &str, args: &JsValue) -> Result<JsValue, String> {
    let core = tauri_core().ok_or_else(|| "not running in the Tauri shell".to_owned())?;
    let invoke: Function = Reflect::get(&core, &JsValue::from_str("invoke"))
        .map_err(|error| js_message(&error))?
        .dyn_into()
        .map_err(|_| "__TAURI__.core.invoke is not a function".to_owned())?;
    let promise: Promise = invoke
        .call2(&core, &JsValue::from_str(command), args)
        .map_err(|error| js_message(&error))?
        .dyn_into()
        .map_err(|_| "invoke returned no promise".to_owned())?;
    JsFuture::from(promise)
        .await
        .map_err(|error| js_message(&error))
}

/// A JavaScript object with string properties.
fn object(properties: &[(&str, &str)]) -> JsValue {
    let object = Object::new();
    for (name, value) in properties {
        // Setting a property of a fresh plain object cannot fail.
        let _ = Reflect::set(&object, &JsValue::from_str(name), &JsValue::from_str(value));
    }
    object.into()
}

/// The message of a JavaScript error value.
pub fn js_message(error: &JsValue) -> String {
    if let Some(message) = error.as_string() {
        return message;
    }
    error.dyn_ref::<js_sys::Error>().map_or_else(
        || format!("{error:?}"),
        |error| String::from(error.message()),
    )
}

/// The `overseer-proxy` serving this page, from its `GET /overseer-proxy.json`.
async fn page_proxy() -> Option<PageProxy> {
    let window = web_sys::window()?;
    let origin = window.location().origin().ok()?;
    let response: web_sys::Response = JsFuture::from(window.fetch_with_str("/overseer-proxy.json"))
        .await
        .ok()?
        .dyn_into()
        .ok()?;
    if !response.ok() {
        return None;
    }
    let body = JsFuture::from(response.text().ok()?)
        .await
        .ok()?
        .as_string()?;
    PageProxy::parse(&origin, &body)
}
