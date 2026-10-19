//! The servers of the configuration. A server has the URL of its API and one
//! way to sign in to an account on it.

use std::fmt;

use overseer::core::{ServerUrl, Token};
use serde::{Deserialize, Deserializer};

use super::secret::{Secret, SecretError};
use crate::envfile::EnvFile;

/// Deserializes the URL of a server. The library checks the URL.
fn server_url<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ServerUrl, D::Error> {
    let text = String::deserialize(deserializer)?;
    ServerUrl::parse(&text).map_err(serde::de::Error::custom)
}

/// How the CLI signs in to a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Auth {
    /// An API token. On the official server, the account settings have a
    /// page for auth tokens.
    Token(Secret),
    /// The email and the password of the account (screepsmod-auth).
    Password {
        /// The email of the account.
        email: String,
        /// The password of the account.
        password: Secret,
    },
}

impl Auth {
    /// Returns the credentials for the library. The secret comes from the
    /// file, the process environment, or the `--env-file` variables `env`.
    ///
    /// # Errors
    ///
    /// Returns an error when the secret is not available.
    pub(crate) fn resolve(&self, env: &EnvFile) -> Result<overseer::Auth, SecretError> {
        Ok(match self {
            Self::Token(token) => {
                overseer::Auth::Token(Token::new(token.resolve(env)?.into_inner()))
            }
            Self::Password { email, password } => overseer::Auth::Credentials {
                email: email.clone(),
                password: password.resolve(env)?.into_inner(),
            },
        })
    }
}

impl fmt::Display for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => write!(f, "token {token}"),
            Self::Password { email, password } => write!(f, "{email}, password {password}"),
        }
    }
}

/// A server: the URL of its API and how to sign in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "ServerFile")]
pub(crate) struct Server {
    /// The URL of the API.
    pub(crate) url: ServerUrl,
    /// How to sign in.
    pub(crate) auth: Auth,
}

/// A server in the form of the file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerFile {
    #[serde(deserialize_with = "server_url")]
    url: ServerUrl,
    token: Option<Secret>,
    email: Option<String>,
    password: Option<Secret>,
}

impl TryFrom<ServerFile> for Server {
    type Error = AuthError;

    fn try_from(file: ServerFile) -> Result<Self, AuthError> {
        let auth = match (file.token, file.email, file.password) {
            (Some(token), None, None) => Auth::Token(token),
            (None, Some(email), Some(password)) => Auth::Password { email, password },
            (Some(_), _, _) => return Err(AuthError::Both),
            (None, _, _) => return Err(AuthError::Missing),
        };
        Ok(Self {
            url: file.url,
            auth,
        })
    }
}

/// A server that does not have exactly one way to sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AuthError {
    /// Neither a token nor an email and a password.
    #[error("set `token`, or `email` and `password`")]
    Missing,
    /// A token and an email or a password.
    #[error("set `token`, or `email` and `password`, not both")]
    Both,
}
