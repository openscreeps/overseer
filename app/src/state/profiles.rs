//! The saved servers of the state ([`AppState::profiles`]) and their changes.
//!
//! In a browser, the app applies each change to its list and saves the list in `localStorage`.
//! In the shell, the list is the profiles file that the CLI (`overseer servers`) shares:
//!
//! 1. The app applies each change to its list at once (the page shows it and its errors).
//! 2. The app sends the change to the shell. The shell applies it to the profiles file under
//!    the lock of the file and returns the document of the file.
//! 3. The app takes that document as its list.
//!
//! When a window gains focus, the app loads the list again: the changes of the CLI show. The
//! main window then follows the sign-ins of the CLI ([`AppState::follow_saved`]). The calls to
//! the shell run one at a time, in order ([`Shell`]), so an older document never replaces a
//! newer one.

use std::cell::RefCell;
use std::collections::VecDeque;

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer_storage::profiles::{Change, Profiles, Server, token_key};
use tokio::sync::oneshot;

use super::AppState;
use crate::platform::{self, Platform};
use crate::route::Route;

/// Shown on the sign-in form when the saved sign-in ended in another program.
const SIGNED_OUT: &str = "You signed out in another program, for example the overseer command.";

/// A call to the shell about the profiles file.
enum Call {
    /// Applies a change to the profiles file.
    Change(Change),
    /// Loads the profiles file again (a window gained focus).
    Reload,
}

/// The state of the calls to the shell. A page has one app state, so it has one of these too.
#[derive(Default)]
struct Shell {
    /// The waiting calls, oldest first.
    waiting: VecDeque<Call>,
    /// A call runs.
    running: bool,
    /// Told when no call waits or runs ([`AppState::profiles_saved`]).
    idle: Vec<oneshot::Sender<()>>,
    /// The epoch of the session whose token is in the secret store (0: none yet).
    token_stored: u64,
}

thread_local! {
    static SHELL: RefCell<Shell> = RefCell::default();
}

/// Records that the session `epoch` stored its token in the secret store.
pub fn token_stored(epoch: u64) {
    SHELL.with_borrow_mut(|shell| shell.token_stored = epoch);
}

/// The oldest waiting call. `None` when none waits: then no call runs, and the waiters of
/// [`Shell::idle`] are told.
fn next_call() -> Option<Call> {
    SHELL.with_borrow_mut(|shell| {
        let call = shell.waiting.pop_front();
        shell.running = call.is_some();
        if call.is_none() {
            for waiter in shell.idle.drain(..) {
                // A waiter that went away needs nothing.
                let _ = waiter.send(());
            }
        }
        call
    })
}

/// The saved profiles at the start ([`Platform::load_profiles`]). Only the main window imports
/// the list of `localStorage`: a popped-out window never writes the profiles.
///
/// # Errors
///
/// Returns a message when the shell cannot read or write the profiles file.
pub async fn load(platform: &Platform) -> Result<Profiles, String> {
    let popout = Route::parse(&platform::location_hash())
        .is_some_and(|route| matches!(route, Route::Window { .. }));
    platform.load_profiles(!popout).await
}

impl AppState {
    /// Applies `change` to the saved profiles and saves them (not in a popped-out window, which
    /// changes its own list only). Returns what [`Profiles::apply`] returns.
    ///
    /// # Errors
    ///
    /// Returns the message of [`Profiles::apply`] (nothing is saved then). A failure to save
    /// shows in the banner.
    pub fn change_profiles(self, change: Change) -> Result<Option<String>, String> {
        let shell = !self.is_popout() && self.platform.with_value(Platform::is_tauri);
        let sent = shell.then(|| change.clone());
        let mut result = Ok(None);
        self.profiles
            .update(|profiles| result = profiles.apply(change));
        if result.is_err() || self.is_popout() {
            return result;
        }
        if let Some(change) = sent {
            self.call(Call::Change(change));
        } else if let Err(error) = self.profiles.with_untracked(platform::save_profiles) {
            self.banner
                .set(Some(format!("Could not save the server list: {error}")));
        }
        result
    }

    /// Waits until the shell saved the changes of the profiles made so far (at once in a
    /// browser). Before a secret is deleted, this keeps the profiles file from naming a sign-in
    /// whose token is gone, also when the app closes at once.
    pub async fn profiles_saved(self) {
        let (waiter, saved) = oneshot::channel();
        let waits = SHELL.with_borrow_mut(|shell| {
            if shell.running {
                shell.idle.push(waiter);
            }
            shell.running
        });
        if waits {
            // An error says only that the queue went away with the page.
            let _ = saved.await;
        }
    }

    /// In the shell, loads the profiles file again and follows its changes.
    pub(super) fn follow_storage(self) {
        if !self.platform.with_value(Platform::is_tauri) {
            return;
        }
        let focus = window_event_listener(leptos::ev::focus, move |_| self.call(Call::Reload));
        // The app lives as long as the page.
        std::mem::forget(focus);
    }

    /// Queues `call`, and runs the queue when no call runs.
    fn call(self, call: Call) {
        let start = SHELL.with_borrow_mut(|shell| {
            shell.waiting.push_back(call);
            !std::mem::replace(&mut shell.running, true)
        });
        if start {
            spawn_local(async move {
                while let Some(call) = next_call() {
                    self.run(call).await;
                }
            });
        }
    }

    /// Runs `call` and takes the document that the shell returns.
    async fn run(self, call: Call) {
        let platform = self.platform();
        match call {
            Call::Change(change) => match platform.change_profiles(&change).await {
                Ok(profiles) => self.take(profiles),
                Err(error) => self
                    .banner
                    .set(Some(format!("Could not save the server list: {error}"))),
            },
            Call::Reload => match platform.load_profiles(false).await {
                Ok(profiles) => {
                    let before = self.route_server();
                    self.take(profiles);
                    if !self.is_popout() {
                        let changed = before != self.route_server();
                        // Outside the queue: signing out waits for the queue
                        // ([`Self::profiles_saved`]).
                        spawn_local(self.follow_saved(changed));
                    }
                }
                Err(error) => self
                    .banner
                    .set(Some(format!("Could not load the server list: {error}"))),
            },
        }
    }

    /// Takes `profiles` as the list; no change keeps the signal quiet.
    fn take(self, profiles: Profiles) {
        if self.profiles.with_untracked(|current| *current != profiles) {
            self.profiles.set(profiles);
        }
    }

    /// The saved server of the route.
    fn route_server(self) -> Option<Server> {
        self.route
            .with_untracked(|route| route.server().and_then(|slug| self.server_by_slug(slug)))
    }

    /// After a reload, follows what another program (the CLI) changed. A live session ends as
    /// with signing out when its server has no saved account or no stored token any more. It
    /// closes when another account signed in: the route then resumes that account. A session
    /// whose saved account and token stay is not disturbed. When the saved server of the route
    /// changed (`route_server_changed`) and it has no live session, the route is followed
    /// again: a server added or signed in outside the app opens.
    pub(super) async fn follow_saved(self, route_server_changed: bool) {
        if let Some(session) = self.session.get_untracked() {
            let user = self.server(&session.server).and_then(|server| server.user);
            match user {
                None => self.end(true, Some(SIGNED_OUT.to_owned())).await,
                Some(user) if session.overseer.user_id() != Some(user.id.as_str()) => {
                    self.disconnect().await;
                }
                Some(_) => {
                    // Before the session stored its token, a missing token says nothing.
                    if SHELL.with_borrow(|shell| shell.token_stored) == session.epoch
                        && matches!(
                            self.platform()
                                .secret_get(&token_key(&session.server))
                                .await,
                            Ok(None)
                        )
                        && self.is_live(session.epoch)
                    {
                        self.end(true, Some(SIGNED_OUT.to_owned())).await;
                    }
                }
            }
        }
        let Some(server) = self.route_server() else {
            return;
        };
        let live = self.session.with_untracked(|session| {
            session
                .as_ref()
                .is_some_and(|session| session.server == server.url)
        });
        if route_server_changed && !live {
            let route = self.route.get_untracked();
            self.follow_route(&route);
        }
    }
}
