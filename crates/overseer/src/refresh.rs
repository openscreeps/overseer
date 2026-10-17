//! Keeps rotating session tokens alive: their servers let a token expire a minute (by default)
//! after issuing it, and issue the next one with every authenticated reply.

use std::sync::Arc;
use std::time::Duration;

use overseer_net::api::AccountApi as _;
use overseer_net::core::Token;
use overseer_net::core::api::ApiError;
use overseer_net::rt::{self, Instant, JoinHandle};
use overseer_net::{Api, Error};
use tokio::sync::watch;

use crate::live::Registry;
use crate::{ConnectionStatus, Notice};

/// How long a rotated token may go without a successor before the session asks for one: well
/// within the default one-minute lifetime, leaving time for a second attempt.
pub(crate) const REFRESH_AFTER: Duration = Duration::from_secs(25);

/// Spawns the task refreshing the token of `api`'s client until `status` is
/// [`Closed`](ConnectionStatus::Closed) (or its sender is gone): a tokio task natively, a task on
/// the browser's event loop in WebAssembly. A refresh is an authenticated `GET /api/auth/me`.
pub(crate) fn spawn(
    api: Api,
    status: watch::Receiver<ConnectionStatus>,
    registry: Arc<Registry>,
) -> JoinHandle<()> {
    let tokens = api.client().token_updates();
    rt::spawn(
        Refresh {
            api,
            tokens,
            status,
            registry,
        }
        .run(),
    )
}

struct Refresh {
    api: Api,
    /// Every token change is a fresh token: an authenticated reply, a sign-in, the socket's
    /// `auth ok` or one set by the app.
    tokens: watch::Receiver<Option<Token>>,
    status: watch::Receiver<ConnectionStatus>,
    registry: Arc<Registry>,
}

impl Refresh {
    async fn run(mut self) {
        let mut due = self.next_due();
        loop {
            tokio::select! {
                biased;
                () = closed(&mut self.status) => return,
                changed = self.tokens.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    due = self.next_due();
                }
                () = until(due) => {
                    let result = tokio::select! {
                        biased;
                        () = closed(&mut self.status) => return,
                        result = self.api.me() => result,
                    };
                    due = match result {
                        // The token expired (and no sign-in replaced it): wait for a new one,
                        // unless one arrived meanwhile.
                        Err(Error::Api(ApiError::Unauthorized)) => {
                            self.registry.notify(Notice::AuthFailed);
                            None
                        }
                        // Success rotated the token; other failures are tried again.
                        _ => self.next_due(),
                    };
                }
            }
        }
    }

    /// When the current token needs a successor: [`REFRESH_AFTER`] from now while a token is
    /// held and the server has rotated tokens, else never.
    fn next_due(&mut self) -> Option<Instant> {
        let held = self.tokens.borrow_and_update().is_some();
        (held && self.api.client().rotates_tokens()).then(|| Instant::now() + REFRESH_AFTER)
    }
}

/// Resolves once the session is closed.
pub(crate) async fn closed(status: &mut watch::Receiver<ConnectionStatus>) {
    // An error means the background task is gone, which also closes the session.
    let _ = status
        .wait_for(|status| *status == ConnectionStatus::Closed)
        .await;
}

/// Resolves at `due`; never without one.
async fn until(due: Option<Instant>) {
    match due {
        Some(due) => rt::sleep_until(due).await,
        None => std::future::pending().await,
    }
}
