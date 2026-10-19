//! The error of the storage.

use std::path::PathBuf;

/// A storage operation failed. The error never contains the value of a secret.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The system gives no configuration directory for the user (`native`).
    #[error("no configuration directory is known for this user")]
    NoConfigDir,
    /// A file operation failed (`native`).
    #[error("could not {action} {}: {source}", path.display())]
    Io {
        /// The operation, for example `write`.
        action: &'static str,
        /// The file or directory.
        path: PathBuf,
        /// The cause.
        source: std::io::Error,
    },
    /// The OS credential store failed (`native`). The message never contains the secret.
    #[error("the credential store failed for {key}: {message}")]
    Secrets {
        /// The secret key, for example `token:https://screeps.com/`.
        key: String,
        /// What failed.
        message: String,
    },
    /// The page has no `localStorage` (`web`).
    #[error("localStorage is unavailable")]
    Unavailable,
    /// `localStorage` failed, for example because it is full (`web`). The text is the message
    /// of the JavaScript error.
    #[error("{0}")]
    Web(String),
}
