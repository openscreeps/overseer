//! The components: the shell (top bar, banner, status bar) around the page of the route.
//!
//! The world canvas fills the window under the top bar; the server list, sign-in, register
//! and profile pages show over the same backdrop. A popped-out window shows the room control
//! alone.

mod drawer;
mod form;
mod profile;
mod rate_limits;
mod register;
mod room_panel;
mod servers;
mod sign_in;
mod surface;
mod topbar;
mod window;
mod world;

use futures_util::future::abortable;
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::ConnectionStatus;
use overseer::map::RoomCoord;
use overseer_view::Lod;

use crate::platform::Platform;
use crate::route::{Route, Shard};
use crate::state::{AppState, Screen};
use crate::ui::world::MapState;

/// What the main area shows; changes of the room alone keep the page.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Page {
    /// The server list.
    Servers,
    /// The route names no saved server.
    UnknownServer(String),
    /// The session is not ready (connecting, sign-in, register, username, failure).
    Session(Screen),
    /// The route's shard is being resolved.
    Resolving,
    /// The route names a shard the server does not have.
    UnknownShard(String),
    /// The signed-in account.
    Profile,
    /// The world of a shard.
    World {
        url: String,
        shard: Shard,
        epoch: u64,
    },
    /// A popped-out room.
    Window {
        url: String,
        shard: Shard,
        room: RoomCoord,
        epoch: u64,
    },
}

/// The app.
#[component]
pub fn App(
    /// Where the app runs.
    platform: Platform,
) -> impl IntoView {
    let state = AppState::new(platform);
    provide_context(state);
    let map = MapState::new();
    provide_context(map);
    let limits = rate_limits::RateLimitsUi::new();
    provide_context(limits);
    limits.listen(map);
    state.start();
    follow_shards(state, map);
    resolve_shard(state);
    let page = Memo::new(move |_| page(state));
    provide_context(page);
    follow_title(state, map);
    let content = move || {
        match page.get() {
        Page::Servers => view! { <servers::Servers /> }.into_any(),
        Page::UnknownServer(slug) => view! {
            <section class="panel narrow" role="alert">
                <h1>"Unknown server"</h1>
                <p>{format!("No saved server is called “{slug}” here. Add it, then open the link again.")}</p>
                <button type="button" class="primary" on:click=move |_| state.navigate(Route::Servers)>
                    "Servers"
                </button>
            </section>
        }
        .into_any(),
        Page::UnknownShard(message) => view! {
            <section class="panel narrow" role="alert">
                <h1>"Unknown shard"</h1>
                <p>{message}</p>
                <button type="button" class="primary" on:click=move |_| open_default_world(state)>
                    "Open the default shard"
                </button>
            </section>
        }
        .into_any(),
        Page::Session(screen) => session_page(state, screen),
        Page::Resolving => starting("Opening the world…".to_owned()),
        Page::Profile => view! { <profile::Profile /> }.into_any(),
        Page::World { url, shard, .. } => {
            let (Some(server), Some(session)) = (state.server(&url), state.session.get_untracked())
            else {
                return starting("Opening the world…".to_owned());
            };
            let room = state.route.with_untracked(|route| match route {
                Route::World { room, .. } => *room,
                _ => None,
            });
            view! { <world::WorldScreen server shard room overseer=session.overseer /> }.into_any()
        }
        Page::Window { url, shard, room, .. } => {
            let (Some(server), Some(session)) = (state.server(&url), state.session.get_untracked())
            else {
                return starting("Opening the room…".to_owned());
            };
            view! { <window::WindowScreen server shard room overseer=session.overseer /> }
                .into_any()
        }
    }
    };
    let world_page = move || matches!(page.get(), Page::World { .. });
    if state.is_popout() {
        return view! {
            <Banner />
            <main id="main" class="popout">{content}</main>
            <StatusBar />
        }
        .into_any();
    }
    view! {
        <topbar::TopBar />
        <Banner />
        <main id="main" class:map-page=world_page>{content}</main>
        <rate_limits::RateLimitsPanel />
        <StatusBar />
    }
    .into_any()
}

/// The page of the route, given the session.
fn page(state: AppState) -> Page {
    let route = state.route.get();
    let Some(slug) = route.server().map(str::to_owned) else {
        return Page::Servers;
    };
    state.profiles.track();
    let Some(server) = state.server_by_slug(&slug) else {
        return Page::UnknownServer(slug);
    };
    let screen = state.screen.get();
    let session = state.session.with(|session| {
        session
            .as_ref()
            .filter(|session| session.server == server.url)
            .map(|session| (session.epoch(), session.overseer.world().shards().to_vec()))
    });
    let Some((epoch, shards)) = session else {
        return Page::Session(match screen {
            Screen::Ready => Screen::Starting(format!("Connecting to {}…", server.name)),
            screen => screen,
        });
    };
    if screen != Screen::Ready {
        return Page::Session(screen);
    }
    let known = |shard: &Shard| match shard {
        Shard::Single => shards.is_empty(),
        Shard::Named(name) => shards.contains(name),
    };
    match route {
        Route::Servers => Page::Servers,
        Route::Profile { .. } => Page::Profile,
        Route::World {
            shard: Some(shard), ..
        } if known(&shard) => Page::World {
            url: server.url,
            shard,
            epoch,
        },
        Route::Window { shard, room, .. } if known(&shard) => Page::Window {
            url: server.url,
            shard,
            room,
            epoch,
        },
        // No shard, or `-` on a sharded server: [`resolve_shard`] picks one.
        Route::World {
            shard: None | Some(Shard::Single),
            ..
        } => Page::Resolving,
        Route::World {
            shard: Some(shard), ..
        }
        | Route::Window { shard, .. } => Page::UnknownShard(match shard.name() {
            Some(name) if shards.is_empty() => {
                format!("{} has a single world, no shard “{name}”.", server.name)
            }
            _ => format!("{} has no shard “{shard}”.", server.name),
        }),
    }
}

/// Names the window after where the user is (the history and the task bar show it).
fn follow_title(state: AppState, map: MapState) {
    if state.is_popout() {
        return;
    }
    Effect::new(move |_| {
        let route = state.route.get();
        state.profiles.track();
        let server = route
            .server()
            .and_then(|slug| state.server_by_slug(slug))
            .map(|server| server.name);
        let title = match (&route, server) {
            (Route::Servers, _) | (_, None) => "Overseer".to_owned(),
            (Route::Profile { .. }, Some(server)) => format!("Account · {server} — Overseer"),
            (_, Some(server)) => {
                let shard = map
                    .shard
                    .get()
                    .and_then(|shard| shard.name().map(str::to_owned));
                let place = [
                    map.room.get().map(|room| room.to_string()),
                    shard,
                    Some(server),
                ];
                format!(
                    "{} — Overseer",
                    place.into_iter().flatten().collect::<Vec<_>>().join(" · ")
                )
            }
        };
        if let Some(document) = web_sys::window().and_then(|window| window.document()) {
            document.set_title(&title);
        }
    });
}

/// Keeps [`MapState::shards`] at the live session's shards.
fn follow_shards(state: AppState, map: MapState) {
    Effect::new(move |_| {
        let shards = state.session.with(|session| {
            session
                .as_ref()
                .map(|session| session.overseer.world().shards().to_vec())
                .unwrap_or_default()
        });
        map.shards.set(shards);
    });
}

/// Replaces a world route without a (valid single-world) shard by the server's last shard,
/// else its default one, and that shard's last room.
fn resolve_shard(state: AppState) {
    Effect::new(move |_| {
        let route = state.route.get();
        let ready = state.screen.get() == Screen::Ready;
        let Route::World {
            server: slug,
            shard,
            ..
        } = route
        else {
            return;
        };
        let pending = match &shard {
            None => true,
            Some(Shard::Single) => state.session.with(|session| {
                session
                    .as_ref()
                    .is_some_and(|session| session.overseer.world().is_sharded())
            }),
            Some(Shard::Named(_)) => false,
        };
        if !ready || !pending {
            return;
        }
        let Some(server) = state.server_by_slug(&slug) else {
            return;
        };
        let Some((shards, default)) = state.session.with_untracked(|session| {
            session
                .as_ref()
                .filter(|session| session.server == server.url)
                .map(|session| {
                    let world = session.overseer.world();
                    (world.shards().to_vec(), world.shard().map(str::to_owned))
                })
        }) else {
            return;
        };
        let place = state
            .saved
            .with_untracked(|saved| saved.places.get(&server.url).cloned())
            .unwrap_or_default();
        let shard = place
            .shard
            .as_deref()
            .filter(|last| shards.iter().any(|shard| shard == last))
            .map(str::to_owned)
            .or(default);
        let shard = Shard::of(shard.as_deref());
        let room = place.rooms.get(&shard.to_string()).copied();
        state.replace_route(Route::World {
            server: slug,
            shard: Some(shard),
            room,
        });
    });
}

/// Opens the route's server on its default shard.
fn open_default_world(state: AppState) {
    if let Some(slug) = state
        .route
        .with_untracked(|route| route.server().map(str::to_owned))
    {
        state.navigate(Route::World {
            server: slug,
            shard: None,
            room: None,
        });
    }
}

/// The page of a session that is not ready.
fn session_page(state: AppState, screen: Screen) -> AnyView {
    match screen {
        Screen::Starting(text) => starting(text),
        Screen::SignIn { .. } | Screen::Register { .. } if state.is_popout() => view! {
            <section class="panel narrow" role="alert">
                <h1>"Signed out"</h1>
                <p>"Sign in in Overseer's main window, then pop the room out again."</p>
            </section>
        }
        .into_any(),
        Screen::SignIn { server, message } => {
            view! { <sign_in::SignIn server message /> }.into_any()
        }
        Screen::Register { server } => view! { <register::Register server /> }.into_any(),
        Screen::SetUsername => view! { <register::SetUsername /> }.into_any(),
        Screen::Failed { message, .. } => view! {
            <section class="panel narrow" role="alert">
                <h1>"Cannot connect"</h1>
                <p>{message}</p>
                <div class="actions">
                    <button type="button" class="primary" on:click=move |_| state.retry()>
                        "Try again"
                    </button>
                    <button type="button" on:click=move |_| state.navigate(Route::Servers)>
                        "Servers"
                    </button>
                </div>
            </section>
        }
        .into_any(),
        Screen::Ready => starting("Opening…".to_owned()),
    }
}

/// A centred progress message.
fn starting(text: String) -> AnyView {
    view! {
        <section class="panel starting" role="status">
            <span class="spinner" aria-hidden="true"></span>
            <p>{text}</p>
        </section>
    }
    .into_any()
}

/// Messages outside any form.
#[component]
fn Banner() -> impl IntoView {
    let state = AppState::expect();
    move || {
        state.banner.get().map(|message| {
            view! {
                <div class="banner" role="alert">
                    <span>{message}</span>
                    <button
                        type="button"
                        aria-label="Dismiss message"
                        on:click=move |_| state.banner.set(None)
                    >
                        "×"
                    </button>
                </div>
            }
        })
    }
}

/// Connection, tick, level of detail, the world's loading progress (while loading; with the
/// rate limit's pause and `Reset limit`), frame time and graphics backend.
#[component]
fn StatusBar() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let limits = rate_limits::RateLimitsUi::expect();
    let connection = move || match state.status.get() {
        None => "Not connected".to_owned(),
        Some(ConnectionStatus::Connecting) => "Connecting…".to_owned(),
        Some(ConnectionStatus::Connected) => "Connected".to_owned(),
        Some(ConnectionStatus::Reconnecting { attempt, delay }) => {
            format!("Reconnecting (attempt {attempt}, in {} s)", delay.as_secs())
        }
        Some(ConnectionStatus::Closed) => "Disconnected".to_owned(),
    };
    let tick = move || {
        map.game_time
            .get()
            .map_or_else(|| "tick —".to_owned(), |time| format!("tick {time}"))
    };
    let lod = move || match map.lod.get() {
        None => "—",
        Some(Lod::Overview) => "Overview",
        Some(Lod::Terrain) => "Terrain",
        Some(Lod::Minimap) => "Minimap",
        Some(Lod::Detail) => "Live rooms",
    };
    let progress = move || {
        map.progress
            .get()
            .as_ref()
            .and_then(|progress| {
                // The clock only matters while paused: the time left updates every second.
                let now = if progress.stats_paused_until.is_some() {
                    limits.now.get()
                } else {
                    0
                };
                crate::feed::progress_label(progress, now)
            })
            .map(|label| {
                view! {
                    <span class="status-item" role="status" title="Loading the world">
                        {label}
                    </span>
                    <rate_limits::StatusReset />
                }
            })
    };
    let frame = move || {
        map.frame_ms
            .get()
            .map_or_else(|| "frame —".to_owned(), |ms| format!("frame {ms:.2} ms"))
    };
    view! {
        <footer class="status-bar" aria-label="Status">
            <span class="status-item">
                <span class=move || topbar::status_class(state.status.get()) aria-hidden="true"></span>
                {connection}
            </span>
            <span class="status-item">{tick}</span>
            <span class="status-item" title="Level of detail">{lod}</span>
            {progress}
            <span class="status-item">{frame}</span>
            <span class="status-item">{move || map.backend.get().unwrap_or("—")}</span>
            <span class="status-item platform">
                {move || {
                    if state.platform.with_value(Platform::is_tauri) { "Desktop" } else { "Web" }
                }}
            </span>
        </footer>
    }
}

/// Runs `future` on the browser's event loop until it ends or the calling component goes
/// away.
pub fn spawn_scoped(future: impl Future<Output = ()> + 'static) {
    let (future, handle) = abortable(future);
    on_cleanup(move || handle.abort());
    spawn_local(async move {
        // `Err` only says it was aborted.
        let _ = future.await;
    });
}

/// Opens `url` outside the app; failures go to the banner.
pub fn open_external(url: &'static str) {
    let state = AppState::expect();
    let platform = state.platform();
    spawn_local(async move {
        if let Err(error) = platform.open_external(url).await {
            state
                .banner
                .set(Some(format!("Could not open {url}: {error}")));
        }
    });
}
