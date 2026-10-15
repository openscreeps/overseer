//! Connecting to a server and reading its failures.

use overseer::core::api::ApiError;
use overseer::net::Client;
use overseer::{Auth, Overseer, ServerProfile};

use crate::platform::Platform;
use crate::profiles::server_password_key;

/// The server's answer when `X-Server-Password` is missing or wrong.
const INCORRECT_SERVER_PASSWORD: &str = "incorrect server password";

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
    /// Classifies a library error.
    pub fn of(error: &overseer::Error) -> Self {
        match error {
            overseer::Error::Net(error) => Self::of_net(error),
            error => Self::Other(describe(error)),
        }
    }

    /// Classifies a client error.
    pub fn of_net(error: &overseer::net::Error) -> Self {
        match error {
            overseer::net::Error::Api(ApiError::Unauthorized) => Self::Unauthorized,
            error => {
                let message = message(error);
                if message == INCORRECT_SERVER_PASSWORD {
                    Self::ServerPassword
                } else {
                    Self::Other(message)
                }
            }
        }
    }
}

/// The text to show for a client error: the server's own error string when it sent one,
/// else the error and its causes.
pub fn message(error: &overseer::net::Error) -> String {
    match error {
        overseer::net::Error::Api(
            ApiError::Server(message)
            | ApiError::Status {
                message: Some(message),
                ..
            },
        ) => message.clone(),
        error => describe(error),
    }
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
/// the stored one.
///
/// # Errors
///
/// Returns the classified failure.
pub async fn connect(
    platform: &Platform,
    server: &str,
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
    };
    Overseer::connect(profile)
        .await
        .map_err(|error| Failure::of(&error))
}

/// A client without an account for the server with base URL `server` (registration checks),
/// sending `server_password` or else the stored one.
///
/// # Errors
///
/// Returns a message when the address cannot be resolved or the client cannot be built.
pub async fn anonymous_client(
    platform: &Platform,
    server: &str,
    server_password: Option<String>,
) -> Result<Client, String> {
    let url = platform.server_url(server).await?;
    let server_password = match server_password {
        Some(password) => Some(password),
        None => stored_server_password(platform, server).await,
    };
    let mut builder = Client::builder(url);
    if let Some(password) = server_password {
        builder = builder.server_password(password);
    }
    builder.build().map_err(|error| describe(&error))
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
            Failure::of_net(&Error::Api(ApiError::Server(
                "incorrect server password".to_owned()
            ))),
            Failure::ServerPassword
        );
        assert_eq!(
            Failure::of_net(&Error::Api(ApiError::Server("exists".to_owned()))),
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
    }
}
