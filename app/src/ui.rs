//! The components: the frame (header, banner, status bar) and one screen at a time.

mod form;
mod profile;
mod register;
mod room;
mod servers;
mod sign_in;

use futures_util::future::abortable;
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::ConnectionStatus;

use crate::platform::Platform;
use crate::state::{AppState, Screen};

/// The app.
#[component]
pub fn App(
    /// Where the app runs.
    platform: Platform,
) -> impl IntoView {
    let state = AppState::new(platform);
    provide_context(state);
    state.start();
    let screen = move || match state.screen.get() {
        Screen::Starting(text) => view! {
            <section class="panel starting" role="status">
                <p>{text}</p>
            </section>
        }
        .into_any(),
        Screen::Servers => view! { <servers::Servers /> }.into_any(),
        Screen::SignIn { server, message } => {
            view! { <sign_in::SignIn server message /> }.into_any()
        }
        Screen::Register { server } => view! { <register::Register server /> }.into_any(),
        Screen::SetUsername => view! { <register::SetUsername /> }.into_any(),
        Screen::Profile => view! { <profile::Profile /> }.into_any(),
        Screen::Room => view! { <room::Room /> }.into_any(),
    };
    view! {
        <Header />
        <Banner />
        <main id="main">{screen}</main>
        <StatusBar />
    }
}

/// App name, the server in use and the navigation.
#[component]
fn Header() -> impl IntoView {
    let state = AppState::expect();
    let signed_in = move || state.session.with(Option::is_some);
    let current = move |target: Screen| move || (state.screen.get() == target).then_some("page");
    let server_name = move || {
        state.session.track();
        state.profiles.track();
        state
            .session_server()
            .map(|server| {
                let user = server
                    .user
                    .and_then(|user| user.username)
                    .map(|name| format!(" · {name}"))
                    .unwrap_or_default();
                format!("{}{user}", server.name)
            })
            .unwrap_or_default()
    };
    view! {
        <header class="app-header">
            <span class="brand">
                <img src="icon.svg" alt="" width="24" height="24" />
                "Overseer"
            </span>
            <span class="server-name">{server_name}</span>
            <nav aria-label="Main">
                <button
                    type="button"
                    aria-current=current(Screen::Servers)
                    on:click=move |_| state.screen.set(Screen::Servers)
                >
                    "Servers"
                </button>
                <Show when=signed_in>
                    <button
                        type="button"
                        aria-current=current(Screen::Profile)
                        on:click=move |_| state.screen.set(Screen::Profile)
                    >
                        "Profile"
                    </button>
                    <button
                        type="button"
                        aria-current=current(Screen::Room)
                        on:click=move |_| state.screen.set(Screen::Room)
                    >
                        "Room"
                    </button>
                </Show>
            </nav>
        </header>
    }
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

/// Connection, game time, frame time and graphics backend.
#[component]
fn StatusBar() -> impl IntoView {
    let state = AppState::expect();
    let connection = move || match state.status.get() {
        None => "Not connected".to_owned(),
        Some(ConnectionStatus::Connecting) => "Connecting…".to_owned(),
        Some(ConnectionStatus::Connected) => "Connected".to_owned(),
        Some(ConnectionStatus::Reconnecting { attempt, delay }) => {
            format!("Reconnecting (attempt {attempt}, in {} s)", delay.as_secs())
        }
        Some(ConnectionStatus::Closed) => "Disconnected".to_owned(),
    };
    let connection_class = move || match state.status.get() {
        Some(ConnectionStatus::Connected) => "dot ok",
        Some(ConnectionStatus::Connecting | ConnectionStatus::Reconnecting { .. }) => "dot busy",
        _ => "dot off",
    };
    let room = move || {
        let stats = state.room_stats.get();
        if state.screen.get() != Screen::Room {
            return Vec::new();
        }
        let mut parts = Vec::new();
        if let Some(room) = stats.room {
            parts.push(room);
        }
        if let Some(time) = stats.game_time {
            parts.push(format!("tick {time}"));
        }
        parts.push(format!("{} objects", stats.objects));
        if let Some(ms) = stats.frame_ms {
            parts.push(format!("frame {ms:.2} ms"));
        }
        if let Some(backend) = stats.backend {
            parts.push(backend.to_owned());
        }
        parts
            .into_iter()
            .map(|part| view! { <span>{part}</span> })
            .collect_view()
    };
    view! {
        <footer class="status-bar" aria-label="Status">
            <span>
                <span class=connection_class aria-hidden="true"></span>
                {connection}
            </span>
            {room}
            <span class="platform">
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
