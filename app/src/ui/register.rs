//! Registration on private servers and choosing a username, with live availability checks.

use std::time::Duration;

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::model::account::Availability;
use overseer::{Auth, Guest};

use crate::register::{Registration, confirm_error, email_error, password_error, username_error};
use crate::route::Route;
use crate::session::{self, Failure};
use crate::state::{AppState, Screen, Start};
use crate::ui::form::{Field, FormError};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::sign_in::{CARD_FORM, CARD_LEAD, CARD_SUBMIT};
use crate::ui::spawn_scoped;
use crate::ui::style;

/// Pause after typing before a value is checked with the server.
const CHECK_DELAY: Duration = Duration::from_millis(400);

/// Why a server cannot register accounts: it lacks password accounts
/// ([`Subsystem::PasswordAccounts`](overseer::model::platform::Subsystem)).
const NO_REGISTRATION: &str = "This server does not register accounts.";

/// What the server said about a username or e-mail address.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Check {
    /// Nothing to check (empty or breaking a local rule), or not yet: the server is not
    /// connected.
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
/// typing pauses and the server is connected (`api`); the latest answer wins.
fn live_check(
    value: RwSignal<String>,
    kind: CheckKind,
    rule: fn(&str) -> Option<&'static str>,
    api: RwSignal<Option<Guest>, LocalStorage>,
) -> RwSignal<Check> {
    let check = RwSignal::new(Check::Idle);
    let generation = StoredValue::new(0_u64);
    Effect::new(move |_| {
        let text = value.get().trim().to_owned();
        // Typed before the server was discovered: checked once it is.
        let connected = api.with(Option::is_some);
        let current = generation.get_value() + 1;
        generation.set_value(current);
        if text.is_empty() || rule(&text).is_some() || !connected {
            check.set(Check::Idle);
            return;
        }
        check.set(Check::Checking);
        set_timeout(
            move || {
                if generation.try_get_value() != Some(current) {
                    return;
                }
                let Some(guest) = api.try_get_untracked().flatten() else {
                    return;
                };
                spawn_local(async move {
                    let result = match kind {
                        CheckKind::Username => guest.username_available(&text).await,
                        CheckKind::Email => guest.email_available(&text).await,
                    };
                    if generation.try_get_value() != Some(current) {
                        return;
                    }
                    check.set(match result {
                        Ok(Availability::Available) => Check::Available,
                        Ok(Availability::Taken) => Check::Taken,
                        Err(error) => Check::Failed(session::error_message(&error)),
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
    let saved = state.server(&server);
    let server_kind = saved.as_ref().and_then(|server| server.kind.forced());
    let name = saved.map_or_else(|| server.clone(), |server| server.name);
    let api = RwSignal::new_local(None::<Guest>);
    let error = RwSignal::new(None::<String>);
    // The server has no password accounts: nothing to submit.
    let unsupported = RwSignal::new(false);
    {
        let platform = state.platform();
        let url = server.clone();
        spawn_scoped(async move {
            match session::guest(&platform, &url, server_kind, None).await {
                Ok(anonymous) => {
                    if !anonymous.can_register() {
                        unsupported.set(true);
                        error.set(Some(NO_REGISTRATION.to_owned()));
                    }
                    api.set(Some(anonymous));
                }
                Err(failure) => error.set(Some(failure.to_string())),
            }
        });
    }
    let username = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let submitted = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let username_check = live_check(username, CheckKind::Username, username_error, api);
    let email_check = live_check(email, CheckKind::Email, email_error, api);
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
        if registration.has_errors() {
            error.set(Some("Correct the marked fields.".to_owned()));
            return;
        }
        let Some(anonymous) = api.get_untracked() else {
            error.set(Some("Not connected to the server yet.".to_owned()));
            return;
        };
        error.set(None);
        busy.set(true);
        let url = url.clone();
        let platform = state.platform();
        spawn_local(async move {
            if let Err(failure) = anonymous.register(registration.account()).await {
                busy.set(false);
                error.set(Some(if anonymous.can_register() {
                    session::error_message(&failure)
                } else {
                    NO_REGISTRATION.to_owned()
                }));
                return;
            }
            let auth = Auth::Credentials {
                email: registration.username.clone(),
                password: registration.password,
            };
            match session::connect(&platform, &url, server_kind, auth, None).await {
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
        <section class=style::CARD aria-labelledby="register-title">
            <header class=style::CARD_HEAD>
                <img src="icon.svg" alt="" width="40" height="40" />
                <div>
                    <h1 id="register-title" class=style::CARD_TITLE>{format!("Register on {name}")}</h1>
                    <p class=CARD_LEAD>{shown_url}</p>
                </div>
            </header>
            <div class=style::SEGMENTED_TABS role="group" aria-label="Account">
                <button
                    type="button"
                    class=style::TAB_SEGMENT
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
                <button type="button" class=style::TAB_SEGMENT aria-pressed="true">"Register"</button>
            </div>
            <form class=CARD_FORM on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
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
                <button
                    type="submit"
                    class=CARD_SUBMIT
                    disabled=move || busy.get() || unsupported.get()
                >
                    {move || if busy.get() { "Registering…" } else { "Register" }}
                </button>
            </form>
            <button
                type="button"
                class=style::BACK_BUTTON
                on:click=move |_| state.navigate(Route::Servers)
            >
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
    let api = RwSignal::new_local(
        state
            .session
            .get_untracked()
            .map(|session| session.overseer.guest()),
    );
    let username = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let submitted = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let username_check = live_check(username, CheckKind::Username, username_error, api);
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
            let overseer = session.overseer;
            let email = (!address.is_empty()).then_some(address.as_str());
            if let Err(failure) = overseer.account().set_username(&name, email).await {
                busy.set(false);
                error.set(Some(session::error_message(&failure)));
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
        <section class=style::CARD aria-labelledby="username-title">
            <header class=style::CARD_HEAD>
                <img src="icon.svg" alt="" width="40" height="40" />
                <h1 id="username-title" class=style::CARD_TITLE>"Choose your username"</h1>
            </header>
            <p class="mb-3 text-muted">"Your account has no username yet. Choose one to start playing; it cannot be changed later."</p>
            <form class=CARD_FORM on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
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
                <button type="submit" class=CARD_SUBMIT disabled=move || busy.get()>
                    "Save username"
                </button>
            </form>
        </section>
    }
}
