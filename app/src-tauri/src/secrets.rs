//! The `secret_*` commands: tokens and server passwords in the OS credential store (Windows
//! Credential Manager, macOS Keychain, the Secret Service on Linux), service `overseer`.
#![expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands command arguments over by value"
)]

use keyring::Entry;

/// Service name of every entry.
const SERVICE: &str = "overseer";

/// The entry for `key`.
fn entry(key: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, key).map_err(|error| error.to_string())
}

/// The secret stored under `key`, `None` when there is none.
///
/// # Errors
///
/// Returns a message when the credential store fails.
#[tauri::command(async)]
pub fn secret_get(key: String) -> Result<Option<String>, String> {
    match entry(&key)?.get_password() {
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
pub fn secret_set(key: String, value: String) -> Result<(), String> {
    entry(&key)?
        .set_password(&value)
        .map_err(|error| error.to_string())
}

/// Deletes the secret under `key`; deleting a missing one succeeds.
///
/// # Errors
///
/// Returns a message when the credential store fails.
#[tauri::command(async)]
pub fn secret_delete(key: String) -> Result<(), String> {
    match entry(&key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
