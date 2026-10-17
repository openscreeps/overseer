//! A popped-out window ([`Route::Window`](crate::route::Route::Window)), on its own session
//! (resumed from the stored token): a room control alone ([`RoomPanel`], on its own world
//! feed) or the console alone ([`ConsolePanel`]).

use leptos::prelude::*;
use overseer::Overseer;
use overseer::map::RoomCoord;

use crate::profiles::Server;
use crate::route::{Shard, WindowView};
use crate::state::AppState;
use crate::ui::console::{ConsolePanel, ConsoleState};
use crate::ui::objects::Selection;
use crate::ui::room_panel::{RoomData, RoomPanel};
use crate::ui::spawn_scoped;
use crate::ui::style;
use crate::ui::topbar::ModeSwitch;
use crate::ui::world::{MapState, Phase, open_world};

/// A window's screen: its head over its content.
const SCREEN: &str = "relative flex flex-col flex-1 min-h-0";
/// A window's head: its title and controls (as tall as its controls need, at least 11 steps).
const HEAD: &str = "flex flex-none items-center justify-between gap-4 min-h-11 px-3 py-1 \
    bg-panel border-b border-edge";
/// A window's title.
const TITLE: &str = "text-base font-semibold leading-tight";

/// `view` of `server`'s shard `shard` alone.
#[component]
pub fn WindowScreen(
    server: Server,
    shard: Shard,
    view: WindowView,
    overseer: Overseer,
) -> impl IntoView {
    let place = match view {
        WindowView::Room(room) => room.to_string(),
        WindowView::Console => "Console".to_owned(),
    };
    let heading = match shard.name() {
        Some(name) => format!("{place} · {name} · {}", server.name),
        None => format!("{place} · {}", server.name),
    };
    if let Some(document) = web_sys::window().and_then(|window| window.document()) {
        document.set_title(&format!("{heading} — Overseer"));
    }
    match view {
        WindowView::Room(room) => {
            view! { <RoomWindow server shard room overseer heading /> }.into_any()
        }
        WindowView::Console => view! { <ConsoleWindow server shard overseer heading /> }.into_any(),
    }
}

/// The room `room` alone.
#[component]
fn RoomWindow(
    server: Server,
    shard: Shard,
    room: RoomCoord,
    overseer: Overseer,
    heading: String,
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
    // The creeps of the room carry their owners' badges.
    #[cfg(target_arch = "wasm32")]
    crate::ui::world::badges::open(map, &overseer, &server);
    let attempt = RwSignal::new(0_u32);
    let url = server.url.clone();
    Effect::new(move |_| {
        attempt.track();
        map.phase.set(Phase::Loading);
        map.feed.set(None);
        let overseer = overseer.clone();
        let url = url.clone();
        let shard = shard.clone();
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
    let room_signal = Signal::stored(Some(room));
    let data = RoomData::watch(room_signal, map);
    let snapshot = Signal::derive(move || data.snapshot());
    let selection = Selection::new(RwSignal::new(None), room_signal, snapshot, || {});
    let user = server.user.as_ref().map(|user| user.id.clone());
    let body = move || match map.phase.get() {
        Phase::Failed(error) => view! {
            <div class=style::OVERLAY role="alert">
                <p class="max-w-128">{format!("The room could not be opened: {error}")}</p>
                <button
                    type="button"
                    class=style::BUTTON_PRIMARY
                    on:click=move |_| attempt.update(|n| *n += 1)
                >
                    "Try again"
                </button>
            </div>
        }
        .into_any(),
        _ => match map.feed.get() {
            Some(feed) => view! {
                <RoomPanel feed room=room_signal data selection user=user.clone() />
            }
            .into_any(),
            None => view! {
                <div class=style::OVERLAY role="status">
                    <span class=style::SPINNER aria-hidden="true"></span>
                    <p class="max-w-128">"Opening the room…"</p>
                </div>
            }
            .into_any(),
        },
    };
    view! {
        <div class=SCREEN>
            <header class=HEAD>
                <h1 class=TITLE>{heading}</h1>
                <ModeSwitch />
            </header>
            {body}
        </div>
    }
}

/// The console alone.
#[component]
fn ConsoleWindow(
    server: Server,
    shard: Shard,
    overseer: Overseer,
    heading: String,
) -> impl IntoView {
    let console = ConsoleState::start(&overseer, &server.url, shard.name().map(str::to_owned));
    view! {
        <div class=SCREEN>
            <header class=HEAD>
                <h1 class=TITLE>{heading}</h1>
            </header>
            <ConsolePanel state=console />
        </div>
    }
}
