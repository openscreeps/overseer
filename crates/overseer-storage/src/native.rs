//! The storage on a desktop: the profiles file in the configuration directory of the app, and
//! the secrets in the OS credential store (Windows Credential Manager, macOS Keychain, the
//! Secret Service on Linux).
//!
//! [`Storage::open`] uses the directory that Tauri's `app_config_dir` gives for the same
//! identifier: [`dirs::config_dir`] joined with the identifier. The desktop app and the CLI
//! thus share one profiles file.
//!
//! [`Storage::update`] changes the file under an exclusive lock on a lock file and replaces it
//! atomically (a temporary file, then a rename). Two processes that change the profiles at the
//! same time thus keep both changes, and a reader never sees a half-written file.
//!
//! The secrets of the app are under the credential service `overseer` ([`service`]). A build
//! with another identifier (for example a verification run) uses its own service, so it never
//! touches the credentials of the app.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use keyring::Entry;

use crate::Error;
use crate::profiles::Profiles;

/// Identifier of the desktop app (`tauri.conf.json`), whose credential service is `overseer`.
pub const APP_IDENTIFIER: &str = "com.openscreeps.overseer";
/// Name of the profiles file in the storage directory.
pub const PROFILES_FILE: &str = "profiles.json";
/// Name of the lock file that [`Storage::update`] locks.
const LOCK_FILE: &str = "profiles.lock";
/// Name of the temporary file that [`Storage::update`] writes before the rename.
const TEMPORARY_FILE: &str = "profiles.json.tmp";

/// The credential service of the app with identifier `identifier`: `overseer` for the app,
/// `overseer:<identifier>` for all other builds.
#[must_use]
pub fn service(identifier: &str) -> String {
    if identifier == APP_IDENTIFIER {
        "overseer".to_owned()
    } else {
        format!("overseer:{identifier}")
    }
}

/// The profiles file and the secrets of one app identifier.
#[derive(Clone, Debug)]
pub struct Storage {
    /// Directory of the profiles file.
    dir: PathBuf,
    /// Credential service of the secrets.
    service: String,
}

impl Storage {
    /// The storage of the app with identifier `identifier` (use [`APP_IDENTIFIER`] for the
    /// app): the directory of Tauri's `app_config_dir` and the credential service
    /// [`service`]. This does not create the directory.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoConfigDir`] when the system gives no configuration directory.
    pub fn open(identifier: &str) -> Result<Self, Error> {
        let dir = dirs::config_dir()
            .ok_or(Error::NoConfigDir)?
            .join(identifier);
        Ok(Self::at(dir, service(identifier)))
    }

    /// The storage with the profiles file in `dir` and the secrets under the credential
    /// service `service`.
    ///
    /// The secrets go through the default credential builder of `keyring`. To keep tests away
    /// from the OS credential store, install another builder with
    /// `keyring::set_default_credential_builder` before the first secret operation.
    #[must_use]
    pub const fn at(dir: PathBuf, service: String) -> Self {
        Self { dir, service }
    }

    /// The directory of the profiles file.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The path of the profiles file.
    #[must_use]
    pub fn profiles_path(&self) -> PathBuf {
        self.dir.join(PROFILES_FILE)
    }

    /// The credential service of the secrets.
    #[must_use]
    pub fn service(&self) -> &str {
        &self.service
    }

    /// The saved profiles. A missing file gives the defaults. A file that does not parse
    /// gives what [`Profiles::parse`] gives, so a corrupt file gives the defaults.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the file exists but cannot be read.
    pub fn load(&self) -> Result<Profiles, Error> {
        let path = self.profiles_path();
        match fs::read(&path) {
            Ok(bytes) => Ok(Profiles::parse(std::str::from_utf8(&bytes).ok())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Profiles::default()),
            Err(source) => Err(Error::Io {
                action: "read",
                path,
                source,
            }),
        }
    }

    /// Reads the profiles, applies `change` and writes the result, under an exclusive lock.
    /// Returns the result of `change`.
    ///
    /// The lock stops all other `update` calls on the same directory, in this process and in
    /// other processes, until the write is complete. The write replaces the file atomically.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the directory, the lock or the file fails.
    pub fn update<R>(&self, change: impl FnOnce(&mut Profiles) -> R) -> Result<R, Error> {
        fs::create_dir_all(&self.dir).map_err(|source| Error::Io {
            action: "create",
            path: self.dir.clone(),
            source,
        })?;
        let _lock = self.lock()?;
        let mut profiles = self.load()?;
        let result = change(&mut profiles);
        self.write(&profiles)?;
        Ok(result)
    }

    /// Opens and locks the lock file. The lock stays until the file closes.
    fn lock(&self) -> Result<File, Error> {
        let path = self.dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|source| Error::Io {
                action: "open",
                path: path.clone(),
                source,
            })?;
        file.lock().map_err(|source| Error::Io {
            action: "lock",
            path,
            source,
        })?;
        Ok(file)
    }

    /// Writes `profiles` to a temporary file and renames it over the profiles file.
    fn write(&self, profiles: &Profiles) -> Result<(), Error> {
        let temporary = self.dir.join(TEMPORARY_FILE);
        let written = File::create(&temporary).and_then(|mut file| {
            file.write_all(profiles.to_json().as_bytes())?;
            file.sync_all()
        });
        written.map_err(|source| Error::Io {
            action: "write",
            path: temporary.clone(),
            source,
        })?;
        let path = self.profiles_path();
        fs::rename(&temporary, &path).map_err(|source| Error::Io {
            action: "replace",
            path,
            source,
        })
    }

    /// The secret stored under `key`, `None` when there is none.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Secrets`] when the credential store fails.
    pub fn secret(&self, key: &str) -> Result<Option<String>, Error> {
        found(self.entry(key)?.get_password()).map_err(|error| secrets_error(key, &error))
    }

    /// Stores `value` under `key` and replaces the previous secret.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Secrets`] when the credential store fails.
    pub fn set_secret(&self, key: &str, value: &str) -> Result<(), Error> {
        self.entry(key)?
            .set_password(value)
            .map_err(|error| secrets_error(key, &error))
    }

    /// Deletes the secret under `key`. Deleting a missing secret succeeds.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Secrets`] when the credential store fails.
    pub fn delete_secret(&self, key: &str) -> Result<(), Error> {
        deleted(self.entry(key)?.delete_credential()).map_err(|error| secrets_error(key, &error))
    }

    /// The credential entry for `key`.
    fn entry(&self, key: &str) -> Result<Entry, Error> {
        Entry::new(&self.service, key).map_err(|error| secrets_error(key, &error))
    }
}

/// A read secret; a missing secret is `None`.
fn found(result: keyring::Result<String>) -> keyring::Result<Option<String>> {
    match result {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error),
    }
}

/// A deletion; the deletion of a missing secret succeeds.
fn deleted(result: keyring::Result<()>) -> keyring::Result<()> {
    match result {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error),
    }
}

/// The [`Error::Secrets`] of `error` for `key`. The message leaves out the data that some
/// errors carry (the bytes of a secret, the matching credentials).
fn secrets_error(key: &str, error: &keyring::Error) -> Error {
    let message = match error {
        keyring::Error::BadEncoding(_) => "the stored secret is not UTF-8".to_owned(),
        keyring::Error::Ambiguous(credentials) => {
            format!("{} credentials match the entry", credentials.len())
        }
        keyring::Error::PlatformFailure(_)
        | keyring::Error::NoStorageAccess(_)
        | keyring::Error::NoEntry
        | keyring::Error::TooLong(..)
        | keyring::Error::Invalid(..) => error.to_string(),
        _ => "unknown failure".to_owned(),
    };
    Error::Secrets {
        key: key.to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests;
