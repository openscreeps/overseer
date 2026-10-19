//! The Overseer sessions of the commands. The library does the discovery, the
//! sign-in, HTTP, the sockets and the token rotation.
//!
//! A session that signs in with the saved token of a server saves each new
//! token of the server ([`Overseer::tokens`]) until the session stops. A
//! private server replaces a session token on each reply and drops the old
//! token 60 s after it issued it. The next run of the CLI, or the desktop
//! app, thus finds a token that is still valid.

use std::ops::{Deref, DerefMut};

use overseer::core::ServerUrl;
use overseer::core::api::Refusal;
use overseer::model::platform::ServerKind;
use overseer::{Auth, Overseer, ServerProfile};
use overseer_storage::native::Storage;
use overseer_storage::profiles::token_key;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::config::{Credentials, Sensitive, Server};
use crate::saved::{self, Saved};

/// A session of a server. The caller must call [`Self::shutdown`] before
/// its command returns. This rule also applies when the command fails.
pub(crate) struct Session {
    overseer: Overseer,
    /// The task that saves each new token, for a saved token.
    follower: Option<Follower>,
}

/// The task that saves each new token of a session.
struct Follower {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Deref for Session {
    type Target = Overseer;

    fn deref(&self) -> &Overseer {
        &self.overseer
    }
}

impl DerefMut for Session {
    fn deref_mut(&mut self) -> &mut Overseer {
        &mut self.overseer
    }
}

impl Session {
    /// Stops the session. Then saves the last token of a saved token.
    pub(crate) async fn shutdown(self) {
        self.overseer.shutdown().await;
        if let Some(follower) = self.follower {
            // The task stops on its own when the token watch closes.
            let _ = follower.stop.send(());
            // A panic of the task is already on stderr.
            let _ = follower.task.await;
        }
    }
}

/// Connects to `server` with `credentials` through the `overseer` library.
/// With a saved token, the session saves each new token in `saved`.
///
/// # Errors
///
/// Returns [`Error::Refused`] when the server refuses the saved token, and
/// [`Error::Api`] for the other failures of the connection.
pub(crate) async fn connect(
    server: &Server,
    credentials: Credentials,
    saved: &Saved,
) -> Result<Session, Error> {
    let follow = match &credentials.saved_token {
        Some(token) => Some((saved.storage()?.clone(), token.expose().to_owned())),
        None => None,
    };
    let overseer = open(
        &server.url,
        server.kind,
        credentials.auth,
        credentials.server_password.map(Sensitive::into_inner),
    )
    .await
    .map_err(|source| {
        if follow.is_some() && source.refusal() == Some(Refusal::Unauthorized) {
            Error::Refused {
                login: server.login.clone(),
                source: Box::new(source),
            }
        } else {
            Error::Api(source)
        }
    })?;
    let follower = follow.map(|(storage, token)| {
        Follower::start(
            &overseer,
            storage,
            token_key(server.url.base().as_str()),
            token,
        )
    });
    Ok(Session { overseer, follower })
}

/// Connects to the server at `url` with `auth` and the server password
/// `server_password`. `kind` forces the contract; `None` discovers it. The
/// caller must call `shutdown` on the session before its command returns.
pub(crate) async fn open(
    url: &ServerUrl,
    kind: Option<ServerKind>,
    auth: Auth,
    server_password: Option<String>,
) -> Result<Overseer, overseer::Error> {
    Overseer::connect(ServerProfile {
        url: url.clone(),
        auth,
        server_password,
        default_shard: None,
        server_kind: kind,
        plugins: None,
    })
    .await
}

impl Follower {
    /// Starts a task that saves each new token of `overseer` under `key` in
    /// `storage`. `stored` is the token in `storage` now.
    fn start(overseer: &Overseer, storage: Storage, key: String, mut stored: String) -> Self {
        let mut tokens = overseer.tokens();
        let (stop, mut stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let mut stopping = false;
            loop {
                let token = tokens
                    .borrow_and_update()
                    .as_ref()
                    .map(|token| token.as_str().to_owned());
                if let Some(token) = token
                    && token != stored
                {
                    let (storage, key, value) = (storage.clone(), key.clone(), token.clone());
                    match tokio::task::spawn_blocking(move || storage.set_secret(&key, &value))
                        .await
                    {
                        Ok(Ok(())) => stored = token,
                        Ok(Err(error)) => {
                            eprintln!("overseer: cannot save the new session token: {error}");
                        }
                        Err(error) => {
                            eprintln!("overseer: cannot save the new session token: {error}");
                        }
                    }
                }
                if stopping {
                    break;
                }
                tokio::select! {
                    changed = tokens.changed() => if changed.is_err() {
                        break;
                    },
                    _ = &mut stopped => stopping = true,
                }
            }
        });
        Self { stop, task }
    }
}

/// A connection that failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The server refuses the saved token.
    #[error(
        "the server refuses the saved token: sign in again with `overseer login {login}` ({source})"
    )]
    Refused {
        /// The argument of `overseer login`.
        login: String,
        /// The refusal.
        source: Box<overseer::Error>,
    },
    /// The connection failed.
    #[error(transparent)]
    Api(#[from] overseer::Error),
    /// The saved secrets are not available.
    #[error(transparent)]
    Saved(#[from] saved::Error),
}
