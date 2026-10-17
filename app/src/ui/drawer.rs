//! The drawer of the world screen, with four tabs (the last one saved):
//!
//! - Room: the room in focus as a room control (viewer) and its facts, "Pop out room", and
//!   "Go to" through its portals (to the destination room, on its shard);
//! - Objects: the room's objects and the inspector, selection shared with the map;
//! - Console: the account's console, "Pop out console";
//! - Memory: the account's `Memory` and segments ([`MemoryPanel`]).
//!
//! On a wide window the drawer is at the right of the map: drag its left edge (or use the
//! arrow keys on it) to resize it; the width is saved and clamped again when the window
//! shrinks. Below the `sm` breakpoint ([`sheet_layout`]) it is a bottom sheet under the map:
//! drag its grab handle (or use the arrow keys on it) to give it one of its snap heights,
//! saved too. The sheet's Room tab shows the facts only: the map over it already shows the
//! room, so no second room viewer is made. The drawer collapses with its button or the top
//! bar's ([`resize`] holds both handles).
//!
//! The drawer follows the room and the console while collapsed, so nothing is missed.

mod resize;

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::map::PortalDestination;
use web_sys::KeyboardEvent;

use self::resize::{DrawerSize, Resizer, SheetHandle};
use crate::portals::GoTo;
use crate::profiles::Server;
use crate::route::{Route, Shard, WindowView};
use crate::saved::DrawerTab;
use crate::state::AppState;
use crate::ui::console::{ConsolePanel, ConsoleState};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::media::{media_query, sheet_layout};
use crate::ui::memory::{MemoryPanel, MemoryState};
use crate::ui::objects::{ObjectsPanel, Selection};
use crate::ui::room_panel::{RoomData, RoomInfo, RoomPreview};
use crate::ui::style::{self, concatcp};
use crate::ui::world::MapState;

/// The tabs, in order: tab, name, id suffix, icon.
const TABS: [(DrawerTab, &str, &str, IconKind); 4] = [
    (DrawerTab::Room, "Room", "room", IconKind::Room),
    (DrawerTab::Objects, "Objects", "objects", IconKind::List),
    (DrawerTab::Console, "Console", "console", IconKind::Terminal),
    (DrawerTab::Memory, "Memory", "memory", IconKind::Memory),
];

/// The collapse button in the drawer's head: quiet, small, lined up with the tabs.
const COLLAPSE_BUTTON: &str = concatcp!(
    style::ICON_BUTTON_GHOST_SM,
    " relative mb-1 ",
    style::TIP_END
);
/// A tab, underlined in the accent while selected; the tabs share the width on phones. In a
/// drawer narrower than 34rem (container queries on [`DRAWER`]) the tabs drop their icons and
/// share the width, so that the labels stay whole at the default width. In a drawer narrower
/// than 25rem (a phone) the tabs show their icons and counts only; the label stays the tab's
/// accessible name ([`TAB_LABEL`]).
const TAB: &str = "inline-flex items-center justify-center gap-2 min-w-0 min-h-control px-3 \
    -mb-px font-medium leading-tight whitespace-nowrap text-muted bg-transparent border-b-2 \
    border-transparent cursor-pointer transition-colors duration-(--duration-fast) ease-standard \
    hover:text-fg hover:bg-hover aria-selected:text-fg aria-selected:border-accent max-sm:flex-auto \
    @max-[34rem]:flex-auto @max-[34rem]:gap-1.5 @max-[34rem]:px-2 \
    @min-[25rem]:@max-[34rem]:[&>svg]:hidden";
/// A tab's label: cut when it does not fit; read by assistive technology only in the
/// narrowest drawer.
const TAB_LABEL: &str = "truncate @max-[25rem]:sr-only";
/// The drawer: at the right of the map, `--drawer-width` wide; under the map as a bottom sheet
/// on phones, `--drawer-height` high. The sheet keeps its handle and tabs, and the map keeps
/// at least 11rem (room for the map controls). `min-w-[280px]` and `max-w-[calc(100%-320px)]`
/// repeat the width limits of [`resize`] (which follow the window) for the world screen, which
/// is narrower than the window by the safe-area insets.
const DRAWER: &str = "@container relative flex flex-none flex-col w-(--drawer-width) min-w-[280px] \
    max-w-[calc(100%-320px)] min-h-0 bg-panel border-l border-edge animate-drawer-in \
    max-sm:w-auto max-sm:max-w-none max-sm:min-w-0 max-sm:h-(--drawer-height) \
    max-sm:min-h-[min(7rem,50%)] max-sm:max-h-[calc(100%-11rem)] max-sm:border-t \
    max-sm:border-l-0 max-sm:animate-none";

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
    let size = DrawerSize::new(state);
    let sheet = media_query(&sheet_layout());
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

    // The console runs while the world is shown; the Memory tab keeps its tree.
    let tools = account_tools(state, map, &server);
    let console = tools.map(|(console, _)| console);
    let memory = tools.map(|(_, memory)| memory);
    if let Some(console) = console {
        Effect::new(move |_| {
            let shown = open() && tab.get() == DrawerTab::Console;
            console.shown.set(shown);
            if shown {
                console.unread.set(0);
            }
        });
    }

    let (pop_out_room, pop_out_console) = pop_outs(state, map, &server);

    let user = StoredValue::new(user);
    let panel = move || match tab.get() {
        DrawerTab::Room => {
            // The sheet sits under the map, which already shows the room: no second viewer.
            let preview = move || {
                if sheet.get() {
                    return None;
                }
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
                            <p class=style::EMPTY_STATE>
                                "Zoom into a room or click one on the map to see it here."
                            </p>
                        }
                    }
                >
                    <div class="flex flex-col gap-3 p-3">
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
                        <p class=style::EMPTY_STATE>"Zoom into a room to list its objects."</p>
                    }
                }
            >
                <ObjectsPanel room=map.room snapshot selection user=user.get_value() centre go_to />
            </Show>
        }
        .into_any(),
        DrawerTab::Console => match console {
            Some(console) => {
                view! { <ConsolePanel state=console pop_out=pop_out_console /> }.into_any()
            }
            None => {
                view! { <p class=style::EMPTY_STATE>"Sign in to use the console."</p> }.into_any()
            }
        },
        DrawerTab::Memory => match memory {
            Some(memory) => view! { <MemoryPanel state=memory /> }.into_any(),
            None => {
                view! { <p class=style::EMPTY_STATE>"Sign in to see your Memory."</p> }.into_any()
            }
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
                class=DRAWER
                aria-label="Drawer"
                style=move || {
                    format!(
                        "--drawer-width: {}px; --drawer-height: {:.2}%",
                        size.width.get(),
                        size.height.get(),
                    )
                }
            >
                {move || {
                    if sheet.get() {
                        view! { <SheetHandle size /> }.into_any()
                    } else {
                        view! { <Resizer size /> }.into_any()
                    }
                }}
                <div class="flex flex-none items-end gap-2 px-2 pt-1 border-b border-edge">
                    <div class="flex flex-1 min-w-0" role="tablist" aria-label="Drawer">
                        <TabList tab select objects unread />
                    </div>
                    <button
                        type="button"
                        class=COLLAPSE_BUTTON
                        aria-label="Collapse drawer"
                        aria-controls="drawer"
                        data-tip="Collapse drawer"
                        on:click=move |_| state.update_saved(|saved| saved.settings.drawer = false)
                    >
                        <Icon kind=IconKind::Drawer />
                    </button>
                </div>
                <div
                    id="drawer-panel"
                    class="flex flex-col flex-1 min-h-0 overflow-y-auto overflow-x-hidden"
                    role="tabpanel"
                    aria-labelledby=labelled_by
                >
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
        DrawerTab::Room | DrawerTab::Memory => None,
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
                    view! { <span class=style::COUNT_NARROW>{text}</span> }
                })
            };
            view! {
                <button
                    type="button"
                    role="tab"
                    id=format!("drawer-tab-{id}")
                    class=TAB
                    aria-selected=move || (tab.get() == which).to_string()
                    aria-controls="drawer-panel"
                    tabindex=move || if tab.get() == which { "0" } else { "-1" }
                    on:click=move |_| select.run(which)
                    on:keydown=keydown
                >
                    <Icon kind=icon />
                    <span class=TAB_LABEL>{label}</span>
                    {count}
                </button>
            }
        })
        .collect_view()
}

/// The console and the Memory tab of the session on `server`, on the shard shown; `None` while
/// signed out.
fn account_tools(
    state: AppState,
    map: MapState,
    server: &Server,
) -> Option<(ConsoleState, MemoryState)> {
    let overseer = state.session.with_untracked(|session| {
        session
            .as_ref()
            .filter(|session| session.server == server.url)
            .map(|session| session.overseer.clone())
    })?;
    let shard = map
        .shard
        .get_untracked()
        .and_then(|shard| shard.name().map(str::to_owned));
    Some((
        ConsoleState::start(&overseer, &server.url, shard.clone()),
        MemoryState::new(&overseer, shard),
    ))
}

/// "Pop out room" and "Pop out console" of `server`: the room in focus or the console of the
/// shard shown, each in a window of its own.
fn pop_outs(state: AppState, map: MapState, server: &Server) -> (Callback<()>, Callback<()>) {
    let url = server.url.clone();
    let name = server.name.clone();
    let room = Callback::new(move |()| {
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
    let console = Callback::new(move |()| {
        let Some(shard) = map.shard.get_untracked() else {
            return;
        };
        let title = match shard.name() {
            Some(shard) => format!("Console · {shard} · {name} — Overseer"),
            None => format!("Console · {name} — Overseer"),
        };
        open_window(state, &url, shard, WindowView::Console, title);
    });
    (room, console)
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
