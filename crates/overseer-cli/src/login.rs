//! The `login` and `logout` commands. They sign in to a saved server and save
//! the sign-in, or delete it. The desktop app uses the same sign-ins. The
//! commands need no configuration file.
//!
//! `login --email` never saves the password. On a server with API tokens, it
//! creates a full API token and saves it. On other servers, it saves the
//! session token of the sign-in. A private server drops a session token 60 s
//! after it issued it, so a session token is good only for the next minute,
//! or while a session uses it.

use std::io::Write;

use overseer::core::api::{ApiError, Refusal};
use overseer::core::{ServerUrl, Token};
use overseer::model::account::AuthTokenRequest;
use overseer::model::platform::Subsystem;
use overseer::net::Outcome;
use overseer::{Auth, Overseer};
use overseer_storage::profiles::{Change, Server, ServerKind, server_password_key, token_key};

use crate::config::Sensitive;
use crate::saved::{self, Saved};
use crate::{Error, connection, prompt, servers};

/// The arguments of `login`.
#[derive(Debug, clap::Args)]
#[command(group(clap::ArgGroup::new("method").required(true).args(["token", "email"])))]
pub(crate) struct Login {
    /// The saved server: its name or its URL. The CLI saves a URL that is not
    /// saved yet
    server: String,
    /// Sign in with an API token. The CLI reads the token from stdin: a
    /// prompt without echo on a terminal, else one line
    #[arg(long)]
    token: bool,
    /// Sign in with the email (or the username) and the password of the
    /// account. The CLI reads the password from stdin. It saves an API token
    /// where the server has API tokens, else a session token, never the
    /// password
    #[arg(long, value_name = "EMAIL")]
    email: Option<String>,
    /// Also read the server password of a private server from stdin (after
    /// the token or the password), and save it
    #[arg(long)]
    server_password: bool,
}

/// The arguments of `logout`.
#[derive(Debug, clap::Args)]
pub(crate) struct Logout {
    /// The saved server: its name or its URL
    server: String,
    /// First delete the saved API token on the server. If the CLI cannot
    /// find the token on the server, it says so and signs out all the same
    #[arg(long)]
    revoke: bool,
}

/// A sign-in that failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Failure {
    /// The saved URL is not a server URL.
    #[error("{server}: {source}")]
    Url {
        /// The name of the server.
        server: String,
        /// Why the URL is not valid.
        source: overseer::core::ServerUrlError,
    },
    /// The server refuses the sign-in, or the connection fails.
    #[error("cannot sign in to {server}: {source}")]
    SignIn {
        /// The name of the server.
        server: String,
        /// The failure.
        source: overseer::Error,
    },
    /// A call after the sign-in fails.
    #[error("{server}: cannot {action}: {source}")]
    Api {
        /// The name of the server.
        server: String,
        /// The call.
        action: &'static str,
        /// The failure.
        source: overseer::Error,
    },
    /// The server signs in no account.
    #[error("{0}: the server signed in no account")]
    NoAccount(String),
    /// The server gives no session token.
    #[error("{0}: the server gave no session token")]
    NoToken(String),
    /// Another process removed the server during the sign-in.
    #[error("{0}: the server is no longer saved; save it again and sign in again")]
    Removed(String),
    /// The change of the saved servers is not valid.
    #[error("the saved servers: {0}")]
    Change(String),
    /// The revocation fails. The CLI keeps the sign-in.
    #[error(
        "cannot revoke the API token on {server}: {source}; to sign out of the CLI only, run `overseer logout` without --revoke"
    )]
    Revoke {
        /// The name of the server.
        server: String,
        /// The failure.
        source: overseer::Error,
    },
}

/// The token that a sign-in saves.
struct Signed {
    /// The id of the account.
    user_id: String,
    /// The username of the account.
    username: Option<String>,
    /// The token. `None` saves the last session token.
    token: Option<Sensitive>,
    /// Why the server created no API token after `--email`, on a server with
    /// API tokens.
    no_api_token: Option<String>,
}

impl Login {
    /// Signs in to the server and saves the sign-in in `saved`. Prints to
    /// `out`.
    pub(crate) async fn run(self, saved: &Saved, out: &mut dyn Write) -> Result<(), Error> {
        let server = match saved.find(&self.server) {
            Ok(server) => server,
            Err(saved::Error::NotSaved { .. }) if is_url(&self.server) => {
                let server = servers::add(saved, &self.server, None)?;
                writeln!(out, "saved server {} ({})", server.name, server.url)?;
                server
            }
            Err(error) => return Err(error.into()),
        };
        let url = parse_url(&server)?;
        let secret = prompt::secret(if self.token { "API token" } else { "password" })?;
        let new_password = if self.server_password {
            Some(prompt::secret("server password")?)
        } else {
            None
        };
        let server_password = match &new_password {
            Some(password) => Some(password.clone()),
            None if server.kind == ServerKind::Private => {
                saved.secret(&server_password_key(&server.url))?
            }
            None => None,
        };
        let given = self.token.then(|| secret.clone());
        let auth = match &self.email {
            Some(email) => Auth::Credentials {
                email: email.clone(),
                password: secret.into_inner(),
            },
            None => Auth::Token(Token::new(secret.into_inner())),
        };
        writeln!(out, "signing in to {} ({})", server.name, server.url)?;
        let overseer = connection::open(
            &url,
            server.kind.forced(),
            auth,
            server_password.map(Sensitive::into_inner),
        )
        .await
        .map_err(|source| Failure::SignIn {
            server: server.name.clone(),
            source,
        })?;
        let result = signed(&overseer, &server, given, self.email.is_some()).await;
        overseer.shutdown().await;
        let signed = result?;
        // The last token of the session: the server can replace a session token on each reply.
        let last = overseer
            .tokens()
            .borrow()
            .as_ref()
            .map(|token| Sensitive::new(token.as_str().to_owned()));
        let (token, api) = match signed.token {
            Some(token) => (token, true),
            None => (
                last.ok_or_else(|| Failure::NoToken(server.name.clone()))?,
                false,
            ),
        };
        saved.set_secret(&token_key(&server.url), &token)?;
        if let Some(password) = &new_password {
            saved.set_secret(&server_password_key(&server.url), password)?;
        }
        let listed = saved.update(|profiles| {
            profiles
                .apply(Change::SignedIn {
                    url: server.url.clone(),
                    user_id: Some(signed.user_id.clone()),
                    api_token: Some(api),
                    login: self.email.clone(),
                })
                .and_then(|_| {
                    profiles.apply(Change::Username {
                        url: server.url.clone(),
                        username: signed.username.clone(),
                    })
                })
                .map(|_| profiles.server(&server.url).is_some())
        })?;
        if !listed.map_err(Failure::Change)? {
            return Err(Failure::Removed(server.name).into());
        }
        let account = signed.username.as_deref().unwrap_or(&signed.user_id);
        if let Some(reason) = &signed.no_api_token {
            writeln!(
                out,
                "the server created no API token ({reason}): the CLI saves the session token"
            )?;
        }
        if api {
            writeln!(
                out,
                "signed in to {} as {account}: saved an API token",
                server.name
            )?;
        } else {
            writeln!(
                out,
                "signed in to {} as {account}: saved a session token\n\
                 caution: the server drops a session token 60 s after it issued it; \
                 use it in the next minute, or sign in again",
                server.name
            )?;
        }
        Ok(())
    }
}

/// Reads the account of the new session `overseer` on `server`. Chooses the
/// token to save: the token `given` with `--token` if the server keeps it,
/// a new API token after `--email` (`password`) on a server with API tokens,
/// else (`None`) the last session token. When the server certainly created
/// no API token (the request was not applied, or HTTP 404: a server without
/// the token routes), the CLI saves the session token.
async fn signed(
    overseer: &Overseer,
    server: &Server,
    given: Option<Sensitive>,
    password: bool,
) -> Result<Signed, Failure> {
    let api = |action: &'static str| {
        move |source| Failure::Api {
            server: server.name.clone(),
            action,
            source,
        }
    };
    let user_id = overseer
        .user_id()
        .ok_or_else(|| Failure::NoAccount(server.name.clone()))?
        .to_owned();
    let me = overseer
        .account()
        .me()
        .await
        .map_err(api("read the account"))?;
    let mut no_api_token = None;
    let token = if password {
        if overseer.world().supports(Subsystem::ApiTokens) {
            let request = AuthTokenRequest {
                kind: Some("full".to_owned()),
                ..AuthTokenRequest::default()
            };
            match overseer.account().create_auth_token(request).await {
                Ok(token) => Some(Sensitive::new(token.as_str().to_owned())),
                Err(error) if not_created(&error) => {
                    no_api_token = Some(error.to_string());
                    None
                }
                Err(error) => return Err(api("create an API token")(error)),
            }
        } else {
            None
        }
    } else {
        // A server replaces a session token, never an API token.
        let last = overseer.tokens().borrow().clone();
        given.filter(|given| last.is_none_or(|last| last.as_str() == given.expose()))
    };
    Ok(Signed {
        user_id,
        username: me.username.clone(),
        token,
        no_api_token,
    })
}

/// Whether the failed creation of an API token certainly created no token:
/// the request was not applied, or the server has no such route (HTTP 404).
fn not_created(error: &overseer::Error) -> bool {
    error.outcome() == Outcome::NotApplied || not_found(error)
}

/// Whether the server answered HTTP 404: it has no such route.
fn not_found(error: &overseer::Error) -> bool {
    matches!(
        error,
        overseer::Error::Net(overseer::net::Error::Api(ApiError::Status { status, .. }))
            if status.as_u16() == 404
    )
}

impl Logout {
    /// Deletes the saved sign-in of the server in `saved`. With `--revoke`,
    /// first deletes the API token on the server. Prints to `out`.
    pub(crate) async fn run(self, saved: &Saved, out: &mut dyn Write) -> Result<(), Error> {
        let server = saved.find(&self.server)?;
        let key = token_key(&server.url);
        let token = saved.secret(&key)?;
        if self.revoke {
            revoke(saved, &server, token.as_ref(), out).await?;
        }
        saved.delete_secret(&key)?;
        saved
            .update(|profiles| {
                profiles.apply(Change::SignedOut {
                    url: server.url.clone(),
                })
            })?
            .map_err(Failure::Change)?;
        let state = if token.is_some() || server.user.is_some() {
            "deleted the saved token"
        } else {
            "no saved sign-in"
        };
        writeln!(
            out,
            "signed out of {} ({}): {state}",
            server.name, server.url
        )?;
        Ok(())
    }
}

/// Deletes the API token `token` of `server` on the server. Prints why the
/// CLI cannot do it, if so.
async fn revoke(
    saved: &Saved,
    server: &Server,
    token: Option<&Sensitive>,
    out: &mut dyn Write,
) -> Result<(), Error> {
    let Some(token) = token else {
        writeln!(out, "no saved token: nothing to revoke")?;
        return Ok(());
    };
    if server.user.as_ref().and_then(|user| user.api_token) == Some(false) {
        writeln!(
            out,
            "the saved token is a session token: nothing to revoke; the server drops it soon"
        )?;
        return Ok(());
    }
    let server_password = if server.kind == ServerKind::Private {
        saved.secret(&server_password_key(&server.url))?
    } else {
        None
    };
    let failure = |source| Failure::Revoke {
        server: server.name.clone(),
        source,
    };
    let overseer = match connection::open(
        &parse_url(server)?,
        server.kind.forced(),
        Auth::Token(Token::new(token.expose().to_owned())),
        server_password.map(Sensitive::into_inner),
    )
    .await
    {
        Ok(overseer) => overseer,
        Err(error) if error.refusal() == Some(Refusal::Unauthorized) => {
            writeln!(out, "the server refuses the saved token: nothing to revoke")?;
            return Ok(());
        }
        Err(error) => return Err(failure(error).into()),
    };
    let result = revoke_on(&overseer, token).await;
    overseer.shutdown().await;
    let message = match result {
        Err(error) if not_found(&error) => {
            "the server has no API token routes (HTTP 404): nothing to revoke".to_owned()
        }
        result => match result.map_err(failure)? {
            Revoked::Unsupported => "the server has no API tokens: nothing to revoke".to_owned(),
            Revoked::Done => "revoked the API token on the server".to_owned(),
            Revoked::Unknown(0) => {
                "cannot identify the API token on the server: no entry matches; \
             revoke it in the account settings of the server"
                    .to_owned()
            }
            Revoked::Unknown(count) => format!(
                "cannot identify the API token on the server: {count} entries match; \
             revoke it in the account settings of the server"
            ),
        },
    };
    writeln!(out, "{message}")?;
    Ok(())
}

/// The outcome of a revocation.
enum Revoked {
    /// The server has no API tokens.
    Unsupported,
    /// The server deleted the token.
    Done,
    /// The number of entries of the token list that match the token is not 1.
    Unknown(usize),
}

/// Deletes the API token `token` of the account of `overseer`. The token
/// list gives the start of each token (8 characters on screeps.com), so
/// exactly one entry must match.
async fn revoke_on(overseer: &Overseer, token: &Sensitive) -> Result<Revoked, overseer::Error> {
    if !overseer.world().supports(Subsystem::ApiTokens) {
        return Ok(Revoked::Unsupported);
    }
    let entries = overseer.account().auth_tokens().await?;
    let matching: Vec<_> = entries
        .iter()
        .filter(|entry| !entry.token.is_empty() && token.expose().starts_with(&entry.token))
        .collect();
    match matching.as_slice() {
        [entry] => {
            overseer.account().delete_auth_token(&entry.id).await?;
            Ok(Revoked::Done)
        }
        other => Ok(Revoked::Unknown(other.len())),
    }
}

/// The URL of the saved server `server`.
fn parse_url(server: &Server) -> Result<ServerUrl, Failure> {
    ServerUrl::parse(&server.url).map_err(|source| Failure::Url {
        server: server.name.clone(),
        source,
    })
}

/// Whether `key`, which names no saved server, is a URL: it has `://`, a
/// `.`, a `:` or a `/`. A name without these characters is a typing error.
fn is_url(key: &str) -> bool {
    key.contains(['.', ':', '/'])
}

#[cfg(test)]
mod tests {
    use super::is_url;

    /// A name without a dot, a colon or a slash is not a URL, so `login` does
    /// not save a server for a misspelled name.
    #[test]
    fn urls_and_names() {
        for url in ["http://127.0.0.1:21025", "localhost:21025", "example.com"] {
            assert!(is_url(url), "{url}");
        }
        for name in ["e2e", "local", "my-server"] {
            assert!(!is_url(name), "{name}");
        }
    }
}
