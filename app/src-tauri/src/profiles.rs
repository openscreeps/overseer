//! The `profiles_*` commands: the saved servers in the profiles file of [`Storage`], which the
//! CLI (`overseer servers`) shares.
//!
//! Each change goes through [`Storage::update`]: the shell applies one [`Change`] to the current
//! file under its lock. Thus the changes of the CLI stay. Each command returns the document of
//! the file ([`Profiles::to_json`]), which the app takes as its new server list.
#![expect(
    clippy::needless_pass_by_value,
    reason = "Tauri hands command arguments over by value"
)]

use overseer_storage::Error;
use overseer_storage::native::Storage;
use overseer_storage::profiles::{Change, Profiles};
use serde::Serialize;
use tauri::{AppHandle, Runtime};

use crate::storage;

/// The answer of [`profiles_load`].
#[derive(Debug, Serialize)]
pub struct Loaded {
    /// The profiles document.
    profiles: String,
    /// Whether the shell imported the document of the app's `localStorage` (`legacy`).
    imported: bool,
}

/// The saved profiles. When there is no profiles file yet, `legacy` (the document that the app
/// kept in `localStorage` before) becomes the profiles file.
///
/// # Errors
///
/// Returns a message when the file cannot be read or written.
#[tauri::command(async)]
pub fn profiles_load<R: Runtime>(
    app: AppHandle<R>,
    legacy: Option<String>,
) -> Result<Loaded, String> {
    load(&storage(&app)?, legacy.as_deref()).map_err(|error| error.to_string())
}

/// Applies `change` to the profiles file and returns the document of the file.
///
/// # Errors
///
/// Returns a message when the file cannot be read or written.
#[tauri::command(async)]
pub fn profiles_change<R: Runtime>(app: AppHandle<R>, change: Change) -> Result<String, String> {
    apply(&storage(&app)?, change).map_err(|error| error.to_string())
}

/// See [`profiles_load`].
fn load(storage: &Storage, legacy: Option<&str>) -> Result<Loaded, Error> {
    let path = storage.profiles_path();
    if let Some(legacy) = legacy
        && !path.exists()
    {
        let mut imported = false;
        let profiles = storage.update(|profiles| {
            // The CLI can make the file before the lock: then its document stays.
            if !path.exists() {
                *profiles = Profiles::parse(Some(legacy));
                imported = true;
            }
            profiles.to_json()
        })?;
        return Ok(Loaded { profiles, imported });
    }
    Ok(Loaded {
        profiles: storage.load()?.to_json(),
        imported: false,
    })
}

/// See [`profiles_change`].
fn apply(storage: &Storage, change: Change) -> Result<String, Error> {
    storage.update(|profiles| {
        // The app applied the change to its own list first and showed the errors (a server
        // that is listed already). Here an error says that the file changed meanwhile, and the
        // returned document shows the result.
        let _added = profiles.apply(change);
        profiles.to_json()
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use overseer_storage::native::Storage;
    use overseer_storage::profiles::{Change, OFFICIAL_URL, Profiles, Server, ServerKind};

    use super::{apply, load};

    const URL: &str = "http://localhost:21025/";

    /// A temporary directory, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("overseer-app-tauri-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }

        /// A storage in this directory (these tests use no secrets).
        fn storage(&self) -> Storage {
            Storage::at(self.0.clone(), "overseer:test".to_owned())
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A document with screeps.com and the local server.
    fn legacy() -> String {
        let mut profiles = Profiles::default();
        profiles
            .add(Server::new(
                "Local".to_owned(),
                URL.to_owned(),
                ServerKind::Private,
            ))
            .unwrap();
        profiles.to_json()
    }

    #[test]
    fn without_a_file_the_legacy_document_becomes_the_file() {
        let dir = TempDir::new("import");
        let storage = dir.storage();
        let loaded = load(&storage, Some(&legacy())).unwrap();
        assert!(loaded.imported);
        assert_eq!(loaded.profiles, legacy());
        assert_eq!(storage.load().unwrap().to_json(), legacy());
    }

    #[test]
    fn an_existing_file_stays_and_the_legacy_document_is_not_imported() {
        let dir = TempDir::new("existing");
        let storage = dir.storage();
        storage.update(|_| ()).unwrap();
        let loaded = load(&storage, Some(&legacy())).unwrap();
        assert!(!loaded.imported);
        assert_eq!(loaded.profiles, Profiles::default().to_json());
        assert_eq!(storage.load().unwrap(), Profiles::default());
    }

    #[test]
    fn without_a_legacy_document_loading_writes_nothing() {
        let dir = TempDir::new("plain");
        let storage = dir.storage();
        let loaded = load(&storage, None).unwrap();
        assert!(!loaded.imported);
        assert_eq!(loaded.profiles, Profiles::default().to_json());
        assert!(!storage.profiles_path().exists());
    }

    #[test]
    fn a_change_applies_to_the_current_file() {
        let dir = TempDir::new("change");
        let storage = dir.storage();
        // The CLI added a server that the app does not know.
        storage
            .update(|profiles| {
                profiles.add(Server::new(
                    "Other".to_owned(),
                    "http://other.example/".to_owned(),
                    ServerKind::Private,
                ))
            })
            .unwrap()
            .unwrap();
        let change = Change::AddServer {
            server: Server::new("Local".to_owned(), URL.to_owned(), ServerKind::Private),
        };
        let document = apply(&storage, change.clone()).unwrap();
        let profiles = Profiles::parse(Some(&document));
        let urls: Vec<_> = profiles
            .servers
            .iter()
            .map(|server| server.url.as_str())
            .collect();
        assert_eq!(urls, [OFFICIAL_URL, "http://other.example/", URL]);
        assert_eq!(storage.load().unwrap(), profiles);
        // The same change again (a server listed meanwhile) keeps the file as it is.
        assert_eq!(apply(&storage, change).unwrap(), document);
        let removed = apply(
            &storage,
            Change::RemoveServer {
                url: URL.to_owned(),
            },
        )
        .unwrap();
        assert!(Profiles::parse(Some(&removed)).server(URL).is_none());
    }
}
