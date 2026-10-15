//! Sign-in: API token or e-mail and password on screeps.com, name and password (and the
//! server password when asked) on private servers.

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Auth;
use overseer::core::Token;

use crate::platform::Platform;
use crate::profiles::{Server, server_password_key};
use crate::route::Route;
use crate::session::{self, Failure};
use crate::state::{AppState, Screen, Start};
use crate::ui::form::FormError;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::open_external;

/// screeps.com registration page (linked by the official client, `app2/main.js`).
const REGISTER_URL: &str = "https://screeps.com/a/#!/register";
/// screeps.com password recovery form (route `/register/ask-recover` of the official client).
const RECOVER_URL: &str = "https://screeps.com/a/#!/register/ask-recover";
/// screeps.com API token page (route `/account/auth-tokens` of the official client).
const AUTH_TOKENS_URL: &str = "https://screeps.com/a/#!/account/auth-tokens";

/// The sign-in screen of the server with base URL `server`.
#[component]
#[expect(
    clippy::needless_pass_by_value,
    reason = "component properties are owned; the URL only selects the server"
)]
pub fn SignIn(
    /// Server base URL.
    server: String,
    /// Shown above the form (expired session, …).
    message: Option<String>,
) -> impl IntoView {
    let state = AppState::expect();
    let Some(server) = state.server(&server) else {
        return view! {
            <section class="panel narrow state-card">
                <span class="state-icon warn"><Icon kind=IconKind::Alert /></span>
                <p role="alert">"This server is no longer in the list."</p>
                <button type="button" on:click=move |_| state.navigate(Route::Servers)>
                    "Back to the servers"
                </button>
            </section>
        }
        .into_any();
    };
    let title = format!("Sign in to {}", server.name);
    let form = if server.kind.uses_screeps_accounts() {
        view! { <ScreepsSignIn server=server.clone() /> }.into_any()
    } else {
        view! { <PrivateSignIn server=server.clone() /> }.into_any()
    };
    view! {
        <section class="panel narrow card" aria-labelledby="sign-in-title">
            <header class="card-head">
                <img src="icon.svg" alt="" width="40" height="40" />
                <div>
                    <h1 id="sign-in-title">{title}</h1>
                    <p class="muted">{server.url.clone()}</p>
                </div>
            </header>
            {message.map(|message| view! { <p class="notice" role="alert">{message}</p> })}
            {form}
            <button type="button" class="link back" on:click=move |_| state.navigate(Route::Servers)>
                <Icon kind=IconKind::ArrowLeft />
                "Choose another server"
            </button>
        </section>
    }
    .into_any()
}

/// Token or e-mail sign-in with screeps.com accounts.
#[component]
fn ScreepsSignIn(server: Server) -> impl IntoView {
    let state = AppState::expect();
    let use_token = RwSignal::new(true);
    let token = RwSignal::new(String::new());
    let email = RwSignal::new(server.login.clone().unwrap_or_default());
    let password = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let url = server.url.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let (auth, login) = if use_token.get_untracked() {
            let token = token.get_untracked().trim().to_owned();
            if token.is_empty() {
                error.set(Some("Enter your API token".to_owned()));
                return;
            }
            (Auth::Token(Token::new(token)), None)
        } else {
            let email = email.get_untracked().trim().to_owned();
            if email.is_empty() || password.with_untracked(String::is_empty) {
                error.set(Some("Enter your e-mail and password".to_owned()));
                return;
            }
            let auth = Auth::Credentials {
                email: email.clone(),
                password: password.get_untracked(),
            };
            (auth, Some(email))
        };
        sign_in(
            state,
            Attempt {
                url: url.clone(),
                auth,
                login,
                server_password: None,
            },
            Feedback {
                busy,
                error,
                needs_server_password: None,
            },
        );
    };
    let store = move || {
        if state.platform.with_value(Platform::is_tauri) {
            "the system keychain"
        } else {
            "this browser's storage"
        }
    };
    view! {
        <div class="segmented tabs" role="group" aria-label="Sign-in method">
            <button type="button" aria-pressed=move || use_token.get().to_string() on:click=move |_| use_token.set(true)>
                "API token"
            </button>
            <button type="button" aria-pressed=move || (!use_token.get()).to_string() on:click=move |_| use_token.set(false)>
                "E-mail and password"
            </button>
        </div>
        <form on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
            <Show
                when=move || use_token.get()
                fallback=move || {
                    view! {
                        <div class="field">
                            <label for="sign-in-email">"E-mail"</label>
                            <input id="sign-in-email" type="email" autocomplete="username" required bind:value=email />
                        </div>
                        <div class="field">
                            <label for="sign-in-password">"Password"</label>
                            <input id="sign-in-password" type="password" autocomplete="current-password" required bind:value=password />
                        </div>
                    }
                }
            >
                <div class="field">
                    <label for="sign-in-token">"API token"</label>
                    <input
                        id="sign-in-token"
                        type="password"
                        autocomplete="off"
                        spellcheck="false"
                        required
                        aria-describedby="sign-in-token-help"
                        bind:value=token
                    />
                    <p id="sign-in-token-help" class="help">
                        "Create a token with full access on your screeps.com account page. It is kept in "
                        {store} "."
                    </p>
                    <button type="button" class="link" on:click=move |_| open_external(AUTH_TOKENS_URL)>
                        "Open the API token page"
                    </button>
                </div>
            </Show>
            <FormError error />
            <button type="submit" class="primary" disabled=move || busy.get()>
                {move || if busy.get() { "Signing in…" } else { "Sign in" }}
            </button>
        </form>
        <p class="links">
            <button type="button" class="link" on:click=move |_| open_external(REGISTER_URL)>
                "Register on screeps.com"
            </button>
            <button type="button" class="link" on:click=move |_| open_external(RECOVER_URL)>
                "Forgot your password?"
            </button>
        </p>
    }
}

/// Name and password sign-in on a private server, with its "Register" tab.
#[component]
fn PrivateSignIn(server: Server) -> impl IntoView {
    let state = AppState::expect();
    let login = RwSignal::new(server.login.clone().unwrap_or_default());
    let password = RwSignal::new(String::new());
    let server_password = RwSignal::new(String::new());
    let needs_server_password = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let url = server.url.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let name = login.get_untracked().trim().to_owned();
        if name.is_empty() || password.with_untracked(String::is_empty) {
            error.set(Some(
                "Enter your username or e-mail and your password".to_owned(),
            ));
            return;
        }
        let entered = server_password.get_untracked();
        sign_in(
            state,
            Attempt {
                url: url.clone(),
                auth: Auth::Credentials {
                    email: name.clone(),
                    password: password.get_untracked(),
                },
                login: Some(name),
                server_password: (needs_server_password.get_untracked() && !entered.is_empty())
                    .then_some(entered),
            },
            Feedback {
                busy,
                error,
                needs_server_password: Some(needs_server_password),
            },
        );
    };
    let register_url = server.url.clone();
    view! {
        <div class="segmented tabs" role="group" aria-label="Account">
            <button type="button" aria-pressed="true">"Sign in"</button>
            <button
                type="button"
                aria-pressed="false"
                on:click=move |_| state.screen.set(Screen::Register { server: register_url.clone() })
            >
                "Register"
            </button>
        </div>
        <form on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
            <div class="field">
                <label for="sign-in-login">"Username or e-mail"</label>
                <input id="sign-in-login" type="text" autocomplete="username" required bind:value=login />
            </div>
            <div class="field">
                <label for="sign-in-password">"Password"</label>
                <input id="sign-in-password" type="password" autocomplete="current-password" required bind:value=password />
            </div>
            <Show when=move || needs_server_password.get()>
                <div class="field">
                    <label for="sign-in-server-password">"Server password"</label>
                    <input
                        id="sign-in-server-password"
                        type="password"
                        autocomplete="off"
                        aria-describedby="sign-in-server-password-help"
                        bind:value=server_password
                    />
                    <p id="sign-in-server-password-help" class="help">
                        "This server is password-protected; ask its operator for the password."
                    </p>
                </div>
            </Show>
            <FormError error />
            <button type="submit" class="primary" disabled=move || busy.get()>
                {move || if busy.get() { "Signing in…" } else { "Sign in" }}
            </button>
        </form>
    }
}

/// A sign-in to try.
struct Attempt {
    /// Server base URL.
    url: String,
    /// Credentials or token.
    auth: Auth,
    /// The name to remember for the next sign-in.
    login: Option<String>,
    /// A newly entered server password, stored after success.
    server_password: Option<String>,
}

/// Where a sign-in reports progress.
#[derive(Clone, Copy)]
struct Feedback {
    busy: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    /// Shows the server password field (private servers).
    needs_server_password: Option<RwSignal<bool>>,
}

/// Connects with `attempt` and begins the session, or reports why not.
fn sign_in(state: AppState, attempt: Attempt, feedback: Feedback) {
    let platform = state.platform();
    feedback.busy.set(true);
    feedback.error.set(None);
    let token_sign_in = matches!(attempt.auth, Auth::Token(_));
    spawn_local(async move {
        state.disconnect().await;
        let result = session::connect(
            &platform,
            &attempt.url,
            attempt.auth,
            attempt.server_password.clone(),
        )
        .await;
        feedback.busy.set(false);
        match result {
            Ok(overseer) => {
                if let Some(password) = attempt.server_password
                    && let Err(error) = platform
                        .secret_set(&server_password_key(&attempt.url), &password)
                        .await
                {
                    state.banner.set(Some(format!(
                        "Could not store the server password: {error}"
                    )));
                }
                let start = if token_sign_in {
                    Start::ApiToken
                } else {
                    Start::Password(attempt.login)
                };
                state.begin(&attempt.url, overseer, start);
            }
            Err(Failure::Unauthorized) => feedback.error.set(Some(
                if token_sign_in {
                    "The token was rejected."
                } else {
                    "Wrong name or password."
                }
                .to_owned(),
            )),
            Err(Failure::ServerPassword) => {
                if let Some(needs) = feedback.needs_server_password {
                    needs.set(true);
                }
                feedback
                    .error
                    .set(Some("incorrect server password".to_owned()));
            }
            Err(Failure::Other(message)) => feedback.error.set(Some(message)),
        }
    });
}
