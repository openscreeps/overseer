//! The `secret_*` commands: tokens and server passwords in the OS credential store, through
//! [`Storage`] of the app identifier. The app itself uses the service `overseer`; builds with
//! another identifier (verification runs) never touch its credentials
//! ([`overseer_storage::native::service`]).
#![expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands command arguments over by value"
)]

use tauri::{AppHandle, Runtime};

use crate::storage;

/// The secret stored under `key`, `None` when there is none.
///
/// # Errors
///
/// Returns a message when the credential store fails.
#[tauri::command(async)]
pub fn secret_get<R: Runtime>(app: AppHandle<R>, key: String) -> Result<Option<String>, String> {
    storage(&app)?
        .secret(&key)
        .map_err(|error| error.to_string())
}

/// Stores `value` under `key`, replacing any previous secret.
///
/// # Errors
///
/// Returns a message when the credential store fails.
#[tauri::command(async)]
pub fn secret_set<R: Runtime>(app: AppHandle<R>, key: String, value: String) -> Result<(), String> {
    storage(&app)?
        .set_secret(&key, &value)
        .map_err(|error| error.to_string())
}

/// Deletes the secret under `key`; deleting a missing one succeeds.
///
/// # Errors
///
/// Returns a message when the credential store fails.
#[tauri::command(async)]
pub fn secret_delete<R: Runtime>(app: AppHandle<R>, key: String) -> Result<(), String> {
    storage(&app)?
        .delete_secret(&key)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use overseer_storage::native::APP_IDENTIFIER;

    #[test]
    fn the_app_identifier_is_the_one_of_the_shell() {
        assert!(
            include_str!("../tauri.conf.json")
                .contains(&format!("\"identifier\": \"{APP_IDENTIFIER}\"")),
            "APP_IDENTIFIER is the identifier of tauri.conf.json"
        );
    }
}
