//! The crate's error type.

use std::path::PathBuf;

use overseer_net::core::api::Refusal;

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
    /// Reading or writing a local file failed (code directories; native-only).
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
    /// A Memory segment id or segment data was rejected before anything was sent.
    #[error(transparent)]
    Segment(#[from] overseer_net::model::memory::SegmentError),
}

impl Error {
    /// The server's own error string
    /// ([`overseer_net::Error::server_message`]), when it sent one.
    #[must_use]
    pub fn server_message(&self) -> Option<&str> {
        match self {
            Self::Net(error) => error.server_message(),
            _ => None,
        }
    }

    /// Why the server refused the client's access ([`overseer_net::Error::refusal`]): the
    /// token or credentials, or the server password. `None` for every other failure.
    #[must_use]
    pub fn refusal(&self) -> Option<Refusal> {
        match self {
            Self::Net(error) => error.refusal(),
            _ => None,
        }
    }
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
