//! The server list and the "Add server" dialog.

use leptos::html::Dialog;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::platform::Platform;
use crate::profiles::{SEASON_URL, Server, ServerKind, server_password_key};
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::toast::{ToastKind, Toasts};

/// Every saved server.
#[component]
pub fn Servers() -> impl IntoView {
    let state = AppState::expect();
    let dialog = NodeRef::<Dialog>::new();
    let items = move || {
        state
            .profiles
            .get()
            .servers
            .into_iter()
            .map(|server| view! { <ServerItem server /> })
            .collect_view()
    };
    let open = move |_| {
        if let Some(dialog) = dialog.get_untracked() {
            let _ = dialog.show_modal();
        }
    };
    view! {
        <div class="page servers-page">
            <section aria-labelledby="servers-title">
                <header class="page-head">
                    <div>
                        <h1 id="servers-title">"Servers"</h1>
                        <p class="muted">"Choose a server to open its world map."</p>
                    </div>
                    <button
                        type="button"
                        class="primary"
                        aria-haspopup="dialog"
                        aria-controls="add-server"
                        on:click=open
                    >
                        <Icon kind=IconKind::Plus />
                        "Add server"
                    </button>
                </header>
                <ul class="server-list">{items}</ul>
            </section>
            <AddServer dialog />
        </div>
    }
}

/// One server of the list.
#[component]
fn ServerItem(server: Server) -> impl IntoView {
    let state = AppState::expect();
    let toasts = Toasts::expect();
    let (kind, icon) = match server.kind {
        ServerKind::Official => ("Official server", IconKind::Globe),
        ServerKind::Season => ("Seasonal server", IconKind::Globe),
        ServerKind::Private => ("Private server", IconKind::Server),
    };
    let account = server.user.as_ref().map_or_else(
        || "Signed out".to_owned(),
        |user| {
            format!(
                "Signed in as {}",
                user.username.as_deref().unwrap_or("(no username yet)")
            )
        },
    );
    let signed_in = server.user.is_some();
    let open_label = if signed_in { "Open" } else { "Sign in" };
    let url = server.url.clone();
    let open = move |_| state.open_server(&url);
    let removable = server.kind != ServerKind::Official;
    let remove_url = server.url.clone();
    let removed_name = server.name.clone();
    let remove = move |_| {
        let url = remove_url.clone();
        let platform = state.platform();
        let slug = state.slug(&url);
        let live = state.session.with_untracked(|session| {
            session
                .as_ref()
                .is_some_and(|session| session.server == url)
        });
        state.update_saved(|saved| saved.forget(&url, slug.as_deref()));
        state.update_profiles(|profiles| profiles.remove(&url));
        toasts.show(ToastKind::Info, format!("Removed {removed_name}"));
        spawn_local(async move {
            // Its session ends as with signing out.
            if live {
                state.sign_out().await;
            }
            let _ = platform
                .secret_delete(&crate::profiles::token_key(&url))
                .await;
            let _ = platform.secret_delete(&server_password_key(&url)).await;
            // The cache is bounded anyway; a failure leaves rooms to be evicted later.
            #[cfg(target_arch = "wasm32")]
            let _ = crate::terrain_cache::forget_server(&url).await;
        });
    };
    let name = server.name.clone();
    let live_url = server.url.clone();
    let dot = move || {
        let live = state.session.with(|session| {
            session
                .as_ref()
                .is_some_and(|session| session.server == live_url)
        });
        if live {
            super::topbar::status_class(state.status.get())
        } else {
            "dot off"
        }
    };
    view! {
        <li class="server">
            <span class="server-icon"><Icon kind=icon /></span>
            <div class="server-info">
                <strong class="server-name">
                    {server.name}
                    <span class=dot aria-hidden="true"></span>
                </strong>
                <span class="muted server-meta">
                    {kind} " · " <span class="server-url">{server.url}</span>
                </span>
                <span class="server-account" class:signed-in=signed_in>
                    <Icon kind=IconKind::User />
                    {account}
                </span>
            </div>
            <div class="server-actions">
                {removable
                    .then(|| {
                        let label = format!("Remove {name}");
                        view! {
                            <button
                                type="button"
                                class="icon-button ghost"
                                aria-label=label.clone()
                                data-tip=label
                                on:click=remove
                            >
                                <Icon kind=IconKind::Trash />
                            </button>
                        }
                    })}
                <button type="button" class="primary" on:click=open>
                    {open_label}
                    <span class="visually-hidden">{format!(" {name}")}</span>
                </button>
            </div>
        </li>
    }
}

/// The dialog adding the Season preset or a private server; a toast confirms.
#[component]
fn AddServer(
    /// The dialog element, opened by the page's "Add server" button.
    dialog: NodeRef<Dialog>,
) -> impl IntoView {
    let state = AppState::expect();
    let toasts = Toasts::expect();
    let preset = RwSignal::new("private".to_owned());
    let name = RwSignal::new(String::new());
    let url = RwSignal::new(String::new());
    let server_password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let private = move || preset.get() == "private";
    let close = move || {
        if let Some(dialog) = dialog.get_untracked() {
            dialog.close();
        }
    };
    // However the dialog closes (Escape, Cancel, success), it opens empty next time.
    let reset = move || {
        preset.set("private".to_owned());
        name.set(String::new());
        url.set(String::new());
        server_password.set(String::new());
        error.set(None);
    };
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let server = if private() {
            Server::new(name.get(), url.get(), ServerKind::Private)
        } else {
            Server::new(
                "Season".to_owned(),
                SEASON_URL.to_owned(),
                ServerKind::Season,
            )
        };
        let mut result = Err(String::new());
        state.update_profiles(|profiles| result = profiles.add(server));
        match result {
            Ok(server_url) => {
                let password = server_password.get_untracked();
                if private() && !password.is_empty() {
                    let platform = state.platform();
                    let key = server_password_key(&server_url);
                    spawn_local(async move {
                        if let Err(failure) = platform.secret_set(&key, &password).await {
                            toasts.show(
                                ToastKind::Error,
                                format!("Could not store the server password: {failure}"),
                            );
                        }
                    });
                }
                let added = state
                    .server(&server_url)
                    .map_or(server_url, |server| server.name);
                toasts.show(ToastKind::Success, format!("Added {added}"));
                close();
            }
            Err(message) => error.set(Some(message)),
        }
    };
    view! {
        <dialog
            id="add-server"
            class="dialog"
            node_ref=dialog
            aria-labelledby="add-title"
            on:close=move |_| reset()
        >
            <form on:submit=submit novalidate>
                <header class="dialog-head">
                    <h2 id="add-title">"Add server"</h2>
                    <button
                        type="button"
                        class="icon-button sm ghost"
                        aria-label="Close"
                        on:click=move |_| close()
                    >
                        <Icon kind=IconKind::Close />
                    </button>
                </header>
                <div class="field">
                    <label for="add-preset">"Server"</label>
                    <select id="add-preset" bind:value=preset>
                        <option value="private">"Private server"</option>
                        <option value="season">"Season (screeps.com)"</option>
                    </select>
                </div>
                <Show
                    when=private
                    fallback=|| {
                        view! {
                            <p class="help">
                                "screeps.com's seasonal world; sign in with your screeps.com account."
                            </p>
                        }
                    }
                >
                    <div class="field">
                        <label for="add-name">"Name"</label>
                        <input id="add-name" type="text" required autofocus bind:value=name />
                    </div>
                    <div class="field">
                        <label for="add-url">"Address"</label>
                        <input
                            id="add-url"
                            type="url"
                            inputmode="url"
                            required
                            placeholder="http://localhost:21025/"
                            bind:value=url
                        />
                    </div>
                    <div class="field">
                        <label for="add-server-password">"Server password (optional)"</label>
                        <input
                            id="add-server-password"
                            type="password"
                            autocomplete="off"
                            aria-describedby="add-server-password-help"
                            bind:value=server_password
                        />
                        <p id="add-server-password-help" class="help">
                            "Only for password-protected servers; kept in "
                            {move || {
                                if state.platform.with_value(Platform::is_tauri) {
                                    "the system keychain."
                                } else {
                                    "this browser's storage."
                                }
                            }}
                        </p>
                    </div>
                </Show>
                {move || error.get().map(|error| view! { <p class="error" role="alert">{error}</p> })}
                <footer class="dialog-actions">
                    <button type="button" on:click=move |_| close()>"Cancel"</button>
                    <button type="submit" class="primary">"Add"</button>
                </footer>
            </form>
        </dialog>
    }
}
