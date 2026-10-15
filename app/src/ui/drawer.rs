//! The right drawer of the world screen: the room in focus as a room control (info, viewer,
//! inspector) and "Pop out room".

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::profiles::Server;
use crate::route::Route;
use crate::state::AppState;
use crate::ui::room_panel::RoomPanel;
use crate::ui::world::MapState;

/// The drawer (hidden when closed in the settings).
#[component]
#[expect(
    clippy::needless_pass_by_value,
    reason = "component properties are owned"
)]
pub fn Drawer(server: Server) -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let open = move || state.saved.with(|saved| saved.settings.drawer);
    let user = server.user.as_ref().map(|user| user.id.clone());
    let url = server.url.clone();
    let name = server.name.clone();
    let pop_out = move |_| {
        let (Some(room), Some(shard), Some(slug)) = (
            map.room.get_untracked(),
            map.shard.get_untracked(),
            state.slug(&url),
        ) else {
            return;
        };
        let route = Route::Window {
            server: slug,
            shard: shard.clone(),
            room,
        };
        let title = match shard.name() {
            Some(shard) => format!("{room} · {shard} · {name} — Overseer"),
            None => format!("{room} · {name} — Overseer"),
        };
        let platform = state.platform();
        spawn_local(async move {
            if let Err(error) = platform.open_window(&route.hash(), &title).await {
                state
                    .banner
                    .set(Some(format!("Could not pop out {room}: {error}")));
            }
        });
    };
    let panel = move || {
        map.feed.get().map(|feed| {
            view! {
                <RoomPanel feed room=map.room tile=map.tile user=user.clone() />
            }
        })
    };
    view! {
        <Show when=open>
            <aside id="drawer" class="drawer" aria-labelledby="drawer-title">
                <header class="drawer-head">
                    <h2 id="drawer-title" class="visually-hidden">"Room"</h2>
                    <button
                        type="button"
                        class="pop-out"
                        disabled=move || map.room.with(Option::is_none)
                        on:click=pop_out.clone()
                    >
                        "Pop out room"
                    </button>
                </header>
                {panel.clone()}
            </aside>
        </Show>
    }
}
