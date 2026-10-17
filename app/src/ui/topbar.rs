//! The top bar: server switcher, shard switcher, breadcrumb, room search, 2D/3D, drawer
//! toggle and account menu. Its height never changes; world controls show on the world
//! route only. On narrow windows the labels collapse to their icons. On phones the brand and
//! the breadcrumb go (the server menu leads to the server list, the drawer names the room),
//! the server name and the shard switcher shrink to the space left, and the room search shows
//! as its magnifier: focused, it opens over the whole bar until Enter, Escape or a tap
//! elsewhere closes it.

use leptos::html::{Button, Div};
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::ConnectionStatus;
use overseer_view::ViewMode;
use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;

use crate::profiles::Profiles;
use crate::route::{Route, Shard, parse_room};
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::rate_limits::RateLimitsUi;
use crate::ui::style::{self, concatcp};
use crate::ui::world::MapState;

/// The brand link: logo and name, back to the server list. Gone on phones (the server menu
/// leads there too).
const BRAND: &str = concatcp!(
    "relative flex flex-none items-center justify-center gap-2 h-control min-w-control px-2 \
    mr-1 font-bold tracking-[0.02em] text-fg no-underline rounded-md hover:bg-hover \
    max-xs:hidden ",
    style::TIP
);
/// A 2D/3D button: a segment, narrower than a form's.
const MODE_SEGMENT: &str = concatcp!(
    style::BTN,
    " ",
    style::BTN_DEFAULT,
    " min-h-control min-w-control px-2 relative ",
    style::SEGMENT_SHAPE,
    " ",
    style::TIP
);
/// A menu's button: its content, then a caret that turns over while the menu is open (no caret
/// on phones). It shrinks with its menu when the bar is short of space.
const MENU_BUTTON: &str = concatcp!(
    style::BTN,
    " ",
    style::BTN_DEFAULT,
    " group min-h-control min-w-control max-w-[min(16rem,100%)] pl-3 pr-2 max-md:pl-2 \
    max-xs:px-2 aria-expanded:bg-panel-3 aria-expanded:border-edge-strong"
);
/// A menu's caret.
const MENU_CARET: &str = "inline-flex text-muted transition-transform duration-(--duration) \
    ease-standard group-aria-expanded:rotate-180 max-xs:hidden";
/// The list of a menu, under its button; never wider or taller than the window (it scrolls).
const MENU_LIST: &str = "absolute top-[calc(100%+0.25rem)] left-0 z-30 min-w-60 \
    max-w-[calc(100vw-1rem)] max-h-[calc(100dvh-var(--spacing-topbar)-var(--spacing-safe-t)-1rem)] \
    overflow-y-auto overscroll-contain p-1 list-none bg-panel-2 border border-edge rounded-lg \
    shadow-lg origin-top-left animate-pop-in";
/// The list of a menu at the window's right edge: aligned to its button's right edge.
const MENU_LIST_END: &str = "absolute top-[calc(100%+0.25rem)] right-0 z-30 min-w-60 \
    max-w-[calc(100vw-1rem)] max-h-[calc(100dvh-var(--spacing-topbar)-var(--spacing-safe-t)-1rem)] \
    overflow-y-auto overscroll-contain p-1 list-none bg-panel-2 border border-edge rounded-lg \
    shadow-lg origin-top-right animate-pop-in";
/// An item of a menu: a full-width quiet button (`group`: its main text marks a checked item).
const MENU_ITEM: &str = "group flex items-center justify-start gap-2 w-full min-h-control px-2 \
    leading-tight whitespace-nowrap text-left text-fg bg-transparent border border-transparent \
    rounded-md cursor-pointer transition-colors duration-(--duration-fast) ease-standard \
    enabled:hover:bg-hover focus-visible:bg-hover focus-visible:outline-offset-[-2px] \
    disabled:opacity-55 disabled:cursor-default";
/// The icon of a [`MENU_ITEM`].
const MENU_ICON: &str = "inline flex-none size-[1.15em] text-muted";
/// The main text of a [`MENU_ITEM`], accent while the item is checked; a long one is cut.
const ITEM_MAIN: &str = "flex-1 min-w-0 truncate font-medium group-aria-checked:text-accent";
/// A note after the main text of a [`MENU_ITEM`].
const ITEM_NOTE: &str = "flex-none text-sm text-muted";
/// A line between groups of menu items.
const MENU_SEPARATOR: &str = "h-px my-1 bg-edge";
/// An item of the breadcrumb: a "›" before every item but the first; on narrow windows only
/// the last item shows.
const CRUMB_ITEM: &str = "flex items-center min-w-0 not-first:before:content-['›'] \
    not-first:before:mx-2 not-first:before:text-faint max-md:not-first:before:hidden \
    max-md:first:not-last:hidden";
/// The breadcrumb's "World" button.
const CRUMB_BUTTON: &str = concatcp!(
    "relative inline-flex items-center justify-center gap-2 min-h-control px-2 font-semibold \
    leading-tight whitespace-nowrap text-muted bg-transparent border border-transparent \
    rounded-md cursor-pointer transition-colors duration-(--duration-fast) ease-standard \
    enabled:hover:text-fg enabled:hover:bg-hover aria-[current=page]:text-fg \
    aria-[current=page]:cursor-default ",
    style::TIP
);
/// The room search: in the bar; on phones, while focused, over the whole bar.
const SEARCH_FORM: &str = "relative flex flex-none items-center max-sm:focus-within:absolute \
    max-sm:focus-within:top-safe-t max-sm:focus-within:bottom-0 max-sm:focus-within:inset-x-2 \
    max-sm:focus-within:z-10 max-sm:focus-within:bg-panel";
/// The room search's input. Narrower on narrow windows; on phones only its magnifier shows
/// until it is focused, then it takes the whole bar (at once: a width that shrinks back after
/// the bar closes would push the bar wider than the window). No "/" hint space for a finger.
const SEARCH_INPUT: &str = "peer w-44 max-lg:w-34 max-md:w-30 max-md:sm:focus:w-36 \
    max-sm:w-control max-sm:focus:w-full min-h-control pl-(--spacing-control) max-sm:pl-0 \
    max-sm:focus:pl-(--spacing-control) pr-[1.9rem] max-md:pr-2 sm:pointer-coarse:pr-2 \
    max-sm:pr-0 max-sm:focus:pr-2 max-sm:cursor-pointer max-sm:focus:cursor-text \
    max-sm:not-focus:text-transparent text-fg text-ellipsis bg-sunken border border-edge \
    rounded-md transition-[border-color,width] max-sm:transition-[border-color] \
    duration-(--duration) ease-standard hover:border-edge-strong focus-visible:outline-offset-0 \
    focus-visible:border-focus aria-invalid:border-error placeholder:text-faint \
    max-sm:placeholder:text-transparent max-sm:focus:placeholder:text-faint \
    [&::-webkit-search-cancel-button]:hidden";
/// The room search's "/" key hint, gone while the input is focused or holds text, and for a
/// finger (no keyboard).
const SEARCH_KEY: &str = concatcp!(
    style::KBD,
    " absolute top-1/2 right-2 -translate-y-1/2 pointer-events-none transition-opacity \
    duration-(--duration-fast) ease-standard peer-focus:opacity-0 \
    peer-not-placeholder-shown:opacity-0 max-md:hidden pointer-coarse:hidden"
);
/// The room search's error, under the input (hidden while empty).
const SEARCH_ERROR: &str = "absolute top-[calc(100%+0.5rem)] right-0 z-20 w-max \
    max-w-[min(18rem,calc(100vw-1rem))] px-2 py-1 text-sm text-error bg-panel-2 border \
    border-error rounded-md shadow-sm empty:hidden";
/// The drawer toggle.
const DRAWER_TOGGLE: &str = concatcp!(style::ICON_BUTTON, " relative ", style::TIP_END);
/// The 2D/3D switch.
const MODE_SWITCH: &str = concatcp!(style::SEGMENTED, " flex-none");
/// The shard switcher: shrinks (cutting the name) when the bar is short of space.
const SHARD_SELECT: &str = concatcp!(style::INPUT, " flex-initial min-w-[4.5rem] max-w-40");

/// The top bar.
#[component]
pub fn TopBar() -> impl IntoView {
    let page = expect_context::<Memo<super::Page>>();
    let on_world = move || matches!(page.get(), super::Page::World { .. });
    view! {
        <header class="relative z-30 flex flex-none items-center gap-2 max-md:gap-1 \
            h-[calc(var(--spacing-topbar)+var(--spacing-safe-t))] min-w-0 pt-safe-t px-3 \
            max-md:px-2 bg-panel border-b border-edge">
            <a class=BRAND href="#/" aria-label="Overseer: servers" data-tip="Servers">
                <img src="icon.svg" alt="" width="22" height="22" />
                <span class="max-lg:hidden">"Overseer"</span>
            </a>
            <ServerSwitcher />
            <Show when=on_world>
                <ShardSwitcher />
            </Show>
            <Breadcrumb />
            <span class="flex-1 min-w-0"></span>
            <Show when=on_world>
                <RoomSearch />
                <ModeSwitch />
                <DrawerToggle />
            </Show>
            <AccountMenu />
        </header>
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
    let button = move |mode: ViewMode, icon: IconKind, text: &'static str, name: &'static str| {
        view! {
            <button
                type="button"
                class=MODE_SEGMENT
                aria-pressed=move || (map.mode.get() == mode).to_string()
                aria-label=name
                data-tip=name
                on:click=move |_| set(mode)
            >
                <Icon kind=icon />
                <span class="max-lg:hidden">{text}</span>
            </button>
        }
    };
    view! {
        <div class=MODE_SWITCH role="group" aria-label="View mode">
            {button(ViewMode::TopDown, IconKind::TopDown, "2D", "Top-down view")}
            {button(ViewMode::Perspective, IconKind::Perspective, "3D", "Perspective view")}
        </div>
    }
}

/// A menu button and its menu: arrow keys move between items, Escape closes it and returns
/// to the button, a click outside or on an item closes it.
#[component]
fn Menu(
    /// Element id of the menu.
    id: &'static str,
    /// Accessible name of the button.
    #[prop(into)]
    label: Signal<String>,
    /// The button's content.
    button: ViewFn,
    /// Opens the list aligned to the button's right edge (a menu at the window's right).
    #[prop(optional)]
    end: bool,
    /// The menu shrinks when the bar is short of space (down to a control's width; the
    /// button's content must cut itself, for example with `truncate`).
    #[prop(optional)]
    shrink: bool,
    /// `li role="none"` items holding `role="menuitem"` (or `menuitemradio`) buttons, each
    /// [`MENU_ITEM`]; `li role="separator"` lines ([`MENU_SEPARATOR`]) between groups.
    children: ChildrenFn,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let root = NodeRef::<Div>::new();
    let trigger = NodeRef::<Button>::new();
    let outside = window_event_listener(leptos::ev::pointerdown, move |event| {
        if !open.get_untracked() {
            return;
        }
        let inside = root.get_untracked().is_some_and(|root| {
            event
                .target()
                .and_then(|target| target.dyn_into::<web_sys::Node>().ok())
                .is_some_and(|target| root.contains(Some(&target)))
        });
        if !inside {
            open.set(false);
        }
    });
    on_cleanup(move || outside.remove());
    let items = move || {
        root.get_untracked()
            .and_then(|root| root.query_selector_all("[role^=menuitem]").ok())
            .map(|list| {
                (0..list.length())
                    .filter_map(|index| list.item(index))
                    .filter_map(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let focus_item = move |step: i32| {
        let items = items();
        if items.is_empty() {
            return;
        }
        let active = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.active_element());
        let current = items.iter().position(|item| {
            active
                .as_ref()
                .is_some_and(|active| active == item.unchecked_ref::<web_sys::Element>())
        });
        let count = i32::try_from(items.len()).unwrap_or(i32::MAX);
        let next = current.map_or(if step > 0 { 0 } else { count - 1 }, |index| {
            (i32::try_from(index).unwrap_or(0) + step).rem_euclid(count)
        });
        if let Some(item) = usize::try_from(next).ok().and_then(|next| items.get(next)) {
            let _ = item.focus();
        }
    };
    let keydown = move |event: KeyboardEvent| match event.key().as_str() {
        "Escape" => {
            event.prevent_default();
            open.set(false);
            if let Some(trigger) = trigger.get_untracked() {
                let _ = trigger.focus();
            }
        }
        "ArrowDown" => {
            event.prevent_default();
            if !open.get_untracked() {
                open.set(true);
            }
            request_animation_frame(move || focus_item(1));
        }
        "ArrowUp" => {
            event.prevent_default();
            focus_item(-1);
        }
        "Tab" => open.set(false),
        _ => {}
    };
    let menu_id = id;
    let list_class = if end { MENU_LIST_END } else { MENU_LIST };
    let root_class = if shrink {
        "relative flex-initial min-w-control"
    } else {
        "relative flex-none"
    };
    view! {
        <div class=root_class data-menu=menu_id node_ref=root on:keydown=keydown>
            <button
                type="button"
                class=MENU_BUTTON
                node_ref=trigger
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                aria-controls=menu_id
                aria-label=label
                on:click=move |_| open.update(|open| *open = !*open)
            >
                {move || button.run()}
                <span class=MENU_CARET>
                    <Icon kind=IconKind::ChevronDown />
                </span>
            </button>
            <ul
                id=menu_id
                role="menu"
                class=list_class
                hidden=move || !open.get()
                on:click=move |event| {
                    let item = event
                        .target()
                        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                        .and_then(|element| element.closest("[role^=menuitem]").ok().flatten());
                    if item.is_some() {
                        open.set(false);
                    }
                }
            >
                {children()}
            </ul>
        </div>
    }
}

/// The classes of the status dot of a connection.
pub fn status_class(status: Option<ConnectionStatus>) -> &'static str {
    match status {
        Some(ConnectionStatus::Connected) => style::DOT_OK,
        Some(ConnectionStatus::Connecting | ConnectionStatus::Reconnecting { .. }) => {
            style::DOT_BUSY
        }
        _ => style::DOT,
    }
}

/// The saved servers and their state; "Manage servers" opens the list.
#[component]
fn ServerSwitcher() -> impl IntoView {
    let state = AppState::expect();
    let current = move || {
        state.profiles.track();
        state
            .route
            .with(|route| route.server().and_then(|slug| state.server_by_slug(slug)))
    };
    let live_url = move || {
        state
            .session
            .with(|session| session.as_ref().map(|s| s.server.clone()))
    };
    let button = move || {
        let server = current();
        let live = server
            .as_ref()
            .is_some_and(|server| live_url().as_deref() == Some(server.url.as_str()));
        let dot = if live {
            status_class(state.status.get())
        } else {
            style::DOT
        };
        let name = server.map_or_else(|| "Servers".to_owned(), |server| server.name);
        view! {
            <span class=dot aria-hidden="true"></span>
            <span class="min-w-0 truncate max-w-48 max-md:max-w-32">{name}</span>
        }
    };
    let label = Signal::derive(move || {
        current().map_or_else(
            || "Server: none chosen".to_owned(),
            |server| format!("Server: {}", server.name),
        )
    });
    let items = move || {
        let live = live_url();
        let current_url = current().map(|server| server.url);
        state
            .profiles
            .get()
            .servers
            .into_iter()
            .map(|server| {
                let account = server.user.as_ref().map_or_else(
                    || "signed out".to_owned(),
                    |user| {
                        user.username
                            .clone()
                            .unwrap_or_else(|| "signed in".to_owned())
                    },
                );
                let dot = if live.as_deref() == Some(server.url.as_str()) {
                    status_class(state.status.get())
                } else {
                    style::DOT
                };
                let checked = (current_url.as_deref() == Some(server.url.as_str())).to_string();
                let url = server.url.clone();
                view! {
                    <li role="none">
                        <button
                            type="button"
                            class=MENU_ITEM
                            role="menuitemradio"
                            aria-checked=checked
                            on:click=move |_| state.open_server(&url)
                        >
                            <span class=dot aria-hidden="true"></span>
                            <span class=ITEM_MAIN>{server.name}</span>
                            <span class=ITEM_NOTE>{account}</span>
                        </button>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <Menu id="server-menu" label button=ViewFn::from(button) shrink=true>
            {items}
            <li role="separator" class=MENU_SEPARATOR></li>
            <li role="none">
                <button
                    type="button"
                    class=MENU_ITEM
                    role="menuitem"
                    on:click=move |_| state.navigate(Route::Servers)
                >
                    <Icon kind=IconKind::Server class=MENU_ICON />
                    <span class=ITEM_MAIN>"Manage servers…"</span>
                </button>
            </li>
        </Menu>
    }
}

/// The shard of the world; hidden with fewer than two shards.
#[component]
fn ShardSwitcher() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let change = move |event: leptos::ev::Event| {
        let Some(select) = event
            .target()
            .and_then(|target| target.dyn_into::<web_sys::HtmlSelectElement>().ok())
        else {
            return;
        };
        let shard = Shard::Named(select.value());
        let Some(slug) = state
            .route
            .with_untracked(|route| route.server().map(str::to_owned))
        else {
            return;
        };
        let room = state.server_by_slug(&slug).and_then(|server| {
            state.saved.with_untracked(|saved| {
                saved
                    .places
                    .get(&server.url)
                    .and_then(|place| place.rooms.get(&shard.to_string()).copied())
            })
        });
        state.navigate(Route::World {
            server: slug,
            shard: Some(shard),
            room,
        });
    };
    move || {
        let shards = map.shards.get();
        (shards.len() >= 2).then(|| {
            let current = map.shard.get().map(|shard| shard.to_string());
            let options = shards
                .into_iter()
                .map(|name| {
                    let selected = current.as_deref() == Some(name.as_str());
                    view! { <option value=name.clone() selected=selected>{name.clone()}</option> }
                })
                .collect_view();
            view! {
                <select class=SHARD_SELECT aria-label="Shard" on:change=change>
                    {options}
                </select>
            }
        })
    }
}

/// Where the user is: World › room on the world route, else the screen's name.
#[component]
fn Breadcrumb() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    move || {
        match state.route.get() {
        Route::World { .. } => {
            let world = map
                .shard
                .get()
                .and_then(|shard| shard.name().map(str::to_owned))
                .unwrap_or_else(|| "World".to_owned());
            let room = map.room.get();
            view! {
                <nav class="flex-initial min-w-0 overflow-hidden max-xs:hidden" aria-label="Breadcrumb">
                    <ol class="flex items-center min-w-0">
                        <li class=CRUMB_ITEM>
                            <button
                                type="button"
                                class=CRUMB_BUTTON
                                aria-current=room.is_none().then_some("page")
                                data-tip="Show the whole world"
                                on:click=move |_| map.show_world()
                            >
                                <Icon kind=IconKind::Globe />
                                <span class="max-md:hidden">{world}</span>
                            </button>
                        </li>
                        {room
                            .map(|room| {
                                view! {
                                    <li class=CRUMB_ITEM aria-current="page">
                                        <span class="font-semibold whitespace-nowrap">
                                            {room.to_string()}
                                        </span>
                                    </li>
                                }
                            })}
                    </ol>
                </nav>
            }
            .into_any()
        }
        Route::Servers => view! {
            <span class="flex-none px-2 font-semibold whitespace-nowrap max-lg:hidden">"Servers"</span>
        }
        .into_any(),
        Route::Profile { .. } => view! {
            <span class="flex-none px-2 font-semibold whitespace-nowrap max-lg:hidden">"Account"</span>
        }
        .into_any(),
        Route::User { .. } => view! {
            <span class="flex-none px-2 font-semibold whitespace-nowrap max-lg:hidden">"Player"</span>
        }
        .into_any(),
        Route::Window { .. } => ().into_any(),
    }
    }
}

/// "W8N3" frames that room and hands the keyboard back to the map; Escape clears the search
/// and returns to the map too.
#[component]
fn RoomSearch() -> impl IntoView {
    let map = MapState::expect();
    let query = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let text = query.get_untracked();
        let Some(room) = parse_room(&text) else {
            error.set(Some(format!(
                "“{}” is not a room name such as W8N3",
                text.trim()
            )));
            return;
        };
        if map
            .bounds
            .get_untracked()
            .is_some_and(|bounds| !bounds.contains(room))
        {
            error.set(Some(format!("{room} is outside this world")));
            return;
        }
        error.set(None);
        query.set(String::new());
        map.go_to(room);
        map.focus_map();
    };
    view! {
        <form class=SEARCH_FORM role="search" on:submit=submit novalidate>
            <span class="absolute top-1/2 left-[calc(var(--spacing-control)/2-0.575em)] -translate-y-1/2 flex text-faint pointer-events-none">
                <Icon kind=IconKind::Search />
            </span>
            <input
                id="room-search"
                class=SEARCH_INPUT
                type="search"
                placeholder="Go to room"
                aria-label="Go to room"
                aria-keyshortcuts="/"
                aria-invalid=move || error.with(Option::is_some).to_string()
                aria-describedby="room-search-error"
                autocomplete="off"
                spellcheck="false"
                bind:value=query
                on:input=move |_| error.set(None)
                on:blur=move |_| error.set(None)
                on:keydown=move |event: KeyboardEvent| {
                    if event.key() == "Escape" {
                        event.prevent_default();
                        query.set(String::new());
                        error.set(None);
                        map.focus_map();
                    }
                }
            />
            <kbd class=SEARCH_KEY aria-hidden="true">"/"</kbd>
            <span id="room-search-error" class=SEARCH_ERROR role="alert">
                {move || error.get()}
            </span>
        </form>
    }
}

/// Opens or closes the drawer (saved in the settings).
#[component]
fn DrawerToggle() -> impl IntoView {
    let state = AppState::expect();
    let open = move || state.saved.with(|saved| saved.settings.drawer);
    let toggle = move |_| {
        let next = !open();
        state.update_saved(|saved| saved.settings.drawer = next);
    };
    let tip = move || {
        if open() {
            "Hide the drawer"
        } else {
            "Show the drawer"
        }
    };
    view! {
        <button
            type="button"
            class=DRAWER_TOGGLE
            aria-expanded=move || open().to_string()
            aria-controls="drawer"
            aria-label="Drawer"
            data-tip=tip
            on:click=toggle
        >
            <Icon kind=IconKind::Drawer />
        </button>
    }
}

/// The route slug and display name of the account signed in on the server with base URL
/// `server`: its saved username, "Account" until it is known.
///
/// A memo, so that it changes only when the slug or the name does: a resume rewrites the
/// saved profiles (`begin`, `remember_username`) with the same account, and rebuilding the
/// account menu then would close it under the user's pointer.
fn signed_in_account(
    profiles: Signal<Profiles>,
    server: Signal<Option<String>>,
) -> Memo<Option<(String, String)>> {
    Memo::new(move |_| {
        let url = server.get()?;
        profiles.with(|profiles| {
            let slug = profiles.slug(&url)?;
            let name = profiles
                .server(&url)?
                .user
                .as_ref()
                .and_then(|user| user.username.clone())
                .unwrap_or_else(|| "Account".to_owned());
            Some((slug, name))
        })
    })
}

/// The signed-in account: profile, rate limits and sign-out.
#[component]
fn AccountMenu() -> impl IntoView {
    let state = AppState::expect();
    let limits = RateLimitsUi::expect();
    let server = Signal::derive(move || {
        state
            .session
            .with(|session| session.as_ref().map(|session| session.server.clone()))
    });
    let account = signed_in_account(state.profiles.into(), server);
    move || {
        account.get().map(|(slug, name)| {
            let label = format!("Account: {name}");
            let button_name = name.clone();
            view! {
                <Menu
                    id="account-menu"
                    label=label
                    end=true
                    button=ViewFn::from(move || {
                        view! {
                            <Icon kind=IconKind::User />
                            <span class="truncate max-md:hidden">{button_name.clone()}</span>
                        }
                    })
                >
                    <li role="none">
                        <button
                            type="button"
                            class=MENU_ITEM
                            role="menuitem"
                            on:click={
                                let slug = slug.clone();
                                move |_| state.navigate(Route::Profile { server: slug.clone() })
                            }
                        >
                            <Icon kind=IconKind::User class=MENU_ICON />
                            <span class=ITEM_MAIN>"Profile"</span>
                        </button>
                    </li>
                    <li role="none">
                        <button
                            type="button"
                            class=MENU_ITEM
                            role="menuitem"
                            on:click=move |_| limits.open.set(true)
                        >
                            <Icon kind=IconKind::Gauge class=MENU_ICON />
                            <span class=ITEM_MAIN>"Rate limits"</span>
                        </button>
                    </li>
                    <li role="separator" class=MENU_SEPARATOR></li>
                    <li role="none">
                        <button
                            type="button"
                            class=MENU_ITEM
                            role="menuitem"
                            on:click=move |_| spawn_local(async move { state.sign_out().await })
                        >
                            <Icon kind=IconKind::SignOut class=MENU_ICON />
                            <span class=ITEM_MAIN>"Sign out"</span>
                        </button>
                    </li>
                </Menu>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use leptos::prelude::*;
    use leptos::reactive::effect::ImmediateEffect;
    use leptos::reactive::owner::Owner;

    use super::signed_in_account;
    use crate::profiles::{Profiles, SavedUser, Server, ServerKind};

    const URL: &str = "http://127.0.0.1:21025/";

    fn profiles(username: Option<&str>) -> Profiles {
        let mut server = Server::new("Local".to_owned(), URL.to_owned(), ServerKind::Private);
        server.user = Some(SavedUser {
            id: "u1".to_owned(),
            username: username.map(str::to_owned),
            api_token: Some(false),
        });
        let mut profiles = Profiles::default();
        profiles.servers.push(server);
        profiles
    }

    #[test]
    fn the_account_changes_only_with_its_slug_or_name() {
        let owner = Owner::new();
        owner.set();
        let saved = RwSignal::new(profiles(Some("alice")));
        let server = RwSignal::new(Some(URL.to_owned()));
        let account = signed_in_account(saved.into(), server.into());
        // What the account menu would be rebuilt with, once per rebuild.
        let builds = Arc::new(Mutex::new(Vec::new()));
        let _menu = ImmediateEffect::new_isomorphic({
            let builds = Arc::clone(&builds);
            move || {
                let account = account.get().map(|(slug, name)| format!("{slug} {name}"));
                builds
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(account);
            }
        });
        let built = || {
            builds
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        };
        assert_eq!(built(), [Some("127.0.0.1-21025 alice".to_owned())]);

        // A resume rewrites the profiles with the same account (`begin`, then
        // `remember_username`): the menu is not rebuilt.
        saved.set(profiles(Some("alice")));
        saved.update(|profiles| profiles.servers[1].login = Some("alice".to_owned()));
        assert_eq!(built().len(), 1);

        saved.set(profiles(Some("bob")));
        saved.set(profiles(None));
        server.set(None);
        assert_eq!(
            built(),
            [
                Some("127.0.0.1-21025 alice".to_owned()),
                Some("127.0.0.1-21025 bob".to_owned()),
                Some("127.0.0.1-21025 Account".to_owned()),
                None,
            ]
        );
    }
}
