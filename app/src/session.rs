//! Connecting to a server, reading its failures, and waiting for the session's request budget.

use std::fmt;

use overseer::core::api::ApiError;
use overseer::model::platform::ServerKind;
use overseer::net::{Api, Client, Priority, rt};
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

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized => f.write_str("unauthorized"),
            Self::ServerPassword => f.write_str(INCORRECT_SERVER_PASSWORD),
            Self::Other(message) => f.write_str(message),
        }
    }
}

/// The server's own error string of a client error, when it sent one.
fn server_message(error: &overseer::net::Error) -> Option<&str> {
    match error {
        overseer::net::Error::Api(
            ApiError::Server(message)
            | ApiError::Status {
                message: Some(message),
                ..
            },
        ) => Some(message),
        _ => None,
    }
}

/// The text to show for a client error: the server's own error string when it sent one,
/// else the error and its causes.
pub fn message(error: &overseer::net::Error) -> String {
    server_message(error).map_or_else(|| describe(error), str::to_owned)
}

/// [`message`] for a library error: the server's own error string when it sent one, else the
/// error and its causes.
pub fn error_message(error: &overseer::Error) -> String {
    match error {
        overseer::Error::Net(error) => message(error),
        error => describe(error),
    }
}

/// Whether `error` of a user lookup (`Users::find`) says that no user has the name. The
/// server sends no status or code for it: only its own error string tells (`user not
/// found`; community servers fail with a `TypeError: …` instead). A failure without the
/// server's string (transport, decoding) is never an unknown user.
pub fn is_unknown_user(error: &overseer::Error) -> bool {
    let overseer::Error::Net(error) = error else {
        return false;
    };
    server_message(error)
        .is_some_and(|message| message.contains("not found") || message.starts_with("TypeError"))
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

/// The operations of the server with base URL `server` without an account (its subsystems,
/// for the sign-in screen, and registration and its checks), sending `server_password` or else
/// the stored one, and forcing the contract `server_kind` unless `None`. Discovers the server
/// (one `GET /api/version`, and `GET /api/authmod` on community servers); needs no account.
///
/// # Errors
///
/// Returns [`Failure::ServerPassword`] when the server wants a (correct) server password, and
/// [`Failure::Other`] when the address cannot be resolved, the client cannot be built or the
/// server cannot be discovered.
pub async fn anonymous_api(
    platform: &Platform,
    server: &str,
    server_kind: Option<ServerKind>,
    server_password: Option<String>,
) -> Result<Api, Failure> {
    let url = platform.server_url(server).await.map_err(Failure::Other)?;
    let server_password = match server_password {
        Some(password) => Some(password),
        None => stored_server_password(platform, server).await,
    };
    let mut builder = Client::builder(url);
    if let Some(password) = server_password {
        builder = builder.server_password(password);
    }
    if let Some(kind) = server_kind {
        builder = builder.server_kind(kind);
    }
    let client = builder
        .build()
        .map_err(|error| Failure::Other(describe(&error)))?;
    client.api().await.map_err(|error| Failure::of_net(&error))
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

/// Waits until the request budget of `overseer`'s session lets `pending + 1` more requests of
/// `priority` go (`Client::budget_ready_at`). A server without rate limits has no budget: this
/// returns at once.
pub async fn wait_for_budget(overseer: &Overseer, priority: Priority, pending: usize) {
    while let Some(at) = overseer.client().budget_ready_at(priority, pending) {
        rt::sleep_until(at).await;
    }
}

#[cfg(test)]
mod tests {
    use overseer::core::api::ApiError;
    use overseer::net::Error;

    use super::{Failure, is_unknown_user, message};

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
        assert_eq!(
            Failure::ServerPassword.to_string(),
            "incorrect server password"
        );
        assert_eq!(Failure::Other("down".to_owned()).to_string(), "down");
    }

    #[test]
    fn unknown_users() {
        let server =
            |text: &str| overseer::Error::Net(Error::Api(ApiError::Server(text.to_owned())));
        assert!(is_unknown_user(&server("user not found")));
        assert!(is_unknown_user(&server(
            "TypeError: Cannot read properties of null (reading 'steam')"
        )));
        assert!(!is_unknown_user(&server("unauthorized")));
        assert!(
            !is_unknown_user(&overseer::Error::Net(Error::UnexpectedReply(
                "user not found"
            ))),
            "only the server's own error string tells"
        );
        assert!(!is_unknown_user(&overseer::Error::Closed));
    }
}
