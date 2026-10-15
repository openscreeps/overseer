//! Where the app runs: the Tauri shell or a plain browser.
//!
//! | | Tauri shell | Browser |
//! |---|---|---|
//! | Servers | through the shell's proxy (`proxy_url`) | directly, or the page's own `overseer-proxy` ([`crate::connect`]) |
//! | Secrets (tokens, server passwords) | OS credential store (`secret_*` commands) | `localStorage` under `overseer.secret.<key>`, readable by anything running on the page's origin |
//! | External links | system browser (`tauri-plugin-opener`) | new tab |
//!
//! Saved profiles ([`crate::profiles`]) are in `localStorage` on both.

use js_sys::{Function, Object, Promise, Reflect};
use overseer::core::ServerUrl;
use url::Url;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::connect::{ConnectTarget, PageProxy, connect_target};
use crate::profiles::{Profiles, STORAGE_KEY};

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

    /// The address to connect to for the server with base URL `server`.
    ///
    /// # Errors
    ///
    /// Returns a message when the URL is invalid or the shell cannot start its proxy.
    pub async fn server_url(&self, server: &str) -> Result<ServerUrl, String> {
        let url = Url::parse(server).map_err(|error| format!("Invalid server URL: {error}"))?;
        let address = match connect_target(self.tauri, self.page_proxy.as_ref(), &url) {
            ConnectTarget::TauriProxy => {
                let args = object(&[("upstream", server)]);
                invoke("proxy_url", &args)
                    .await?
                    .as_string()
                    .ok_or_else(|| "proxy_url returned no URL".to_owned())?
            }
            ConnectTarget::Direct(url) | ConnectTarget::PageOrigin(url) => url.into(),
        };
        ServerUrl::parse(&address).map_err(|error| error.to_string())
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
}

/// The saved profiles.
pub fn load_profiles() -> Profiles {
    let json = local_storage().and_then(|storage| storage.get_item(STORAGE_KEY).ok().flatten());
    Profiles::parse(json.as_deref())
}

/// Saves the profiles.
///
/// # Errors
///
/// Returns a message when `localStorage` is unavailable or full.
pub fn save_profiles(profiles: &Profiles) -> Result<(), String> {
    local_storage()
        .ok_or_else(|| "localStorage is unavailable".to_owned())?
        .set_item(STORAGE_KEY, &profiles.to_json())
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
fn js_message(error: &JsValue) -> String {
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
