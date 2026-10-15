//! The server list and the "Add server" form.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::platform::Platform;
use crate::profiles::{SEASON_URL, Server, ServerKind, server_password_key};
use crate::state::AppState;

/// Every saved server.
#[component]
pub fn Servers() -> impl IntoView {
    let state = AppState::expect();
    let items = move || {
        state
            .profiles
            .get()
            .servers
            .into_iter()
            .map(|server| view! { <ServerItem server /> })
            .collect_view()
    };
    view! {
        <div class="page">
            <section aria-labelledby="servers-title">
                <h1 id="servers-title">"Servers"</h1>
                <p class="muted">"Choose a server to open its world map."</p>
                <ul class="server-list">{items}</ul>
            </section>
            <AddServer />
        </div>
    }
}

/// One server of the list.
#[component]
fn ServerItem(server: Server) -> impl IntoView {
    let state = AppState::expect();
    let kind = match server.kind {
        ServerKind::Official => "Official server",
        ServerKind::Season => "Seasonal server",
        ServerKind::Private => "Private server",
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
    let open_label = if server.user.is_some() {
        "Open"
    } else {
        "Sign in"
    };
    let url = server.url.clone();
    let open = move |_| state.open_server(&url);
    let removable = server.kind != ServerKind::Official;
    let remove_url = server.url.clone();
    let remove = move |_| {
        let url = remove_url.clone();
        let platform = state.platform();
        let slug = state.slug(&url);
        state.update_saved(|saved| saved.forget(&url, slug.as_deref()));
        state.update_profiles(|profiles| profiles.remove(&url));
        spawn_local(async move {
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
            <div class="server-info">
                <strong>
                    <span class=dot aria-hidden="true"></span>
                    {server.name}
                </strong>
                <span class="muted">{kind} " · " {server.url}</span>
                <span>{account}</span>
            </div>
            <div class="actions">
                <button type="button" class="primary" on:click=open>
                    {open_label}
                    <span class="visually-hidden">{format!(" {name}")}</span>
                </button>
                {removable
                    .then(|| {
                        view! {
                            <button type="button" on:click=remove>
                                "Remove"
                                <span class="visually-hidden">{format!(" {name}")}</span>
                            </button>
                        }
                    })}
            </div>
        </li>
    }
}

/// Adds the Season preset or a private server.
#[component]
fn AddServer() -> impl IntoView {
    let state = AppState::expect();
    let preset = RwSignal::new("private".to_owned());
    let name = RwSignal::new(String::new());
    let url = RwSignal::new(String::new());
    let server_password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let added = RwSignal::new(None::<String>);
    let private = move || preset.get() == "private";
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
                error.set(None);
                added.set(Some(format!("Added {server_url}")));
                let password = server_password.get();
                if private() && !password.is_empty() {
                    let platform = state.platform();
                    spawn_local(async move {
                        if let Err(failure) = platform
                            .secret_set(&server_password_key(&server_url), &password)
                            .await
                        {
                            state.banner.set(Some(format!(
                                "Could not store the server password: {failure}"
                            )));
                        }
                    });
                }
                name.set(String::new());
                url.set(String::new());
                server_password.set(String::new());
            }
            Err(message) => {
                added.set(None);
                error.set(Some(message));
            }
        }
    };
    view! {
        <section class="panel compact" aria-labelledby="add-title">
            <h2 id="add-title">"Add server"</h2>
            <form on:submit=submit novalidate>
                <div class="field">
                    <label for="add-preset">"Server"</label>
                    <select id="add-preset" bind:value=preset>
                        <option value="private">"Private server"</option>
                        <option value="season">"Season (screeps.com)"</option>
                    </select>
                </div>
                <Show when=private>
                    <div class="field">
                        <label for="add-name">"Name"</label>
                        <input id="add-name" type="text" required bind:value=name />
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
                <p class="success" role="status">{move || added.get()}</p>
                <button type="submit" class="primary">"Add"</button>
            </form>
        </section>
    }
}
