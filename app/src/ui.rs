//! The components: the shell (top bar, banner, status bar, toasts) around the page of the
//! route.
//!
//! The world canvas fills the window under the top bar; the server list, sign-in, register
//! and profile pages show over the same backdrop. A popped-out window shows a room control or
//! the console alone. The shell is as tall as the visible window (`h-dvh` on `<body>`: a
//! phone's browser bars and on-screen keyboard shrink it) and keeps the safe-area insets
//! (`app/tailwind.css`). The layout follows the window's width (phone, tablet, desktop) and
//! the pointer (a finger gets larger controls), mostly in CSS; [`media`] gives the layout as
//! a signal where it changes what is built.

mod account;
mod console;
mod drawer;
mod form;
mod hover;
mod icon;
mod inspector;
mod media;
mod memory;
mod objects;
mod player;
mod profile;
mod rate_limits;
mod register;
mod room_panel;
mod servers;
mod sign_in;
mod status_bar;
mod style;
mod surface;
mod tick;
mod toast;
mod topbar;
mod usage;
mod window;
mod world;

use futures_util::future::abortable;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::platform::Platform;
use crate::route::{Route, Shard, WindowView};
use crate::state::{self, AppState, Screen};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::status_bar::StatusBar;
use crate::ui::style::concatcp;
use crate::ui::toast::{ToastRegion, Toasts};
use crate::ui::world::MapState;

/// The main area of a page shown over the backdrop.
const MAIN: &str = "flex-1 min-h-0 flex flex-col gap-4 px-5 py-6 max-md:px-3 max-md:py-4 \
    overflow-auto bg-[radial-gradient(circle_at_30%_20%,var(--color-glow)_0,var(--color-bg)_60%)]";
/// The main area of the world or a popped-out window: the page fills it.
const MAIN_FULL: &str = "flex-1 min-h-0 flex flex-col gap-4 p-0 overflow-hidden bg-bg";
/// A whole-page state in a narrow panel.
const STATE_PANEL: &str = concatcp!(style::PANEL_NARROW, " ", style::STATE_CARD);
/// A progress message: a [`STATE_PANEL`] lower on the page.
const STARTING: &str = concatcp!(STATE_PANEL, " mt-16");
/// The buttons of a [`STATE_PANEL`], centred.
const STATE_ACTIONS: &str = concatcp!(style::ACTIONS, " justify-center");
/// The banner's dismiss button.
const BANNER_DISMISS: &str = concatcp!(style::ICON_BUTTON_GHOST_SM, " relative ", style::TIP_LEFT);

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
    /// The public profile of the player with this name.
    User(String),
    /// The world of a shard.
    World {
        url: String,
        shard: Shard,
        epoch: u64,
    },
    /// A popped-out window.
    Window {
        url: String,
        shard: Shard,
        view: WindowView,
        epoch: u64,
    },
}

/// The app: the `Starting…` screen until the saved servers are loaded
/// ([`state::load_profiles`]; in the shell, from its profiles file), then [`Loaded`].
#[component]
pub fn App(
    /// Where the app runs.
    platform: Platform,
) -> impl IntoView {
    let profiles = RwSignal::new(None);
    let loading = platform.clone();
    spawn_local(async move { profiles.set(Some(state::load_profiles(&loading).await)) });
    move || match profiles.get() {
        None => starting("Starting…".to_owned()),
        Some(profiles) => view! { <Loaded platform=platform.clone() profiles /> }.into_any(),
    }
}

/// The app with its saved servers.
#[component]
fn Loaded(
    /// Where the app runs.
    platform: Platform,
    /// The saved profiles at the start, or why they could not be loaded.
    profiles: Result<overseer_storage::profiles::Profiles, String>,
) -> impl IntoView {
    let state = AppState::new(platform, profiles);
    provide_context(state);
    let map = MapState::new();
    provide_context(map);
    provide_context(hover::MapRooms::new());
    let limits = rate_limits::RateLimitsUi::new();
    provide_context(limits);
    limits.listen(map);
    let usage = usage::UsageUi::new(limits.now.into());
    provide_context(usage);
    let tick = tick::TickUi::default();
    provide_context(tick);
    provide_context(Toasts::new());
    state.start();
    limits.follow(state, map);
    usage.follow(state);
    tick.follow(state, map);
    follow_shards(state, map);
    resolve_shard(state);
    let page = Memo::new(move |_| page(state));
    provide_context(page);
    follow_title(state, map);
    let content = move || {
        match page.get() {
        Page::Servers => view! { <servers::Servers /> }.into_any(),
        Page::UnknownServer(slug) => view! {
            <section class=STATE_PANEL role="alert">
                <span class=style::STATE_ICON_WARN>
                    <Icon kind=IconKind::Alert class=style::STATE_ICON_SVG />
                </span>
                <h1 class=style::STATE_TITLE>"Unknown server"</h1>
                <p class=style::STATE_TEXT>
                    {format!("No saved server is called “{slug}” here. Add it, then open the link again.")}
                </p>
                <div class=STATE_ACTIONS>
                    <button
                        type="button"
                        class=style::BUTTON_PRIMARY
                        on:click=move |_| state.navigate(Route::Servers)
                    >
                        "Servers"
                    </button>
                </div>
            </section>
        }
        .into_any(),
        Page::UnknownShard(message) => view! {
            <section class=STATE_PANEL role="alert">
                <span class=style::STATE_ICON_WARN>
                    <Icon kind=IconKind::Alert class=style::STATE_ICON_SVG />
                </span>
                <h1 class=style::STATE_TITLE>"Unknown shard"</h1>
                <p class=style::STATE_TEXT>{message}</p>
                <div class=STATE_ACTIONS>
                    <button
                        type="button"
                        class=style::BUTTON_PRIMARY
                        on:click=move |_| open_default_world(state)
                    >
                        "Open the default shard"
                    </button>
                </div>
            </section>
        }
        .into_any(),
        Page::Session(screen) => session_page(state, screen),
        Page::Resolving => starting("Opening the world…".to_owned()),
        Page::Profile => view! { <profile::Profile /> }.into_any(),
        Page::User(name) => view! { <player::PlayerPage name /> }.into_any(),
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
        Page::Window { url, shard, view, .. } => {
            let (Some(server), Some(session)) = (state.server(&url), state.session.get_untracked())
            else {
                return starting("Opening the window…".to_owned());
            };
            view! { <window::WindowScreen server shard view overseer=session.overseer /> }
                .into_any()
        }
    }
    };
    let world_page = move || matches!(page.get(), Page::World { .. });
    if state.is_popout() {
        return view! {
            <Banner />
            <main id="main" class=MAIN_FULL>{content}</main>
            <usage::UsagePanel />
            <ToastRegion />
            <StatusBar />
        }
        .into_any();
    }
    view! {
        <topbar::TopBar />
        <Banner />
        <main id="main" class=move || if world_page() { MAIN_FULL } else { MAIN }>{content}</main>
        <rate_limits::RateLimitsPanel />
        <usage::UsagePanel />
        <rate_limits::LiftPrompt />
        <ToastRegion />
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
        Route::User { name, .. } => Page::User(name),
        Route::World {
            shard: Some(shard), ..
        } if known(&shard) => Page::World {
            url: server.url,
            shard,
            epoch,
        },
        Route::Window { shard, view, .. } if known(&shard) => Page::Window {
            url: server.url,
            shard,
            view,
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
            (Route::User { name, .. }, Some(_)) => format!("{name} — Overseer"),
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
            <section class=STATE_PANEL role="alert">
                <span class=style::STATE_ICON>
                    <Icon kind=IconKind::User class=style::STATE_ICON_SVG />
                </span>
                <h1 class=style::STATE_TITLE>"Signed out"</h1>
                <p class=style::STATE_TEXT>
                    "Sign in in Overseer's main window, then pop the window out again."
                </p>
            </section>
        }
        .into_any(),
        Screen::SignIn { server, message } => {
            view! { <sign_in::SignIn server message /> }.into_any()
        }
        Screen::Register { server } => view! { <register::Register server /> }.into_any(),
        Screen::SetUsername => view! { <register::SetUsername /> }.into_any(),
        Screen::Failed { message, .. } => view! {
            <section class=STATE_PANEL role="alert">
                <span class=style::STATE_ICON_ERROR>
                    <Icon kind=IconKind::Alert class=style::STATE_ICON_SVG />
                </span>
                <h1 class=style::STATE_TITLE>"Cannot connect"</h1>
                <p class=style::STATE_TEXT>{message}</p>
                <div class=STATE_ACTIONS>
                    <button type="button" class=style::BUTTON_PRIMARY on:click=move |_| state.retry()>
                        <Icon kind=IconKind::Refresh />
                        "Try again"
                    </button>
                    <button
                        type="button"
                        class=style::BUTTON
                        on:click=move |_| state.navigate(Route::Servers)
                    >
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
        <section class=STARTING role="status">
            <span class=style::SPINNER aria-hidden="true"></span>
            <p class=style::STATE_TEXT>{text}</p>
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
                <div
                    class="flex flex-none items-center gap-3 py-1 pr-2 pl-4 text-fg bg-warn-soft \
                        border-b border-warn/45"
                    role="alert"
                >
                    <Icon kind=IconKind::Alert class=concatcp!(style::ICON, " text-warn") />
                    <span class="flex-1 min-w-0">{message}</span>
                    <button
                        type="button"
                        class=BANNER_DISMISS
                        aria-label="Dismiss message"
                        data-tip="Dismiss"
                        on:click=move |_| state.banner.set(None)
                    >
                        <Icon kind=IconKind::Close />
                    </button>
                </div>
            }
        })
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

/// A number no other element id of the page has: components that may be shown more than once
/// at a time give their elements ids of their own with it.
pub fn next_id() -> u64 {
    thread_local! {
        static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }
    NEXT.with(|next| {
        let id = next.get();
        next.set(id + 1);
        id
    })
}

/// `HH:MM:SS` of `time` in the local time zone.
#[expect(clippy::cast_precision_loss, reason = "times are far below 2^53 ms")]
pub fn clock(time: overseer::net::rt::SystemTime) -> String {
    let millis = time
        .duration_since(overseer::net::rt::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(millis as f64));
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
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
