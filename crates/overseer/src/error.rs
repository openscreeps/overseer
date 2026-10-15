//! The crate's error type.

use std::path::PathBuf;

use crate::live::Closed;

/// Why an operation failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A transport, protocol or server failure of [`overseer_net`].
    #[error(transparent)]
    Net(overseer_net::Error),
    /// The operation needs a signed-in user, but the session is anonymous.
    #[error("the operation needs a signed-in user")]
    Anonymous,
    /// [`Overseer::shutdown`](crate::Overseer::shutdown) was called, or the socket gave up
    /// reconnecting.
    #[error("the session is closed")]
    Closed,
    /// Reading or writing a local file failed (code directories).
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// The failure.
        #[source]
        source: std::io::Error,
    },
    /// An argument was rejected before anything was sent.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}

impl From<overseer_net::Error> for Error {
    /// Keeps [`overseer_net::Error::Anonymous`] as [`Error::Anonymous`].
    fn from(error: overseer_net::Error) -> Self {
        match error {
            overseer_net::Error::Anonymous => Self::Anonymous,
            error => Self::Net(error),
        }
    }
}

impl From<Closed> for Error {
    fn from(Closed: Closed) -> Self {
        Self::Closed
    }
}
