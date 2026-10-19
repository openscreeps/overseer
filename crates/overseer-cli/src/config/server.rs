//! The servers of the profiles. A server has the URL of its API and one way to
//! sign in to an account on it: the credentials of a `[servers.NAME]` table,
//! or the saved sign-in of `overseer login`.

use std::fmt;

use overseer::core::{ServerUrl, Token};
use overseer::model::platform::ServerKind;
use overseer_storage::profiles::{self as saved, Profiles, server_password_key, token_key};
use serde::{Deserialize, Deserializer};

use super::name::ServerName;
use super::secret::{Secret, SecretError, Sensitive};
use crate::envfile::EnvFile;
use crate::saved::Saved;

/// Deserializes the URL of a server. The library checks the URL.
fn server_url<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ServerUrl, D::Error> {
    let text = String::deserialize(deserializer)?;
    ServerUrl::parse(&text).map_err(serde::de::Error::custom)
}

/// How the CLI signs in to a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Auth {
    /// An API token in the configuration. On the official server, the
    /// account settings have a page for auth tokens.
    Token(Secret),
    /// The email and the password of the account in the configuration
    /// (screepsmod-auth).
    Password {
        /// The email of the account.
        email: String,
        /// The password of the account.
        password: Secret,
    },
    /// The saved token of the server (`overseer login`).
    Saved(SavedToken),
    /// No credentials: the configuration has none, and no account is signed
    /// in on the saved server.
    Missing,
}

/// The kind of a saved token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SavedToken {
    /// A persistent API token.
    Api,
    /// A session token. A private server replaces it on each reply and
    /// drops it 60 s after it issued it.
    Session,
    /// A token that the app saved before it recorded the kind.
    Unknown,
}

impl SavedToken {
    /// The kind of the token of `user`.
    const fn of(user: &saved::SavedUser) -> Self {
        match user.api_token {
            Some(true) => Self::Api,
            Some(false) => Self::Session,
            None => Self::Unknown,
        }
    }
}

impl fmt::Display for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token(token) => write!(f, "token {token}"),
            Self::Password { email, password } => write!(f, "{email}, password {password}"),
            Self::Saved(SavedToken::Api) => f.write_str("saved API token"),
            Self::Saved(SavedToken::Session) => f.write_str("saved session token"),
            Self::Saved(SavedToken::Unknown) => f.write_str("saved token"),
            Self::Missing => f.write_str("none"),
        }
    }
}

/// The credentials of a server for one connection.
#[derive(Debug)]
pub(crate) struct Credentials {
    /// The credentials for the library.
    pub(crate) auth: overseer::Auth,
    /// The server password (`X-Server-Password`).
    pub(crate) server_password: Option<Sensitive>,
    /// The saved token, when the CLI signs in with it. The session saves
    /// each new token of the server.
    pub(crate) saved_token: Option<Sensitive>,
}

/// A server of a profile: the URL of its API and how to sign in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Server {
    /// The URL of the API.
    pub(crate) url: ServerUrl,
    /// How to sign in.
    pub(crate) auth: Auth,
    /// The contract to force (saved screeps.com servers). `None` discovers it.
    pub(crate) kind: Option<ServerKind>,
    /// Whether the server password comes from the saved secrets (a saved
    /// private server).
    pub(crate) saved_password: bool,
    /// The argument of `overseer login` for this server.
    pub(crate) login: String,
    /// The account of the server: two profiles with the same identity use
    /// one account.
    pub(crate) identity: String,
}

impl Server {
    /// The server of the table `[servers.name]`. Without credentials in the
    /// table, the CLI uses the saved sign-in of its URL.
    pub(crate) fn of_file(name: &ServerName, file: &FileServer, profiles: &Profiles) -> Self {
        let url = file.url.base().as_str();
        let entry = profiles.server(url);
        let (auth, identity) = match &file.auth {
            Some(auth) => (auth.clone(), format!("file:{name}")),
            None => (saved_auth(entry), format!("saved:{url}")),
        };
        Self {
            url: file.url.clone(),
            auth,
            kind: None,
            saved_password: entry.is_some_and(|entry| entry.kind == saved::ServerKind::Private),
            login: login_name(profiles, entry, url),
            identity,
        }
    }

    /// The saved server `entry` of `profiles`.
    ///
    /// # Errors
    ///
    /// Returns an error when the saved URL is not a server URL.
    pub(crate) fn of_saved(
        entry: &saved::Server,
        profiles: &Profiles,
    ) -> Result<Self, overseer::core::ServerUrlError> {
        Ok(Self {
            url: ServerUrl::parse(&entry.url)?,
            auth: saved_auth(Some(entry)),
            kind: entry.kind.forced(),
            saved_password: entry.kind == saved::ServerKind::Private,
            login: login_name(profiles, Some(entry), &entry.url),
            identity: format!("saved:{}", entry.url),
        })
    }

    /// Returns the credentials for one connection. A secret comes from the
    /// file, the process environment, the `--env-file` variables `env`, or
    /// the saved secrets `saved`.
    ///
    /// # Errors
    ///
    /// Returns an error when a secret is not available, or when the server
    /// has no credentials.
    pub(crate) fn credentials(
        &self,
        env: &EnvFile,
        saved: &Saved,
    ) -> Result<Credentials, SecretError> {
        let url = self.url.base().as_str();
        let secret = |key: &str| {
            saved
                .secret(key)
                .map_err(|error| SecretError::Saved(error.to_string()))
        };
        let mut saved_token = None;
        let auth = match &self.auth {
            Auth::Token(token) => {
                overseer::Auth::Token(Token::new(token.resolve(env)?.into_inner()))
            }
            Auth::Password { email, password } => overseer::Auth::Credentials {
                email: email.clone(),
                password: password.resolve(env)?.into_inner(),
            },
            Auth::Saved(_) => {
                let token = secret(&token_key(url))?
                    .ok_or_else(|| SecretError::NoSavedToken(self.login.clone()))?;
                let auth = overseer::Auth::Token(Token::new(token.expose().to_owned()));
                saved_token = Some(token);
                auth
            }
            Auth::Missing => return Err(SecretError::NotSignedIn(self.login.clone())),
        };
        let server_password = if self.saved_password {
            secret(&server_password_key(url))?
        } else {
            None
        };
        Ok(Credentials {
            auth,
            server_password,
            saved_token,
        })
    }
}

/// The sign-in of the saved server `entry`.
fn saved_auth(entry: Option<&saved::Server>) -> Auth {
    entry
        .and_then(|entry| entry.user.as_ref())
        .map_or(Auth::Missing, |user| Auth::Saved(SavedToken::of(user)))
}

/// The argument of `overseer login` for the server at `url`: the name of its
/// saved entry `entry` when that name finds it, else the URL.
fn login_name(profiles: &Profiles, entry: Option<&saved::Server>, url: &str) -> String {
    entry
        .filter(|entry| {
            !entry.name.contains(char::is_whitespace)
                && matches!(profiles.find(&entry.name), Ok(Some(found)) if found.url == entry.url)
        })
        .map_or_else(|| url.to_owned(), |entry| entry.name.clone())
}

/// A `[servers.NAME]` table: the URL of the API, and the credentials if the
/// table has them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "ServerFile")]
pub(crate) struct FileServer {
    /// The URL of the API.
    pub(crate) url: ServerUrl,
    /// The credentials ([`Auth::Token`] or [`Auth::Password`]), or `None` for
    /// the saved sign-in.
    pub(crate) auth: Option<Auth>,
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

impl TryFrom<ServerFile> for FileServer {
    type Error = AuthError;

    fn try_from(file: ServerFile) -> Result<Self, AuthError> {
        let auth = match (file.token, file.email, file.password) {
            (None, None, None) => None,
            (Some(token), None, None) => Some(Auth::Token(token)),
            (None, Some(email), Some(password)) => Some(Auth::Password { email, password }),
            (Some(_), _, _) => return Err(AuthError::Both),
            (None, _, _) => return Err(AuthError::Incomplete),
        };
        Ok(Self {
            url: file.url,
            auth,
        })
    }
}

/// A server that has more than one way to sign in, or half of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AuthError {
    /// An email without a password, or a password without an email.
    #[error("set `email` and `password` together, or set `token`")]
    Incomplete,
    /// A token and an email or a password.
    #[error("set `token`, or `email` and `password`, not both")]
    Both,
}
