//! The `secret_*` commands: tokens and server passwords in the OS credential store (Windows
//! Credential Manager, macOS Keychain, the Secret Service on Linux), under the service
//! [`service`] derives from the app identifier: `overseer` for the app itself, so that builds
//! with another identifier (verification runs) never touch its credentials.
#![expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands command arguments over by value"
)]

use keyring::Entry;
use tauri::{AppHandle, Runtime};

/// Identifier of the app (`tauri.conf.json`), whose service is `overseer`.
const APP_IDENTIFIER: &str = "com.openscreeps.overseer";

/// The credential service of the app with identifier `identifier`: `overseer` for the app,
/// `overseer:<identifier>` for any other build.
fn service(identifier: &str) -> String {
    if identifier == APP_IDENTIFIER {
        "overseer".to_owned()
    } else {
        format!("overseer:{identifier}")
    }
}

/// The entry for `key` of `app`.
fn entry<R: Runtime>(app: &AppHandle<R>, key: &str) -> Result<Entry, String> {
    Entry::new(&service(&app.config().identifier), key).map_err(|error| error.to_string())
}

/// The secret stored under `key`, `None` when there is none.
///
/// # Errors
///
/// Returns a message when the credential store fails.
#[tauri::command(async)]
pub fn secret_get<R: Runtime>(app: AppHandle<R>, key: String) -> Result<Option<String>, String> {
    found(entry(&app, &key)?.get_password())
}

/// A read secret, a missing one being `None`.
fn found(result: keyring::Result<String>) -> Result<Option<String>, String> {
    match result {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Stores `value` under `key`, replacing any previous secret.
///
/// # Errors
///
/// Returns a message when the credential store fails.
#[tauri::command(async)]
pub fn secret_set<R: Runtime>(app: AppHandle<R>, key: String, value: String) -> Result<(), String> {
    entry(&app, &key)?
        .set_password(&value)
        .map_err(|error| error.to_string())
}

/// Deletes the secret under `key`; deleting a missing one succeeds.
///
/// # Errors
///
/// Returns a message when the credential store fails.
#[tauri::command(async)]
pub fn secret_delete<R: Runtime>(app: AppHandle<R>, key: String) -> Result<(), String> {
    deleted(entry(&app, &key)?.delete_credential())
}

/// A deletion, deleting a missing secret succeeding.
fn deleted(result: keyring::Result<()>) -> Result<(), String> {
    match result {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::{APP_IDENTIFIER, deleted, found, service};

    #[test]
    fn missing_secrets_are_not_errors() {
        assert_eq!(found(Ok("t".to_owned())), Ok(Some("t".to_owned())));
        assert_eq!(found(Err(keyring::Error::NoEntry)), Ok(None));
        assert!(found(Err(keyring::Error::TooLong("user".to_owned(), 1))).is_err());
        assert_eq!(deleted(Err(keyring::Error::NoEntry)), Ok(()));
        assert!(deleted(Err(keyring::Error::Invalid("k".to_owned(), "r".to_owned()))).is_err());
    }

    #[test]
    fn other_builds_never_share_the_apps_credentials() {
        assert!(
            include_str!("../tauri.conf.json")
                .contains(&format!("\"identifier\": \"{APP_IDENTIFIER}\"")),
            "APP_IDENTIFIER is the identifier of tauri.conf.json"
        );
        assert_eq!(service(APP_IDENTIFIER), "overseer");
        assert_eq!(
            service("com.openscreeps.overseer.verify"),
            "overseer:com.openscreeps.overseer.verify"
        );
        assert_ne!(
            service("com.openscreeps.overseer.verify"),
            service(APP_IDENTIFIER)
        );
    }
}
