//! The saved servers and sign-ins. The CLI shares them with the desktop app: the profiles file
//! in the configuration directory of the app and the secrets in the OS credential store
//! ([`overseer_storage::native`]).
//!
//! The CLI uses the storage of the app identifier [`APP_IDENTIFIER`]. The variable
//! [`IDENTIFIER_VAR`] selects the storage of another identifier, for example of a test or of
//! another build. Each identifier has its own directory and its own credential service. The
//! variable does not select another kind of store.
//!
//! The CLI opens the storage only when a command needs it. Each change goes through
//! [`Storage::update`], so the CLI and the app keep the changes of each other.

use std::cell::OnceCell;
use std::env::VarError;

use overseer_storage::native::{APP_IDENTIFIER, Storage};
use overseer_storage::profiles::{self, Profiles, Server};

use crate::config::Sensitive;

/// The variable that selects the app identifier of the storage.
pub(crate) const IDENTIFIER_VAR: &str = "OVERSEER_APP_IDENTIFIER";

/// The saved servers and secrets of one app identifier, opened on first use.
#[derive(Debug, Default)]
pub(crate) struct Saved {
    storage: OnceCell<Storage>,
}

impl Saved {
    /// The storage of the identifier of [`IDENTIFIER_VAR`], or of [`APP_IDENTIFIER`].
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The storage `storage`.
    #[cfg(test)]
    pub(crate) fn at(storage: Storage) -> Self {
        Self {
            storage: OnceCell::from(storage),
        }
    }

    /// The storage. Opens it on the first call.
    ///
    /// # Errors
    ///
    /// Returns an error when the identifier is not valid, or when the system gives no
    /// configuration directory.
    pub(crate) fn storage(&self) -> Result<&Storage, Error> {
        if let Some(storage) = self.storage.get() {
            return Ok(storage);
        }
        let storage = Storage::open(&identifier()?)?;
        Ok(self.storage.get_or_init(|| storage))
    }

    /// The saved servers, read from the file now.
    pub(crate) fn load(&self) -> Result<Profiles, Error> {
        Ok(self.storage()?.load()?)
    }

    /// Changes the saved servers with `change` under the lock of the storage.
    pub(crate) fn update<R>(&self, change: impl FnOnce(&mut Profiles) -> R) -> Result<R, Error> {
        Ok(self.storage()?.update(change)?)
    }

    /// The saved server that `key` names: its name or its URL ([`Profiles::find`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotSaved`] when no saved server matches, and [`Error::Ambiguous`] when
    /// two saved servers have the name `key`.
    pub(crate) fn find(&self, key: &str) -> Result<Server, Error> {
        let profiles = self.load()?;
        match profiles.find(key)? {
            Some(server) => Ok(server.clone()),
            None => Err(Error::NotSaved {
                key: key.to_owned(),
                names: profiles.servers.iter().map(|s| s.name.clone()).collect(),
            }),
        }
    }

    /// The secret under `key`, `None` when there is none.
    pub(crate) fn secret(&self, key: &str) -> Result<Option<Sensitive>, Error> {
        Ok(self.storage()?.secret(key)?.map(Sensitive::new))
    }

    /// Saves `value` under `key`.
    pub(crate) fn set_secret(&self, key: &str, value: &Sensitive) -> Result<(), Error> {
        Ok(self.storage()?.set_secret(key, value.expose())?)
    }

    /// Deletes the secret under `key`. A missing secret is not an error.
    pub(crate) fn delete_secret(&self, key: &str) -> Result<(), Error> {
        Ok(self.storage()?.delete_secret(key)?)
    }
}

/// The app identifier of the storage: the value of [`IDENTIFIER_VAR`], or [`APP_IDENTIFIER`].
fn identifier() -> Result<String, Error> {
    match std::env::var(IDENTIFIER_VAR) {
        Err(VarError::NotPresent) => Ok(APP_IDENTIFIER.to_owned()),
        Ok(value)
            if !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) =>
        {
            Ok(value)
        }
        Ok(value) => Err(Error::Identifier(value)),
        Err(VarError::NotUnicode(value)) => {
            Err(Error::Identifier(value.to_string_lossy().into_owned()))
        }
    }
}

/// The saved servers or secrets are not available.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The value of [`IDENTIFIER_VAR`] is not an app identifier.
    #[error(
        "${IDENTIFIER_VAR} {0:?}: an app identifier has only ASCII letters, digits, `.`, `-` and `_`"
    )]
    Identifier(String),
    /// The storage failed. The message never contains a secret.
    #[error("the saved servers: {0}")]
    Storage(#[from] overseer_storage::Error),
    /// Two saved servers have the name.
    #[error(transparent)]
    Ambiguous(#[from] profiles::Ambiguous),
    /// No saved server has the name or the URL.
    #[error(
        "no saved server {key}; saved servers: {}; add one with `overseer servers add URL`",
        .names.join(", ")
    )]
    NotSaved {
        /// The name or the URL.
        key: String,
        /// The names of the saved servers.
        names: Vec<String>,
    },
}

#[cfg(test)]
pub(crate) mod testing {
    //! A storage for tests: a temporary directory and secrets in memory.

    use std::ops::Deref;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use overseer_storage::native::{Storage, memory};

    use super::Saved;

    /// A [`Saved`] in a new temporary directory, with the secrets in memory. Dropping it
    /// removes the directory.
    pub(crate) struct TempSaved {
        dir: PathBuf,
        saved: Saved,
    }

    impl TempSaved {
        /// A new empty storage. `name` describes the test.
        pub(crate) fn new(name: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            memory::install();
            let unique = format!(
                "{}-{}-{name}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let dir = std::env::temp_dir().join(format!("overseer-cli-test-{unique}"));
            let _ = std::fs::remove_dir_all(&dir);
            let storage = Storage::at(dir.clone(), format!("overseer-cli-test:{unique}"));
            Self {
                dir,
                saved: Saved::at(storage),
            }
        }
    }

    impl Deref for TempSaved {
        type Target = Saved;

        fn deref(&self) -> &Saved {
            &self.saved
        }
    }

    impl Drop for TempSaved {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}
