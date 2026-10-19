//! The storage of a browser page: the profiles document and the secrets in `localStorage`.
//!
//! The profiles document is under [`PROFILES_KEY`]. A secret is under [`SECRET_PREFIX`]
//! followed by its key.
//!
//! Caution: all scripts that run on the origin of the page can read these secrets.

use wasm_bindgen::{JsCast as _, JsValue};

use crate::Error;
use crate::profiles::Profiles;

/// `localStorage` key of the profiles document.
pub const PROFILES_KEY: &str = "overseer.profiles";
/// `localStorage` prefix of the secrets.
pub const SECRET_PREFIX: &str = "overseer.secret.";

/// The saved profiles. No `localStorage` or no document gives the defaults.
#[must_use]
pub fn load_profiles() -> Profiles {
    let json = local_storage().and_then(|storage| storage.get_item(PROFILES_KEY).ok().flatten());
    Profiles::parse(json.as_deref())
}

/// Saves the profiles.
///
/// # Errors
///
/// Returns an error when `localStorage` is unavailable or full.
pub fn save_profiles(profiles: &Profiles) -> Result<(), Error> {
    local_storage()
        .ok_or(Error::Unavailable)?
        .set_item(PROFILES_KEY, &profiles.to_json())
        .map_err(|error| Error::Web(js_message(&error)))
}

/// The secret stored under `key`. No `localStorage` or no secret gives `None`.
#[must_use]
pub fn secret(key: &str) -> Option<String> {
    local_storage().and_then(|storage| storage.get_item(&secret_item(key)).ok().flatten())
}

/// Stores `value` under `key` and replaces the previous secret.
///
/// # Errors
///
/// Returns an error when `localStorage` is unavailable or full.
pub fn set_secret(key: &str, value: &str) -> Result<(), Error> {
    local_storage()
        .ok_or(Error::Unavailable)?
        .set_item(&secret_item(key), value)
        .map_err(|error| Error::Web(js_message(&error)))
}

/// Deletes the secret under `key`. Deleting a missing secret succeeds.
///
/// # Errors
///
/// Returns an error when `localStorage` is unavailable.
pub fn delete_secret(key: &str) -> Result<(), Error> {
    local_storage()
        .ok_or(Error::Unavailable)?
        .remove_item(&secret_item(key))
        .map_err(|error| Error::Web(js_message(&error)))
}

/// The `localStorage` item of the secret `key`.
fn secret_item(key: &str) -> String {
    format!("{SECRET_PREFIX}{key}")
}

/// The `localStorage` of the page.
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
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

#[cfg(test)]
mod tests {
    use super::{PROFILES_KEY, SECRET_PREFIX, secret_item};

    #[test]
    fn keys_stay_the_same() {
        assert_eq!(PROFILES_KEY, "overseer.profiles");
        assert_eq!(SECRET_PREFIX, "overseer.secret.");
        assert_eq!(
            secret_item("token:https://screeps.com/"),
            "overseer.secret.token:https://screeps.com/"
        );
    }
}
