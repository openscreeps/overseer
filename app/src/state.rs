//! The app's state and the session lifecycle: resume on start, sign in, token rotation,
//! expiry, disconnect and sign-out.

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::{Auth, ConnectionStatus, Notice, Overseer};
use tokio::sync::broadcast::error::RecvError;

use crate::platform::{self, Platform};
use crate::profiles::{Profiles, SavedUser, Server, token_key};
use crate::session::{self, Failure};

/// Shown when a stored token is rejected.
const EXPIRED: &str = "Your session has expired. Sign in again.";

/// The screen on display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Connecting or resuming a session, with what is happening.
    Starting(String),
    /// The server list.
    Servers,
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
    /// The signed-in account has no username yet.
    SetUsername,
    /// The signed-in account.
    Profile,
    /// The room viewer.
    Room,
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

/// What the room screen reports to the status bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomStats {
    /// `shard/room` or `room` on display.
    pub room: Option<String>,
    /// Game tick of the latest update.
    pub game_time: Option<u64>,
    /// CPU time of the last frame, in milliseconds.
    pub frame_ms: Option<f32>,
    /// Objects drawn.
    pub objects: usize,
    /// Graphics backend (`WebGPU`, `WebGL2`).
    pub backend: Option<&'static str>,
}

/// Everything the components share; `Copy`, provided as context.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Where the app runs.
    pub platform: StoredValue<Platform>,
    /// Saved servers.
    pub profiles: RwSignal<Profiles>,
    /// The screen on display.
    pub screen: RwSignal<Screen>,
    /// The live session.
    pub session: RwSignal<Option<Session>, LocalStorage>,
    /// Connection state of the live session.
    pub status: RwSignal<Option<ConnectionStatus>>,
    /// A message for the user outside any form (server messages, failures of background work).
    pub banner: RwSignal<Option<String>>,
    /// Room screen figures for the status bar.
    pub room_stats: RwSignal<RoomStats>,
    /// Counter of sessions begun.
    epoch: StoredValue<u64>,
}

impl AppState {
    /// A fresh state on the start screen.
    pub fn new(platform: Platform) -> Self {
        Self {
            platform: StoredValue::new(platform),
            profiles: RwSignal::new(platform::load_profiles()),
            screen: RwSignal::new(Screen::Starting("Starting…".to_owned())),
            session: RwSignal::new_local(None),
            status: RwSignal::new(None),
            banner: RwSignal::new(None),
            room_stats: RwSignal::new(RoomStats::default()),
            epoch: StoredValue::new(0),
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

    /// Resumes the last active session or shows the server list.
    pub fn start(self) {
        spawn_local(async move {
            let active = self.profiles.with_untracked(|profiles| {
                profiles
                    .active
                    .as_deref()
                    .and_then(|url| profiles.server(url))
                    .filter(|server| server.user.is_some())
                    .cloned()
            });
            match active {
                Some(server) => self.resume(server).await,
                None => self.screen.set(Screen::Servers),
            }
        });
    }

    /// The saved server with base URL `url`.
    pub fn server(self, url: &str) -> Option<Server> {
        self.profiles
            .with_untracked(|profiles| profiles.server(url).cloned())
    }

    /// The server of the live session.
    pub fn session_server(self) -> Option<Server> {
        let url = self
            .session
            .with_untracked(|session| session.as_ref().map(|session| session.server.clone()))?;
        self.server(&url)
    }

    /// Changes the saved profiles and saves them.
    pub fn update_profiles(self, change: impl FnOnce(&mut Profiles)) {
        self.profiles.update(change);
        if let Err(error) = self.profiles.with_untracked(platform::save_profiles) {
            self.banner
                .set(Some(format!("Could not save the server list: {error}")));
        }
    }

    /// Opens a server from the list: its live session, its saved session, or its sign-in.
    pub fn open_server(self, url: &str) {
        let live = self.session.with_untracked(|session| {
            session
                .as_ref()
                .is_some_and(|session| session.server == url)
        });
        if live {
            self.screen.set(Screen::Profile);
            return;
        }
        let Some(server) = self.server(url) else {
            return;
        };
        if server.user.is_some() {
            spawn_local(async move { self.resume(server).await });
        } else {
            self.screen.set(Screen::SignIn {
                server: server.url,
                message: None,
            });
        }
    }

    /// Resumes the saved session of `server` with its stored token. A rejected token returns
    /// to the sign-in form (name filled); other failures keep the token and show the list.
    async fn resume(self, server: Server) {
        self.disconnect().await;
        self.screen
            .set(Screen::Starting(format!("Connecting to {}…", server.name)));
        let platform = self.platform();
        let token = match platform.secret_get(&token_key(&server.url)).await {
            Ok(Some(token)) => token,
            Ok(None) => {
                self.forget_user(&server.url);
                self.screen.set(Screen::SignIn {
                    server: server.url,
                    message: None,
                });
                return;
            }
            Err(error) => {
                self.screen.set(Screen::Servers);
                self.banner
                    .set(Some(format!("Could not read the stored token: {error}")));
                return;
            }
        };
        let auth = Auth::Token(overseer::core::Token::new(token));
        match session::connect(&platform, &server.url, auth, None).await {
            Ok(overseer) => self.begin(&server.url, overseer, None),
            Err(Failure::Unauthorized) => {
                let _ = platform.secret_delete(&token_key(&server.url)).await;
                self.forget_user(&server.url);
                self.screen.set(Screen::SignIn {
                    server: server.url,
                    message: Some(EXPIRED.to_owned()),
                });
            }
            Err(Failure::ServerPassword) => self.screen.set(Screen::SignIn {
                server: server.url,
                message: Some("This server needs its server password.".to_owned()),
            }),
            Err(Failure::Other(error)) => {
                self.screen.set(Screen::Servers);
                self.banner.set(Some(format!(
                    "Could not connect to {}: {error}",
                    server.name
                )));
            }
        }
    }

    /// Starts using a connected session on the server with base URL `url`: saves who signed
    /// in (and the `login` name used), persists every token, follows the connection and
    /// notices, and shows the profile (or the username form).
    pub fn begin(self, url: &str, overseer: Overseer, login: Option<String>) {
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
            profiles.active = Some(url.to_owned());
            if let Some(server) = profiles.server_mut(url) {
                if login.is_some() {
                    server.login = login;
                }
                let username = server.user.take().and_then(|user| user.username);
                server.user = user_id.map(|id| SavedUser { id, username });
            }
        });
        self.follow_tokens(epoch, url, &overseer);
        self.follow_status(epoch, &overseer);
        self.follow_notices(epoch, &overseer);
        let url = url.to_owned();
        spawn_local(async move {
            match overseer.account().me().await {
                Ok(me) if me.username.is_none() => self.screen.set(Screen::SetUsername),
                Ok(me) => {
                    self.remember_username(&url, me.username.clone());
                    self.screen.set(Screen::Profile);
                }
                Err(error) => {
                    self.screen.set(Screen::Profile);
                    self.banner.set(Some(format!(
                        "Could not load the profile: {}",
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
                        self.end(true, Some(EXPIRED.to_owned())).await;
                        break;
                    }
                    Notice::ServerMessage(message) => self.banner.set(Some(message)),
                    Notice::ServerDown => self
                        .banner
                        .set(Some("The server is down for maintenance.".to_owned())),
                    Notice::ChannelError { channel, message } => {
                        self.banner.set(Some(format!("{channel}: {message}")));
                    }
                    Notice::Undecodable(_) | Notice::Lagged { .. } => {}
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
        match session::connect(&self.platform(), &session.server, auth, None).await {
            Ok(overseer) => {
                self.begin(&session.server, overseer, None);
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
            if profiles.active.as_deref() == Some(url) {
                profiles.active = None;
            }
        });
    }
}
