//! The top bar: server switcher, shard switcher, breadcrumb, room search, 2D/3D, drawer
//! toggle and account menu. Its height never changes; world controls show on the world
//! route only. On narrow windows the labels collapse to their icons (`style.css`).

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
use crate::ui::world::MapState;

/// The top bar.
#[component]
pub fn TopBar() -> impl IntoView {
    let page = expect_context::<Memo<super::Page>>();
    let on_world = move || matches!(page.get(), super::Page::World { .. });
    view! {
        <header class="topbar">
            <a class="brand" href="#/" aria-label="Overseer: servers" data-tip="Servers">
                <img src="icon.svg" alt="" width="22" height="22" />
                <span class="brand-name">"Overseer"</span>
            </a>
            <ServerSwitcher />
            <Show when=on_world>
                <ShardSwitcher />
            </Show>
            <Breadcrumb />
            <span class="spacer"></span>
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
                aria-pressed=move || (map.mode.get() == mode).to_string()
                aria-label=name
                data-tip=name
                on:click=move |_| set(mode)
            >
                <Icon kind=icon />
                <span class="button-text">{text}</span>
            </button>
        }
    };
    view! {
        <div class="segmented mode-switch" role="group" aria-label="View mode">
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
    /// `li role="none"` items holding `role="menuitem"` (or `menuitemradio`) buttons.
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
    view! {
        <div class="menu" data-menu=menu_id node_ref=root on:keydown=keydown>
            <button
                type="button"
                class="menu-button"
                node_ref=trigger
                aria-haspopup="menu"
                aria-expanded=move || open.get().to_string()
                aria-controls=menu_id
                aria-label=label
                on:click=move |_| open.update(|open| *open = !*open)
            >
                {move || button.run()}
                <span class="caret"><Icon kind=IconKind::ChevronDown /></span>
            </button>
            <ul
                id=menu_id
                role="menu"
                class="menu-list"
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

/// The CSS class of the status dot of a connection.
pub fn status_class(status: Option<ConnectionStatus>) -> &'static str {
    match status {
        Some(ConnectionStatus::Connected) => "dot ok",
        Some(ConnectionStatus::Connecting | ConnectionStatus::Reconnecting { .. }) => "dot busy",
        _ => "dot off",
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
            "dot off"
        };
        let name = server.map_or_else(|| "Servers".to_owned(), |server| server.name);
        view! {
            <span class=dot aria-hidden="true"></span>
            <span class="menu-text">{name}</span>
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
                    "dot off"
                };
                let checked = (current_url.as_deref() == Some(server.url.as_str())).to_string();
                let url = server.url.clone();
                view! {
                    <li role="none">
                        <button
                            type="button"
                            role="menuitemradio"
                            aria-checked=checked
                            on:click=move |_| state.open_server(&url)
                        >
                            <span class=dot aria-hidden="true"></span>
                            <span class="item-main">{server.name}</span>
                            <span class="item-note">{account}</span>
                        </button>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <Menu id="server-menu" label button=ViewFn::from(button)>
            {items}
            <li role="separator"></li>
            <li role="none">
                <button type="button" role="menuitem" on:click=move |_| state.navigate(Route::Servers)>
                    <Icon kind=IconKind::Server />
                    <span class="item-main">"Manage servers…"</span>
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
                <select class="shard-select" aria-label="Shard" on:change=change>
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
    move || match state.route.get() {
        Route::World { .. } => {
            let world = map
                .shard
                .get()
                .and_then(|shard| shard.name().map(str::to_owned))
                .unwrap_or_else(|| "World".to_owned());
            let room = map.room.get();
            view! {
                <nav class="breadcrumb" aria-label="Breadcrumb">
                    <ol>
                        <li>
                            <button
                                type="button"
                                class="crumb"
                                aria-current=room.is_none().then_some("page")
                                data-tip="Show the whole world"
                                on:click=move |_| map.show_world()
                            >
                                <Icon kind=IconKind::Globe />
                                <span class="crumb-text">{world}</span>
                            </button>
                        </li>
                        {room
                            .map(|room| {
                                view! {
                                    <li aria-current="page">
                                        <span class="crumb">{room.to_string()}</span>
                                    </li>
                                }
                            })}
                    </ol>
                </nav>
            }
            .into_any()
        }
        Route::Servers => view! { <span class="crumb-title">"Servers"</span> }.into_any(),
        Route::Profile { .. } => view! { <span class="crumb-title">"Account"</span> }.into_any(),
        Route::Window { .. } => ().into_any(),
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
        <form class="room-search" role="search" on:submit=submit novalidate>
            <span class="search-icon"><Icon kind=IconKind::Search /></span>
            <input
                id="room-search"
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
                on:keydown=move |event: KeyboardEvent| {
                    if event.key() == "Escape" {
                        event.prevent_default();
                        query.set(String::new());
                        error.set(None);
                        map.focus_map();
                    }
                }
            />
            <kbd class="search-key" aria-hidden="true">"/"</kbd>
            <span id="room-search-error" class="search-error" role="alert">
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
            class="icon-button"
            aria-expanded=move || open().to_string()
            aria-controls="drawer"
            aria-label="Drawer"
            data-tip=tip
            data-tip-pos="bottom-end"
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
                    button=ViewFn::from(move || {
                        view! {
                            <Icon kind=IconKind::User />
                            <span class="menu-text">{button_name.clone()}</span>
                        }
                    })
                >
                    <li role="none">
                        <button
                            type="button"
                            role="menuitem"
                            on:click={
                                let slug = slug.clone();
                                move |_| state.navigate(Route::Profile { server: slug.clone() })
                            }
                        >
                            <Icon kind=IconKind::User />
                            <span class="item-main">"Profile"</span>
                        </button>
                    </li>
                    <li role="none">
                        <button
                            type="button"
                            role="menuitem"
                            on:click=move |_| limits.open.set(true)
                        >
                            <Icon kind=IconKind::Gauge />
                            <span class="item-main">"Rate limits"</span>
                        </button>
                    </li>
                    <li role="separator"></li>
                    <li role="none">
                        <button
                            type="button"
                            role="menuitem"
                            on:click=move |_| spawn_local(async move { state.sign_out().await })
                        >
                            <Icon kind=IconKind::SignOut />
                            <span class="item-main">"Sign out"</span>
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
