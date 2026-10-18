//! Connecting to a server and reading its failures.

use std::fmt;

use overseer::core::api::{INCORRECT_SERVER_PASSWORD, Refusal};
use overseer::model::platform::ServerKind;
use overseer::{Auth, Guest, Overseer, ServerProfile};

use crate::platform::Platform;
use crate::profiles::server_password_key;

/// Why connecting or a request failed, as the UI treats it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The token or the name and password were rejected (HTTP 401).
    Unauthorized,
    /// The server is password-protected and the server password is missing or wrong.
    ServerPassword,
    /// Anything else, with the message to show (server error strings as-is).
    Other(String),
}

impl Failure {
    /// Classifies a library error by its refusal ([`overseer::Error::refusal`]).
    pub fn of(error: &overseer::Error) -> Self {
        match error.refusal() {
            Some(Refusal::Unauthorized) => Self::Unauthorized,
            Some(Refusal::ServerPassword) => Self::ServerPassword,
            None => Self::Other(error_message(error)),
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::ServerPassword => f.write_str(INCORRECT_SERVER_PASSWORD),
            Self::Other(message) => f.write_str(message),
        }
    }
}

/// The text to show for a client error: the server's own error string when it sent one,
/// else the error and its causes.
pub fn message(error: &overseer::net::Error) -> String {
    error
        .server_message()
        .map_or_else(|| describe(error), str::to_owned)
}

/// [`message`] for a library error: the server's own error string when it sent one, else the
/// error and its causes.
pub fn error_message(error: &overseer::Error) -> String {
    error
        .server_message()
        .map_or_else(|| describe(error), str::to_owned)
}

/// `error` followed by each of its causes.
pub fn describe(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !text.ends_with(&cause_text) {
            text.push_str(": ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// Connects to the server with base URL `server` as `auth`, sending `server_password` or else
/// the stored one, and forcing the contract `server_kind` unless `None`
/// ([`ServerKind::forced`](crate::profiles::ServerKind::forced)).
///
/// # Errors
///
/// Returns the classified failure.
pub async fn connect(
    platform: &Platform,
    server: &str,
    server_kind: Option<ServerKind>,
    auth: Auth,
    server_password: Option<String>,
) -> Result<Overseer, Failure> {
    let url = platform.server_url(server).await.map_err(Failure::Other)?;
    let server_password = match server_password {
        Some(password) => Some(password),
        None => stored_server_password(platform, server).await,
    };
    let profile = ServerProfile {
        url,
        auth,
        server_password,
        default_shard: None,
        server_kind,
        plugins: None,
    };
    Overseer::connect(profile)
        .await
        .map_err(|error| Failure::of(&error))
}

/// The server with base URL `server` without an account ([`Guest`]: its subsystems, for the
/// sign-in screen, and registration and its checks), sending `server_password` or else the
/// stored one, and forcing the contract `server_kind` unless `None`. Discovers the server
/// ([`Guest::discover`]); needs no account.
///
/// # Errors
///
/// Returns [`Failure::ServerPassword`] when the server wants a (correct) server password, and
/// [`Failure::Other`] when the address cannot be resolved, the client cannot be built or the
/// server cannot be discovered.
pub async fn guest(
    platform: &Platform,
    server: &str,
    server_kind: Option<ServerKind>,
    server_password: Option<String>,
) -> Result<Guest, Failure> {
    let url = platform.server_url(server).await.map_err(Failure::Other)?;
    let server_password = match server_password {
        Some(password) => Some(password),
        None => stored_server_password(platform, server).await,
    };
    Guest::discover(url, server_password, server_kind)
        .await
        .map_err(|error| Failure::of(&error))
}

/// The stored server password of `server`, if any (a failing store counts as none).
async fn stored_server_password(platform: &Platform, server: &str) -> Option<String> {
    platform
        .secret_get(&server_password_key(server))
        .await
        .ok()
        .flatten()
        .filter(|password| !password.is_empty())
}

#[cfg(test)]
mod tests {
    use overseer::core::api::ApiError;
    use overseer::net::Error;

    use super::{Failure, message};

    #[test]
    fn classifies_failures() {
        assert_eq!(
            Failure::of(&overseer::Error::Net(Error::Api(ApiError::Unauthorized))),
            Failure::Unauthorized
        );
        assert_eq!(
            Failure::of(&overseer::Error::Net(Error::Api(ApiError::Server(
                "incorrect server password".to_owned()
            )))),
            Failure::ServerPassword
        );
        assert_eq!(
            Failure::of(&overseer::Error::Net(Error::Api(ApiError::Server(
                "exists".to_owned()
            )))),
            Failure::Other("exists".to_owned())
        );
        assert_eq!(
            Failure::of(&overseer::Error::Closed),
            Failure::Other("the session is closed".to_owned())
        );
        assert_eq!(
            message(&Error::UnexpectedReply("no user")),
            "unexpected reply: no user"
        );
        assert_eq!(
            Failure::ServerPassword.to_string(),
            "incorrect server password"
        );
        assert_eq!(Failure::Other("down".to_owned()).to_string(), "down");
    }
}
