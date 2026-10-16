//! The right drawer of the world screen: resizable (drag or arrow keys on its edge, width
//! saved), collapsible (its button or the top bar's), with three tabs (the last one saved):
//!
//! - Room: the room in focus as a room control (viewer) and its facts, "Pop out room", and
//!   "Go to" through its portals (to the destination room, on its shard);
//! - Objects: the room's objects and the inspector, selection shared with the map;
//! - Console: the account's console, "Pop out console".
//!
//! The drawer follows the room and the console while collapsed, so nothing is missed.

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::map::PortalDestination;
use web_sys::{KeyboardEvent, PointerEvent};

use crate::portals::GoTo;
use crate::profiles::Server;
use crate::route::{Route, Shard, WindowView};
use crate::saved::{DRAWER_WIDTH, DrawerTab};
use crate::state::AppState;
use crate::ui::console::{ConsolePanel, ConsoleState};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::objects::{ObjectsPanel, Selection};
use crate::ui::room_panel::{RoomData, RoomInfo, RoomPreview};
use crate::ui::world::MapState;

/// Narrowest drawer, in CSS pixels.
const MIN_WIDTH: u16 = 280;
/// Widest drawer, in CSS pixels (the map keeps [`MAP_MIN_WIDTH`] besides).
const MAX_WIDTH: u16 = 960;
/// Narrowest map next to the drawer, in CSS pixels.
const MAP_MIN_WIDTH: f64 = 320.0;
/// Arrow-key resize step, in CSS pixels.
const KEY_STEP: u16 = 16;

/// The tabs, in order: tab, name, id suffix, icon.
const TABS: [(DrawerTab, &str, &str, IconKind); 3] = [
    (DrawerTab::Room, "Room", "room", IconKind::Room),
    (DrawerTab::Objects, "Objects", "objects", IconKind::List),
    (DrawerTab::Console, "Console", "console", IconKind::Terminal),
];

/// The widest drawer the window allows.
fn max_width() -> u16 {
    let window = web_sys::window()
        .and_then(|window| window.inner_width().ok())
        .and_then(|width| width.as_f64())
        .unwrap_or(f64::from(MAX_WIDTH) + MAP_MIN_WIDTH);
    let room = (window - MAP_MIN_WIDTH).clamp(f64::from(MIN_WIDTH), f64::from(MAX_WIDTH));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u16 range of MIN_WIDTH..=MAX_WIDTH"
    )]
    let room = room as u16;
    room
}

/// `width` within what the window allows.
fn clamp_width(width: f64) -> u16 {
    let max = max_width();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to the u16 range of MIN_WIDTH..=max"
    )]
    let width = width.round().clamp(f64::from(MIN_WIDTH), f64::from(max)) as u16;
    width
}

/// The drawer (collapsed when closed in the settings).
#[component]
#[expect(
    clippy::needless_pass_by_value,
    reason = "component properties are owned"
)]
pub fn Drawer(server: Server) -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let open = move || state.saved.with(|saved| saved.settings.drawer);
    let tab = Memo::new(move |_| state.saved.with(|saved| saved.settings.drawer_tab));
    let set_tab = move |next: DrawerTab| {
        if tab.get_untracked() != next {
            state.update_saved(|saved| saved.settings.drawer_tab = next);
        }
    };
    let width = RwSignal::new(clamp_width(f64::from(
        state
            .saved
            .with_untracked(|saved| saved.settings.drawer_width),
    )));
    let user = server.user.as_ref().map(|user| user.id.clone());

    // The room in focus and the selection, shared with the map.
    let data = RoomData::watch(map.room.into(), map);
    let snapshot = Signal::derive(move || data.snapshot());
    let selection = Selection::new(map.tile, map.room.into(), snapshot, move || {
        if tab.get_untracked() == DrawerTab::Room {
            set_tab(DrawerTab::Objects);
        }
    });
    let centre = Callback::new(move |(room, x, y)| {
        map.with_viewer(|viewer| viewer.look_at(room, Some((x, y)), true));
    });
    // A portal's "Go to": its room on this shard, else the room on the other shard.
    let go_to = Callback::new(move |destination: PortalDestination| {
        let shard = map.shard.get_untracked();
        match GoTo::of(&destination, shard.as_ref().and_then(Shard::name)) {
            GoTo::Room(room) => map.go_to(room),
            GoTo::Shard(shard, room) => {
                let Some(slug) = state
                    .route
                    .with_untracked(|route| route.server().map(str::to_owned))
                else {
                    return;
                };
                state.navigate(Route::World {
                    server: slug,
                    shard: Some(Shard::Named(shard)),
                    room: Some(room),
                });
            }
        }
    });

    // The console runs while the world is shown.
    let console = state
        .session
        .with_untracked(|session| {
            session
                .as_ref()
                .filter(|session| session.server == server.url)
                .map(|session| session.overseer.clone())
        })
        .map(|overseer| {
            let shard = map
                .shard
                .get_untracked()
                .and_then(|shard| shard.name().map(str::to_owned));
            ConsoleState::start(&overseer, &server.url, shard)
        });
    if let Some(console) = console {
        Effect::new(move |_| {
            let shown = open() && tab.get() == DrawerTab::Console;
            console.shown.set(shown);
            if shown {
                console.unread.set(0);
            }
        });
    }

    let url = server.url.clone();
    let name = server.name.clone();
    let pop_out_room = Callback::new(move |()| {
        let (Some(room), Some(shard)) = (map.room.get_untracked(), map.shard.get_untracked())
        else {
            return;
        };
        let title = match shard.name() {
            Some(shard) => format!("{room} · {shard} · {name} — Overseer"),
            None => format!("{room} · {name} — Overseer"),
        };
        open_window(state, &url, shard, WindowView::Room(room), title);
    });
    let url = server.url.clone();
    let name = server.name.clone();
    let pop_out_console = Callback::new(move |()| {
        let Some(shard) = map.shard.get_untracked() else {
            return;
        };
        let title = match shard.name() {
            Some(shard) => format!("Console · {shard} · {name} — Overseer"),
            None => format!("Console · {name} — Overseer"),
        };
        open_window(state, &url, shard, WindowView::Console, title);
    });

    let user = StoredValue::new(user);
    let panel = move || match tab.get() {
        DrawerTab::Room => {
            let preview = move || {
                map.feed.get().map(|feed| {
                    view! {
                        <RoomPreview feed room=map.room tile=map.tile user=user.get_value() data />
                    }
                })
            };
            view! {
                <Show
                    when=move || map.room.with(Option::is_some)
                    fallback=|| {
                        view! {
                            <p class="empty-state">
                                "Zoom into a room or click one on the map to see it here."
                            </p>
                        }
                    }
                >
                    <div class="room-tab">
                        <RoomInfo room=map.room data user=user.get_value() pop_out=pop_out_room go_to>
                            {preview}
                        </RoomInfo>
                    </div>
                </Show>
            }
            .into_any()
        }
        DrawerTab::Objects => view! {
            <Show
                when=move || map.room.with(Option::is_some)
                fallback=|| {
                    view! {
                        <p class="empty-state">"Zoom into a room to list its objects."</p>
                    }
                }
            >
                <ObjectsPanel room=map.room snapshot selection user=user.get_value() centre />
            </Show>
        }
        .into_any(),
        DrawerTab::Console => match console {
            Some(console) => {
                view! { <ConsolePanel state=console pop_out=pop_out_console /> }.into_any()
            }
            None => view! { <p class="empty-state">"Sign in to use the console."</p> }.into_any(),
        },
    };
    let objects = Signal::derive(move || {
        snapshot.with(|snapshot| snapshot.as_ref().map(|snapshot| snapshot.objects.len()))
    });
    let unread = Signal::derive(move || console.map_or(0, |console| console.unread.get()));
    let select = Callback::new(set_tab);
    let labelled_by = move || {
        let id = TABS
            .iter()
            .find(|(which, ..)| *which == tab.get())
            .map_or("room", |(_, _, id, _)| *id);
        format!("drawer-tab-{id}")
    };
    view! {
        <Show when=open>
            <aside
                id="drawer"
                class="drawer"
                aria-label="Drawer"
                style=move || format!("--drawer-width: {}px", width.get())
            >
                <Resizer width />
                <div class="drawer-head">
                    <div class="drawer-tabs" role="tablist" aria-label="Drawer">
                        <TabList tab select objects unread />
                    </div>
                    <button
                        type="button"
                        class="icon-button sm"
                        aria-label="Collapse drawer"
                        aria-controls="drawer"
                        data-tip="Collapse drawer"
                        data-tip-pos="bottom-end"
                        on:click=move |_| state.update_saved(|saved| saved.settings.drawer = false)
                    >
                        <Icon kind=IconKind::Drawer />
                    </button>
                </div>
                <div id="drawer-panel" class="drawer-panel" role="tabpanel" aria-labelledby=labelled_by>
                    {panel}
                </div>
            </aside>
        </Show>
    }
}

/// The tabs, with the room's object count and the console's unread lines; arrow keys, Home
/// and End move between them.
#[component]
fn TabList(
    tab: Memo<DrawerTab>,
    select: Callback<DrawerTab>,
    objects: Signal<Option<usize>>,
    unread: Signal<usize>,
) -> impl IntoView {
    let badge = move |which: DrawerTab| match which {
        DrawerTab::Room => None,
        DrawerTab::Objects => objects.get(),
        DrawerTab::Console => Some(unread.get()).filter(|unread| *unread > 0),
    };
    TABS.iter()
        .map(|&(which, label, id, icon)| {
            let keydown = move |event: KeyboardEvent| {
                let index = TABS
                    .iter()
                    .position(|(tab, ..)| *tab == which)
                    .unwrap_or_default();
                let next = match event.key().as_str() {
                    "ArrowRight" => (index + 1) % TABS.len(),
                    "ArrowLeft" => (index + TABS.len() - 1) % TABS.len(),
                    "Home" => 0,
                    "End" => TABS.len() - 1,
                    _ => return,
                };
                event.prevent_default();
                let (next_tab, _, id, _) = TABS[next];
                select.run(next_tab);
                focus(&format!("drawer-tab-{id}"));
            };
            let count = move || {
                badge(which).map(|count| {
                    let text = if count > 999 {
                        "999+".to_owned()
                    } else {
                        count.to_string()
                    };
                    view! { <span class="count">{text}</span> }
                })
            };
            view! {
                <button
                    type="button"
                    role="tab"
                    id=format!("drawer-tab-{id}")
                    class="drawer-tab"
                    aria-selected=move || (tab.get() == which).to_string()
                    aria-controls="drawer-panel"
                    tabindex=move || if tab.get() == which { "0" } else { "-1" }
                    on:click=move |_| select.run(which)
                    on:keydown=keydown
                >
                    <Icon kind=icon />
                    <span>{label}</span>
                    {count}
                </button>
            }
        })
        .collect_view()
}

/// Opens `view` of the server with base URL `url` in a window titled `title`.
fn open_window(state: AppState, url: &str, shard: Shard, view: WindowView, title: String) {
    let Some(slug) = state.slug(url) else {
        return;
    };
    let route = Route::Window {
        server: slug,
        shard,
        view,
    };
    let platform = state.platform();
    spawn_local(async move {
        if let Err(error) = platform.open_window(&route.hash(), &title).await {
            state
                .banner
                .set(Some(format!("Could not open the window: {error}")));
        }
    });
}

/// Focuses the element with id `id` once rendered.
fn focus(id: &str) {
    let id = id.to_owned();
    request_animation_frame(move || {
        if let Some(element) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.get_element_by_id(&id))
            .and_then(|element| {
                wasm_bindgen::JsCast::dyn_into::<web_sys::HtmlElement>(element).ok()
            })
        {
            let _ = element.focus();
        }
    });
}

/// The drawer's left edge: drag it (or use the arrow keys on it) to resize the drawer;
/// double-click restores the default width. The width is saved when the drag ends.
#[component]
fn Resizer(width: RwSignal<u16>) -> impl IntoView {
    let state = AppState::expect();
    // The pointer's x and the width when the drag began.
    let drag = StoredValue::new(None::<(f64, u16)>);
    let save = move || {
        let width = width.get_untracked();
        state.update_saved(|saved| saved.settings.drawer_width = width);
    };
    let down = move |event: PointerEvent| {
        if event.button() != 0 {
            return;
        }
        event.prevent_default();
        if let Some(target) = event
            .target()
            .and_then(|target| wasm_bindgen::JsCast::dyn_into::<web_sys::Element>(target).ok())
        {
            let _ = target.set_pointer_capture(event.pointer_id());
        }
        drag.set_value(Some((f64::from(event.client_x()), width.get_untracked())));
    };
    let moved = move |event: PointerEvent| {
        if let Some((x, start)) = drag.get_value() {
            width.set(clamp_width(
                f64::from(start) + x - f64::from(event.client_x()),
            ));
        }
    };
    let up = move |_: PointerEvent| {
        if drag.get_value().is_some() {
            drag.set_value(None);
            save();
        }
    };
    let keydown = move |event: KeyboardEvent| {
        let current = width.get_untracked();
        let next = match event.key().as_str() {
            "ArrowLeft" => current.saturating_add(KEY_STEP),
            "ArrowRight" => current.saturating_sub(KEY_STEP),
            "Home" => MIN_WIDTH,
            "End" => MAX_WIDTH,
            _ => return,
        };
        event.prevent_default();
        width.set(clamp_width(f64::from(next)));
        save();
    };
    view! {
        <div
            class="drawer-resize"
            role="separator"
            tabindex="0"
            aria-orientation="vertical"
            aria-label="Resize drawer"
            aria-controls="drawer"
            aria-valuemin=MIN_WIDTH
            aria-valuemax=max_width()
            aria-valuenow=move || width.get()
            data-tip="Drag to resize · double-click to reset"
            data-tip-pos="left"
            on:pointerdown=down
            on:pointermove=moved
            on:pointerup=up
            on:pointercancel=up
            on:keydown=keydown
            on:dblclick=move |_| {
                width.set(clamp_width(f64::from(DRAWER_WIDTH)));
                save();
            }
        >
            <span class="grip" aria-hidden="true">
                <Icon kind=IconKind::Grip />
            </span>
        </div>
    }
}
