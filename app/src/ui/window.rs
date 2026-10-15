//! A popped-out room: the room control alone in its window, on its own world feed (the
//! window has its own session, resumed from the stored token).

use leptos::prelude::*;
use overseer::Overseer;
use overseer::map::RoomCoord;
use overseer_view::ViewMode;

use crate::profiles::Server;
use crate::route::Shard;
use crate::state::AppState;
use crate::ui::room_panel::RoomPanel;
use crate::ui::spawn_scoped;
use crate::ui::world::{MapState, Phase, open_world};

/// The room `room` of `server`'s shard `shard` alone.
#[component]
#[expect(
    clippy::needless_pass_by_value,
    reason = "component properties are owned"
)]
pub fn WindowScreen(
    server: Server,
    shard: Shard,
    room: RoomCoord,
    overseer: Overseer,
) -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let mode = state
        .saved
        .with_untracked(|saved| saved.places.get(&server.url).map(|place| place.mode))
        .unwrap_or_default();
    map.mode.set(mode);
    map.shard.set(Some(shard.clone()));
    map.room.set(Some(room));
    let tile = RwSignal::new(None);
    let attempt = RwSignal::new(0_u32);
    let url = server.url.clone();
    let shard_for_load = shard.clone();
    Effect::new(move |_| {
        attempt.track();
        map.phase.set(Phase::Loading);
        map.feed.set(None);
        let overseer = overseer.clone();
        let url = url.clone();
        let shard = shard_for_load.clone();
        spawn_scoped(async move {
            match open_world(state, map, &overseer, &url, &shard).await {
                Ok(feed) if feed.bounds().contains(room) => {
                    map.feed.set(Some(feed));
                    map.phase.set(Phase::Ready);
                }
                Ok(_) => map
                    .phase
                    .set(Phase::Failed(format!("{room} is outside this world."))),
                Err(error) => map.phase.set(Phase::Failed(error)),
            }
        });
    });
    let heading = match shard.name() {
        Some(shard) => format!("{room} · {shard} · {}", server.name),
        None => format!("{room} · {}", server.name),
    };
    if let Some(document) = web_sys::window().and_then(|window| window.document()) {
        document.set_title(&format!("{heading} — Overseer"));
    }
    let user = server.user.as_ref().map(|user| user.id.clone());
    let body = move || match map.phase.get() {
        Phase::Failed(error) => view! {
            <div class="overlay" role="alert">
                <p>{format!("The room could not be opened: {error}")}</p>
                <button type="button" class="primary" on:click=move |_| attempt.update(|n| *n += 1)>
                    "Try again"
                </button>
            </div>
        }
        .into_any(),
        _ => match map.feed.get() {
            Some(feed) => view! {
                <RoomPanel feed room=Signal::stored(Some(room)) tile user=user.clone() full=true />
            }
            .into_any(),
            None => view! {
                <div class="overlay" role="status">
                    <span class="spinner" aria-hidden="true"></span>
                    <p>"Opening the room…"</p>
                </div>
            }
            .into_any(),
        },
    };
    view! {
        <div class="window-screen">
            <header class="window-head">
                <h1>{heading}</h1>
                <ModeSwitch />
            </header>
            {body}
        </div>
    }
}

/// 2D/3D for every viewer of the window, saved for the route's server.
#[component]
pub fn ModeSwitch() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let set = move |mode: ViewMode| {
        map.set_mode(mode);
        let server = state
            .route
            .with_untracked(|route| route.server().and_then(|slug| state.server_by_slug(slug)));
        if let Some(server) = server {
            state.update_saved(|saved| saved.place_mut(&server.url).mode = mode);
        }
    };
    let button = move |mode: ViewMode, label: &'static str, name: &'static str| {
        view! {
            <button
                type="button"
                aria-pressed=move || (map.mode.get() == mode).to_string()
                aria-label=name
                on:click=move |_| set(mode)
            >
                {label}
            </button>
        }
    };
    view! {
        <div class="segmented" role="group" aria-label="View mode">
            {button(ViewMode::TopDown, "2D", "Top-down view")}
            {button(ViewMode::Perspective, "3D", "Perspective view")}
        </div>
    }
}
