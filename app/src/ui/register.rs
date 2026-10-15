//! Registration on private servers and choosing a username, with live availability checks.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Auth;
use overseer::core::api::{endpoints, types};
use overseer::net::Client;

use crate::register::{Registration, confirm_error, email_error, password_error, username_error};
use crate::route::Route;
use crate::session::{self, Failure};
use crate::state::{AppState, Screen, Start};
use crate::ui::form::{Field, FormError};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::spawn_scoped;

/// Pause after typing before a value is checked with the server.
const CHECK_DELAY: Duration = Duration::from_millis(400);

/// What the server said about a username or e-mail address.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Check {
    /// Nothing to check (empty or breaking a local rule).
    Idle,
    /// Waiting for the answer.
    Checking,
    /// Free to use.
    Available,
    /// Already used by an account.
    Taken,
    /// The check failed; the server's message.
    Failed(String),
}

/// Which availability check.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckKind {
    /// `GET /api/register/check-username`.
    Username,
    /// `GET /api/register/check-email`.
    Email,
}

/// Checks `value` with the server whenever it changes and follows its local `rule`, once
/// typing pauses; the latest answer wins.
fn live_check(
    value: RwSignal<String>,
    kind: CheckKind,
    rule: fn(&str) -> Option<&'static str>,
    client: RwSignal<Option<Client>, LocalStorage>,
) -> RwSignal<Check> {
    let check = RwSignal::new(Check::Idle);
    let generation = StoredValue::new(0_u64);
    Effect::new(move |_| {
        let text = value.get().trim().to_owned();
        let current = generation.get_value() + 1;
        generation.set_value(current);
        if text.is_empty() || rule(&text).is_some() {
            check.set(Check::Idle);
            return;
        }
        check.set(Check::Checking);
        set_timeout(
            move || {
                if generation.try_get_value() != Some(current) {
                    return;
                }
                let Some(client) = client.try_get_untracked().flatten() else {
                    check.set(Check::Failed("Not connected to the server".to_owned()));
                    return;
                };
                spawn_local(async move {
                    let result = match kind {
                        CheckKind::Username => client
                            .send(&endpoints::RegisterCheckUsername { username: text })
                            .await
                            .map(drop),
                        CheckKind::Email => client
                            .send(&endpoints::RegisterCheckEmail { email: text })
                            .await
                            .map(drop),
                    };
                    if generation.try_get_value() != Some(current) {
                        return;
                    }
                    check.set(match result {
                        Ok(()) => Check::Available,
                        Err(error) => match session::message(&error) {
                            exists if exists == "exists" => Check::Taken,
                            message => Check::Failed(message),
                        },
                    });
                });
            },
            CHECK_DELAY,
        );
    });
    check
}

/// The status line of a check; a taken value is reported as the field's error instead.
fn check_text(check: &Check) -> Option<String> {
    match check {
        Check::Idle | Check::Taken => None,
        Check::Checking => Some("Checking…".to_owned()),
        Check::Available => Some("Available".to_owned()),
        Check::Failed(message) => Some(format!("Could not check: {message}")),
    }
}

/// The registration form of the private server with base URL `server`.
#[component]
pub fn Register(
    /// Server base URL.
    server: String,
) -> impl IntoView {
    let state = AppState::expect();
    let name = state
        .server(&server)
        .map_or_else(|| server.clone(), |server| server.name);
    let client = RwSignal::new_local(None::<Client>);
    let error = RwSignal::new(None::<String>);
    {
        let platform = state.platform();
        let url = server.clone();
        spawn_scoped(async move {
            match session::anonymous_client(&platform, &url, None).await {
                Ok(anonymous) => client.set(Some(anonymous)),
                Err(message) => error.set(Some(message)),
            }
        });
    }
    let username = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let submitted = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let username_check = live_check(username, CheckKind::Username, username_error, client);
    let email_check = live_check(email, CheckKind::Email, email_error, client);
    let form = move || Registration {
        username: username.get().trim().to_owned(),
        email: email.get().trim().to_owned(),
        password: password.get(),
        confirm: confirm.get(),
    };
    // A rule's message once the field has content or the form was submitted.
    let shown = move |value: RwSignal<String>, error: Option<&'static str>| {
        (submitted.get() || !value.with(String::is_empty))
            .then_some(error)
            .flatten()
            .map(str::to_owned)
    };
    let username_message = Signal::derive(move || {
        shown(username, username_error(username.get().trim()))
            .or_else(|| (username_check.get() == Check::Taken).then(|| "Already taken".to_owned()))
    });
    let email_message = Signal::derive(move || {
        shown(email, email_error(email.get().trim())).or_else(|| {
            (email_check.get() == Check::Taken).then(|| "Already registered".to_owned())
        })
    });
    let password_message = Signal::derive(move || shown(password, password_error(&password.get())));
    let confirm_message =
        Signal::derive(move || shown(confirm, confirm_error(&password.get(), &confirm.get())));
    let url = server.clone();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        submitted.set(true);
        let registration = form();
        if !registration.errors().is_empty() {
            error.set(Some("Correct the marked fields.".to_owned()));
            return;
        }
        let Some(anonymous) = client.get_untracked() else {
            error.set(Some("Not connected to the server yet.".to_owned()));
            return;
        };
        error.set(None);
        busy.set(true);
        let url = url.clone();
        let platform = state.platform();
        spawn_local(async move {
            let body = types::RegisterSubmitRequest {
                username: registration.username.clone(),
                email: registration.email,
                password: registration.password.clone(),
                ..Default::default()
            };
            if let Err(failure) = anonymous.send(&endpoints::RegisterSubmit { body }).await {
                busy.set(false);
                error.set(Some(session::message(&failure)));
                return;
            }
            let auth = Auth::Credentials {
                email: registration.username.clone(),
                password: registration.password,
            };
            match session::connect(&platform, &url, auth, None).await {
                Ok(overseer) => {
                    state.begin(&url, overseer, Start::Password(Some(registration.username)));
                }
                Err(failure) => {
                    busy.set(false);
                    let message = match failure {
                        Failure::Unauthorized => "the server rejected the new account".to_owned(),
                        Failure::ServerPassword => "incorrect server password".to_owned(),
                        Failure::Other(message) => message,
                    };
                    error.set(Some(format!(
                        "Registered, but signing in failed: {message}"
                    )));
                }
            }
        });
    };
    let sign_in_url = server;
    let shown_url = sign_in_url.clone();
    view! {
        <section class="panel narrow card" aria-labelledby="register-title">
            <header class="card-head">
                <img src="icon.svg" alt="" width="40" height="40" />
                <div>
                    <h1 id="register-title">{format!("Register on {name}")}</h1>
                    <p class="muted">{shown_url}</p>
                </div>
            </header>
            <div class="segmented tabs" role="group" aria-label="Account">
                <button
                    type="button"
                    aria-pressed="false"
                    on:click=move |_| {
                        state
                            .screen
                            .set(Screen::SignIn {
                                server: sign_in_url.clone(),
                                message: None,
                            });
                    }
                >
                    "Sign in"
                </button>
                <button type="button" aria-pressed="true">"Register"</button>
            </div>
            <form on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
                <Field
                    id="register-username"
                    label="Username"
                    autocomplete="username"
                    value=username
                    error=username_message
                    status=Signal::derive(move || check_text(&username_check.get()))
                    help="Letters, digits, _ and -; at least 3 characters."
                />
                <Field
                    id="register-email"
                    label="E-mail"
                    kind="email"
                    autocomplete="email"
                    value=email
                    error=email_message
                    status=Signal::derive(move || check_text(&email_check.get()))
                />
                <Field
                    id="register-password"
                    label="Password"
                    kind="password"
                    autocomplete="new-password"
                    value=password
                    error=password_message
                    help="8 to 1024 characters."
                />
                <Field
                    id="register-confirm"
                    label="Repeat the password"
                    kind="password"
                    autocomplete="new-password"
                    value=confirm
                    error=confirm_message
                />
                <FormError error />
                <button type="submit" class="primary" disabled=move || busy.get()>
                    {move || if busy.get() { "Registering…" } else { "Register" }}
                </button>
            </form>
            <button type="button" class="link back" on:click=move |_| state.navigate(Route::Servers)>
                <Icon kind=IconKind::ArrowLeft />
                "Choose another server"
            </button>
        </section>
    }
}

/// Choosing the username of a signed-in account that has none (Steam or GitHub sign-ups).
#[component]
pub fn SetUsername() -> impl IntoView {
    let state = AppState::expect();
    let client = RwSignal::new_local(
        state
            .session
            .get_untracked()
            .map(|session| session.overseer.client().clone()),
    );
    let username = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let submitted = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let username_check = live_check(username, CheckKind::Username, username_error, client);
    let username_message = Signal::derive(move || {
        let rule = username_error(username.get().trim());
        (submitted.get() || !username.with(String::is_empty))
            .then_some(rule)
            .flatten()
            .map(str::to_owned)
            .or_else(|| (username_check.get() == Check::Taken).then(|| "Already taken".to_owned()))
    });
    let email_message = Signal::derive(move || {
        let text = email.get();
        let text = text.trim();
        (!text.is_empty())
            .then(|| email_error(text))
            .flatten()
            .map(str::to_owned)
    });
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        submitted.set(true);
        let name = username.get_untracked().trim().to_owned();
        let address = email.get_untracked().trim().to_owned();
        if username_error(&name).is_some()
            || (!address.is_empty() && email_error(&address).is_some())
        {
            error.set(Some("Correct the marked fields.".to_owned()));
            return;
        }
        let Some(session) = state.session.get_untracked() else {
            error.set(Some("Not signed in.".to_owned()));
            return;
        };
        error.set(None);
        busy.set(true);
        spawn_local(async move {
            let body = types::RegisterSetUsernameRequest {
                username: name,
                email: (!address.is_empty()).then_some(address),
                ..Default::default()
            };
            let overseer = session.overseer;
            if let Err(failure) = overseer
                .client()
                .send(&endpoints::RegisterSetUsername { body })
                .await
            {
                busy.set(false);
                error.set(Some(session::message(&failure)));
                return;
            }
            match overseer.account().refresh().await {
                Ok(me) => {
                    state.remember_username(&session.server, me.username.clone());
                    state.screen.set(Screen::Ready);
                }
                Err(failure) => {
                    busy.set(false);
                    error.set(Some(session::describe(&failure)));
                }
            }
        });
    };
    view! {
        <section class="panel narrow card" aria-labelledby="username-title">
            <header class="card-head">
                <img src="icon.svg" alt="" width="40" height="40" />
                <h1 id="username-title">"Choose your username"</h1>
            </header>
            <p class="lead">"Your account has no username yet. Choose one to start playing; it cannot be changed later."</p>
            <form on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
                <Field
                    id="set-username"
                    label="Username"
                    autocomplete="username"
                    value=username
                    error=username_message
                    status=Signal::derive(move || check_text(&username_check.get()))
                    help="Letters, digits, _ and -; at least 3 characters."
                />
                <Field
                    id="set-username-email"
                    label="E-mail (optional)"
                    kind="email"
                    autocomplete="email"
                    value=email
                    error=email_message
                />
                <FormError error />
                <button type="submit" class="primary" disabled=move || busy.get()>
                    "Save username"
                </button>
            </form>
        </section>
    }
}
