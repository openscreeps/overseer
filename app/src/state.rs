//! The app's state: the route (URL hash), what is remembered across restarts, and the
//! session lifecycle (resume for the route's server, sign in, token rotation, expiry,
//! disconnect and sign-out).
//!
//! The route decides the server: when it names a server other than the live session's, that
//! server's saved session resumes from its stored token, or its sign-in form shows. The live
//! session stays connected while the server list is shown.
//!
//! A popped-out window ([`Route::Window`] at start) shares the stored tokens but never writes
//! the saved profiles or [`Saved`] state, and never deletes a token: the main window owns
//! them.

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::{Auth, ConnectionStatus, Notice, Overseer};
use overseer_storage::profiles::{Profiles, SavedUser, Server, token_key};
use tokio::sync::broadcast::error::RecvError;

use crate::platform::{self, Platform};
use crate::profiles::Slugs as _;
use crate::route::Route;
use crate::saved::Saved;
use crate::session::{self, Failure};

/// Shown when a stored token is rejected.
const EXPIRED: &str = "Your session has expired. Sign in again.";

/// The session side of the screen on display for a server route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Connecting or resuming a session, with what is happening.
    Starting(String),
    /// Sign-in to the server with base URL `server`, with a message (expired session, …).
    SignIn {
        /// Server base URL.
        server: String,
        /// Shown above the form.
        message: Option<String>,
    },
    /// Registration on the server with base URL `server` (private servers).
    Register {
        /// Server base URL.
        server: String,
    },
    /// The server could not be reached (its session stays saved).
    Failed {
        /// Server base URL.
        server: String,
        /// What went wrong.
        message: String,
    },
    /// The signed-in account has no username yet.
    SetUsername,
    /// The route's content (world, profile, room window).
    Ready,
}

impl Screen {
    /// Whether this is the sign-in, registration or failure screen of a server other than the
    /// one with base URL `url`.
    fn is_of_other_server(&self, url: &str) -> bool {
        match self {
            Self::SignIn { server, .. }
            | Self::Register { server }
            | Self::Failed { server, .. } => server != url,
            Self::Starting(_) | Self::SetUsername | Self::Ready => false,
        }
    }

    /// The screen of a session whose account has `username`: the username form while it has
    /// none, else the route's content.
    const fn of_account(username: Option<&str>) -> Self {
        if username.is_some() {
            Self::Ready
        } else {
            Self::SetUsername
        }
    }
}

/// The live session.
#[derive(Clone)]
pub struct Session {
    /// Base URL of the server.
    pub server: String,
    /// The connection.
    pub overseer: Overseer,
    /// Distinguishes this session from later ones in its background tasks.
    epoch: u64,
}

impl Session {
    /// Distinguishes this session from earlier and later ones.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// How a session began, which decides what [`AppState::begin`] remembers of the sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// Resumed from the stored token: the saved sign-in stays.
    Resumed,
    /// Signed in (or registered) with a name or e-mail and a password; the name, when given,
    /// fills the sign-in form next time.
    Password(Option<String>),
    /// Signed in with an API token (screeps.com).
    ApiToken,
}

/// Everything the components share; `Copy`, provided as context.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Where the app runs.
    pub platform: StoredValue<Platform>,
    /// Saved servers.
    pub profiles: RwSignal<Profiles>,
    /// Where the user was (main window only writes it).
    pub saved: RwSignal<Saved>,
    /// The route on display.
    pub route: RwSignal<Route>,
    /// The session side of the screen.
    pub screen: RwSignal<Screen>,
    /// The live session.
    pub session: RwSignal<Option<Session>, LocalStorage>,
    /// Connection state of the live session.
    pub status: RwSignal<Option<ConnectionStatus>>,
    /// A message for the user outside any form (server messages, failures of background work).
    pub banner: RwSignal<Option<String>>,
    /// This window shows a popped-out room.
    pub popout: StoredValue<bool>,
    /// Counter of sessions begun.
    epoch: StoredValue<u64>,
    /// Base URL of the server a session is being resumed for.
    resuming: StoredValue<Option<String>>,
}

impl AppState {
    /// A fresh state on the start screen.
    pub fn new(platform: Platform) -> Self {
        Self {
            platform: StoredValue::new(platform),
            profiles: RwSignal::new(platform::load_profiles()),
            saved: RwSignal::new(platform::load_saved()),
            route: RwSignal::new(Route::Servers),
            screen: RwSignal::new(Screen::Starting("Starting…".to_owned())),
            session: RwSignal::new_local(None),
            status: RwSignal::new(None),
            banner: RwSignal::new(None),
            popout: StoredValue::new(false),
            epoch: StoredValue::new(0),
            resuming: StoredValue::new(None),
        }
    }

    /// The state provided by [`crate::ui::App`].
    pub fn expect() -> Self {
        expect_context()
    }

    /// The platform.
    pub fn platform(self) -> Platform {
        self.platform.get_value()
    }

    /// Whether this window is a popped-out room.
    pub fn is_popout(self) -> bool {
        self.popout.get_value()
    }

    /// Starts on the URL's route, else the last route, else the server list; follows the
    /// URL hash (back, forward, edited links) and keeps the route's server connected.
    pub fn start(self) {
        let hash = platform::location_hash();
        let route = if hash.is_empty() || hash == "#" {
            self.saved
                .with_untracked(|saved| saved.route.clone())
                .filter(|route| !matches!(route, Route::Window { .. }))
                .unwrap_or(Route::Servers)
        } else {
            Route::parse(&hash).unwrap_or_else(|| {
                self.banner
                    .set(Some(format!("“{hash}” is not a page of Overseer.")));
                Route::Servers
            })
        };
        self.popout.set_value(matches!(route, Route::Window { .. }));
        self.replace_route(route);
        let listener = window_event_listener(leptos::ev::hashchange, move |_| {
            let hash = platform::location_hash();
            let route = Route::parse(&hash).unwrap_or(Route::Servers);
            if self.route.get_untracked() != route {
                self.route.set(route);
            }
        });
        // The app lives as long as the page.
        std::mem::forget(listener);
        Effect::new(move |_| {
            let route = self.route.get();
            self.follow_route(&route);
        });
    }

    /// Shows `route` as a new history entry.
    pub fn navigate(self, route: Route) {
        if self.route.get_untracked() == route {
            return;
        }
        platform::push_hash(&route.hash());
        self.route.set(route);
    }

    /// Shows `route` in place of the current history entry (camera-driven changes, resolved
    /// shards).
    pub fn replace_route(self, route: Route) {
        platform::replace_hash(&route.hash());
        if self.route.get_untracked() != route {
            self.route.set(route);
        }
    }

    /// Saves the route and connects its server.
    fn follow_route(self, route: &Route) {
        if !self.is_popout() && !matches!(route, Route::Window { .. }) {
            self.update_saved(|saved| saved.route = Some(route.clone()));
        }
        let Some(slug) = route.server() else {
            return;
        };
        let Some(server) = self.server_by_slug(slug) else {
            // The route's screen says the server is unknown.
            self.screen.set(Screen::Ready);
            return;
        };
        let live = self.session.get_untracked();
        if let Some(session) = live.filter(|session| session.server == server.url) {
            // Back from another server's form or failure: the live session's screen, as when
            // it began (the username form while the account has none).
            if self
                .screen
                .with_untracked(|screen| screen.is_of_other_server(&server.url))
            {
                self.screen
                    .set(Screen::Starting(format!("Connecting to {}…", server.name)));
                self.settle(session.epoch, server.url, session.overseer);
            }
            return;
        }
        if self.resuming.get_value().as_deref() == Some(server.url.as_str()) {
            return;
        }
        if server.user.is_some() {
            spawn_local(async move { self.resume(server).await });
        } else {
            let showing_form = self.screen.with_untracked(|screen| match screen {
                Screen::SignIn { server: url, .. } | Screen::Register { server: url } => {
                    *url == server.url
                }
                _ => false,
            });
            if !showing_form {
                self.screen.set(Screen::SignIn {
                    server: server.url,
                    message: None,
                });
            }
        }
    }

    /// The saved server with base URL `url`.
    pub fn server(self, url: &str) -> Option<Server> {
        self.profiles
            .with_untracked(|profiles| profiles.server(url).cloned())
    }

    /// The saved server whose route slug is `slug`.
    pub fn server_by_slug(self, slug: &str) -> Option<Server> {
        self.profiles
            .with_untracked(|profiles| profiles.by_slug(slug).cloned())
    }

    /// The route slug of the saved server with base URL `url`.
    pub fn slug(self, url: &str) -> Option<String> {
        self.profiles.with_untracked(|profiles| profiles.slug(url))
    }

    /// The server of the live session.
    pub fn session_server(self) -> Option<Server> {
        let url = self
            .session
            .with_untracked(|session| session.as_ref().map(|session| session.server.clone()))?;
        self.server(&url)
    }

    /// Changes the saved profiles and saves them (not in a popped-out window).
    pub fn update_profiles(self, change: impl FnOnce(&mut Profiles)) {
        self.profiles.update(change);
        if self.is_popout() {
            return;
        }
        if let Err(error) = self.profiles.with_untracked(platform::save_profiles) {
            self.banner
                .set(Some(format!("Could not save the server list: {error}")));
        }
    }

    /// Changes the saved state and saves it (not in a popped-out window).
    pub fn update_saved(self, change: impl FnOnce(&mut Saved)) {
        if self.is_popout() {
            return;
        }
        self.saved.update(change);
        // Saving is best effort: losing a camera position is harmless, and a full storage is
        // reported by the profiles already.
        let _ = self.saved.with_untracked(platform::save_saved);
    }

    /// Connects the route's server again after a failure.
    pub fn retry(self) {
        let route = self.route.get_untracked();
        self.follow_route(&route);
    }

    /// Opens a server: its world (resuming or signing in as needed).
    pub fn open_server(self, url: &str) {
        let Some(slug) = self.slug(url) else {
            return;
        };
        self.navigate(Route::World {
            server: slug,
            shard: None,
            room: None,
        });
    }

    /// Resumes the saved session of `server` with its stored token. A rejected token returns
    /// to the sign-in form (name filled); other failures keep the token and show an error.
    async fn resume(self, server: Server) {
        self.resuming.set_value(Some(server.url.clone()));
        self.disconnect().await;
        self.screen
            .set(Screen::Starting(format!("Connecting to {}…", server.name)));
        let outcome = self.connect_saved(&server).await;
        self.resuming.set_value(None);
        match outcome {
            Ok(overseer) => self.begin(&server.url, overseer, Start::Resumed),
            Err(screen) => self.screen.set(screen),
        }
        // The route may have moved on meanwhile.
        let route = self.route.get_untracked();
        if route.server() != self.slug(&server.url).as_deref() {
            self.follow_route(&route);
        }
    }

    /// Connects with the stored token of `server`, or says which screen to show instead.
    async fn connect_saved(self, server: &Server) -> Result<Overseer, Screen> {
        let platform = self.platform();
        let sign_in = |message: Option<&str>| Screen::SignIn {
            server: server.url.clone(),
            message: message.map(str::to_owned),
        };
        let token = match platform.secret_get(&token_key(&server.url)).await {
            Ok(Some(token)) => token,
            Ok(None) => {
                self.forget_user(&server.url);
                return Err(sign_in(None));
            }
            Err(error) => {
                self.banner
                    .set(Some(format!("Could not read the stored token: {error}")));
                return Err(sign_in(None));
            }
        };
        let auth = Auth::Token(overseer::core::Token::new(token));
        match session::connect(&platform, &server.url, server.kind.forced(), auth, None).await {
            Ok(overseer) => Ok(overseer),
            Err(Failure::Unauthorized) => {
                if !self.is_popout() {
                    let _ = platform.secret_delete(&token_key(&server.url)).await;
                    self.forget_user(&server.url);
                }
                Err(sign_in(Some(EXPIRED)))
            }
            Err(Failure::ServerPassword) => {
                Err(sign_in(Some("This server needs its server password.")))
            }
            Err(Failure::Other(error)) => Err(Screen::Failed {
                server: server.url.clone(),
                message: format!("Could not connect to {}: {error}", server.name),
            }),
        }
    }

    /// Starts using a connected session on the server with base URL `url`: saves who signed
    /// in and how (`start`), persists every token, follows the connection and notices, and
    /// shows the route's content (or the username form).
    pub fn begin(self, url: &str, overseer: Overseer, start: Start) {
        let epoch = self.epoch.get_value() + 1;
        self.epoch.set_value(epoch);
        if let Some(previous) = self.session.get_untracked() {
            spawn_local(async move { previous.overseer.shutdown().await });
        }
        self.session.set(Some(Session {
            server: url.to_owned(),
            overseer: overseer.clone(),
            epoch,
        }));
        self.status.set(Some(*overseer.status().borrow()));
        self.banner.set(None);
        let user_id = overseer.user_id().map(str::to_owned);
        self.update_profiles(|profiles| {
            if let Some(server) = profiles.server_mut(url) {
                let saved = server.user.take();
                let api_token = match &start {
                    Start::Resumed => saved.as_ref().and_then(|user| user.api_token),
                    Start::Password(_) => Some(false),
                    Start::ApiToken => Some(true),
                };
                if let Start::Password(Some(login)) = start {
                    server.login = Some(login);
                }
                let username = saved.and_then(|user| user.username);
                server.user = user_id.map(|id| SavedUser {
                    id,
                    username,
                    api_token,
                });
            }
        });
        self.follow_tokens(epoch, url, &overseer);
        self.follow_status(epoch, &overseer);
        self.follow_notices(epoch, &overseer);
        // Signing in from the server list opens the server.
        if self.route.get_untracked().server() != self.slug(url).as_deref() {
            self.open_server(url);
        }
        self.settle(epoch, url.to_owned(), overseer);
    }

    /// Shows the content of session `epoch` on `url` once its account is known: the username
    /// form while it has none, else the route's content.
    fn settle(self, epoch: u64, url: String, overseer: Overseer) {
        spawn_local(async move {
            let me = overseer.account().me().await;
            // Ended meanwhile (signed out, expired, replaced): its sign-in form stays.
            if !self.is_live(epoch) {
                return;
            }
            match me {
                Ok(me) => {
                    let screen = Screen::of_account(me.username.as_deref());
                    if me.username.is_some() {
                        self.remember_username(&url, me.username.clone());
                    }
                    self.screen.set(screen);
                }
                Err(error) => {
                    self.screen.set(Screen::Ready);
                    self.banner.set(Some(format!(
                        "Could not load the account: {}",
                        session::describe(&error)
                    )));
                }
            }
        });
    }

    /// Saves the username of the account signed in on `url`.
    pub fn remember_username(self, url: &str, username: Option<String>) {
        self.update_profiles(|profiles| {
            if let Some(user) = profiles
                .server_mut(url)
                .and_then(|server| server.user.as_mut())
            {
                user.username = username;
            }
        });
    }

    /// Whether `epoch` is the live session.
    fn is_live(self, epoch: u64) -> bool {
        self.session
            .with_untracked(|session| session.as_ref().is_some_and(|s| s.epoch == epoch))
    }

    /// Stores every token of the session (sign-in, rotations) in the secret store.
    fn follow_tokens(self, epoch: u64, url: &str, overseer: &Overseer) {
        let mut tokens = overseer.tokens();
        let platform = self.platform();
        let key = token_key(url);
        spawn_local(async move {
            let mut stored: Option<String> = None;
            loop {
                let token = tokens
                    .borrow_and_update()
                    .as_ref()
                    .map(|token| token.as_str().to_owned());
                if !self.is_live(epoch) {
                    break;
                }
                if let Some(token) = token
                    && stored.as_ref() != Some(&token)
                {
                    match platform.secret_set(&key, &token).await {
                        Ok(()) => stored = Some(token),
                        Err(error) => self
                            .banner
                            .set(Some(format!("Could not store the session token: {error}"))),
                    }
                }
                if tokens.changed().await.is_err() {
                    break;
                }
            }
        });
    }

    /// Mirrors the connection state into [`Self::status`].
    fn follow_status(self, epoch: u64, overseer: &Overseer) {
        let mut status = overseer.status();
        spawn_local(async move {
            while status.changed().await.is_ok() {
                if !self.is_live(epoch) {
                    break;
                }
                self.status.set(Some(*status.borrow_and_update()));
            }
        });
    }

    /// Shows server messages; a rejected token ends the session.
    fn follow_notices(self, epoch: u64, overseer: &Overseer) {
        let mut notices = overseer.notices();
        spawn_local(async move {
            loop {
                let notice = match notices.recv().await {
                    Ok(notice) => notice,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                };
                if !self.is_live(epoch) {
                    break;
                }
                match notice {
                    Notice::AuthFailed => {
                        self.end(!self.is_popout(), Some(EXPIRED.to_owned())).await;
                        break;
                    }
                    Notice::ServerMessage(message) => self.banner.set(Some(message)),
                    Notice::ServerDown => self
                        .banner
                        .set(Some("The server is down for maintenance.".to_owned())),
                    // Refused live rooms show in their room control.
                    Notice::ChannelError {
                        channel: overseer::core::socket::Channel::Room { .. },
                        ..
                    }
                    | Notice::Undecodable(_)
                    | Notice::Lagged { .. } => {}
                    Notice::ChannelError { channel, message } => {
                        self.banner.set(Some(format!("{channel}: {message}")));
                    }
                }
            }
        });
    }

    /// Replaces the live session with a new sign-in as `login` with `password` on the same
    /// server: a password change revokes every session of the account.
    ///
    /// # Errors
    ///
    /// Returns why signing in failed; the sign-in form is shown then.
    pub async fn renew(self, login: String, password: String) -> Result<(), String> {
        let Some(session) = self.session.get_untracked() else {
            return Err("Not signed in".to_owned());
        };
        self.disconnect().await;
        let auth = Auth::Credentials {
            email: login,
            password,
        };
        let server_kind = self
            .server(&session.server)
            .and_then(|server| server.kind.forced());
        match session::connect(&self.platform(), &session.server, server_kind, auth, None).await {
            Ok(overseer) => {
                self.begin(&session.server, overseer, Start::Password(None));
                Ok(())
            }
            Err(failure) => {
                let message = match failure {
                    Failure::Unauthorized => "the new password was rejected".to_owned(),
                    Failure::ServerPassword => "incorrect server password".to_owned(),
                    Failure::Other(message) => message,
                };
                self.screen.set(Screen::SignIn {
                    server: session.server,
                    message: Some(format!("Signing in again failed: {message}")),
                });
                Err(message)
            }
        }
    }

    /// Closes the live session but keeps it saved (it resumes from its token later).
    pub async fn disconnect(self) {
        let session = self.session.get_untracked();
        if let Some(session) = session {
            self.epoch.set_value(self.epoch.get_value() + 1);
            self.session.set(None);
            self.status.set(None);
            session.overseer.shutdown().await;
        }
    }

    /// Signs out of the live session: forgets its token and shows the sign-in form.
    pub async fn sign_out(self) {
        self.end(true, None).await;
    }

    /// Ends the live session (forgetting its token when `forget`) and shows its sign-in form
    /// with `message`.
    async fn end(self, forget: bool, message: Option<String>) {
        let Some(session) = self.session.get_untracked() else {
            return;
        };
        self.disconnect().await;
        if forget {
            if let Err(error) = self
                .platform()
                .secret_delete(&token_key(&session.server))
                .await
            {
                self.banner
                    .set(Some(format!("Could not delete the stored token: {error}")));
            }
            self.forget_user(&session.server);
        }
        self.screen.set(Screen::SignIn {
            server: session.server,
            message,
        });
    }

    /// Marks the server with base URL `url` signed out.
    fn forget_user(self, url: &str) {
        self.update_profiles(|profiles| {
            if let Some(server) = profiles.server_mut(url) {
                server.user = None;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::Screen;

    const LIVE: &str = "http://127.0.0.1:21025/";
    const OTHER: &str = "https://screeps.com/";

    #[test]
    fn only_another_servers_forms_and_failures_belong_to_another_server() {
        let other = [
            Screen::SignIn {
                server: OTHER.to_owned(),
                message: None,
            },
            Screen::Register {
                server: OTHER.to_owned(),
            },
            Screen::Failed {
                server: OTHER.to_owned(),
                message: "down".to_owned(),
            },
        ];
        for screen in other {
            assert!(screen.is_of_other_server(LIVE), "{screen:?}");
        }
        let own = [
            Screen::SignIn {
                server: LIVE.to_owned(),
                message: None,
            },
            Screen::Register {
                server: LIVE.to_owned(),
            },
            Screen::Failed {
                server: LIVE.to_owned(),
                message: "down".to_owned(),
            },
            Screen::Starting("Starting…".to_owned()),
            Screen::SetUsername,
            Screen::Ready,
        ];
        for screen in own {
            assert!(!screen.is_of_other_server(LIVE), "{screen:?}");
        }
    }

    #[test]
    fn an_account_without_a_username_gets_the_username_form() {
        assert_eq!(Screen::of_account(None), Screen::SetUsername);
        assert_eq!(Screen::of_account(Some("player")), Screen::Ready);
    }
}
