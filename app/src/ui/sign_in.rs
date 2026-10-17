//! Sign-in, as the server's subsystems allow ([`Methods`]): an API token where the server has
//! API tokens and a website to create them (screeps.com), an e-mail address (or, where the
//! server takes one, a username) and password where it has password sign-in, and the server
//! password when the server asks for one. Registration is the "Register" tab where the server
//! registers accounts through its API, and the website's page where it has one. The server is
//! discovered without an account first ([`session::anonymous_api`]).

use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Auth;
use overseer::core::Token;
use overseer::model::platform::{ServerKind, Subsystem, Subsystems};
use overseer::net::Api;
use overseer::net::api::{AccountWebsite, PasswordSignIn as _};

use crate::platform::Platform;
use crate::profiles::{Server, server_password_key};
use crate::rate_limits::page_host;
use crate::route::Route;
use crate::session::{self, Failure};
use crate::state::{AppState, Screen, Start};
use crate::ui::form::FormError;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::open_external;
use crate::ui::style::{self, concatcp};

/// A form in a card ([`style::CARD`]): [`style::FORM`] without its buttons keeping their width,
/// so that [`CARD_SUBMIT`] spans the card.
pub(super) const CARD_FORM: &str = "flex flex-col gap-3";
/// The submit button of a [`CARD_FORM`]: as wide as the card.
pub(super) const CARD_SUBMIT: &str = concatcp!(style::BUTTON_PRIMARY, " self-stretch mt-2");
/// A card's head text under its title (the server's address).
pub(super) const CARD_LEAD: &str = "text-sm text-muted wrap-anywhere";
/// A link button on a line of its own in a field, centred as wide as the field.
const FIELD_LINK: &str = concatcp!(style::LINK_BUTTON, " justify-center");

/// What the sign-in screen offers on a server, from its subsystems.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Methods {
    /// API-token sign-in, with the website's page creating tokens: where the server has API
    /// tokens and an account website (the app creates no token itself).
    token_page: Option<&'static str>,
    /// Password sign-in, and whether a username signs in too in place of the e-mail address.
    password: Option<bool>,
    /// The "Register" tab: the server registers accounts through its API.
    register_form: bool,
    /// The account website's pages: registration and password recovery.
    website: Option<AccountWebsite>,
}

impl Methods {
    /// What a server offering `subsystems` allows: password sign-in taking a username or not
    /// (`accepts_username`, `None` without password sign-in), and its account `website`.
    fn new(
        subsystems: Subsystems,
        accepts_username: Option<bool>,
        website: Option<AccountWebsite>,
    ) -> Self {
        let website = website.filter(|_| subsystems.contains(Subsystem::AccountWebsite));
        Self {
            token_page: website
                .filter(|_| subsystems.contains(Subsystem::ApiTokens))
                .map(|website| website.api_tokens),
            password: accepts_username.filter(|_| subsystems.contains(Subsystem::PasswordSignIn)),
            register_form: subsystems.contains(Subsystem::PasswordAccounts),
            website,
        }
    }

    /// What `api`'s server allows.
    fn of(api: &Api) -> Self {
        Self::new(
            api.subsystems(),
            api.password_sign_in()
                .map(|sign_in| sign_in.accepts_username()),
            api.account_website(),
        )
    }
}

/// Where the anonymous discovery of the server stands.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Discovery {
    /// Under way.
    Pending,
    /// The server wants a (correct) server password first.
    NeedsServerPassword,
    /// It failed, with the message to show.
    Failed(String),
    /// Discovered: what the server allows.
    Ready(Methods),
}

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
            <section class=concatcp!(style::PANEL_NARROW, " ", style::STATE_CARD)>
                <span class=style::STATE_ICON_WARN>
                    <Icon kind=IconKind::Alert class=style::STATE_ICON_SVG />
                </span>
                <p class=style::STATE_TEXT role="alert">"This server is no longer in the list."</p>
                <button
                    type="button"
                    class=style::BUTTON
                    on:click=move |_| state.navigate(Route::Servers)
                >
                    "Back to the servers"
                </button>
            </section>
        }
        .into_any();
    };
    let title = format!("Sign in to {}", server.name);
    let lead = server.url.clone();
    let discovery = RwSignal::new(Discovery::Pending);
    // The server password entered before discovery could succeed, stored after a sign-in.
    let entered = RwSignal::new(None::<String>);
    let url = server.url.clone();
    let server_kind = server.kind.forced();
    let discover = Callback::new(move |server_password: Option<String>| {
        discovery.set(Discovery::Pending);
        let platform = state.platform();
        let url = url.clone();
        spawn_local(async move {
            let discovered =
                session::anonymous_api(&platform, &url, server_kind, server_password).await;
            let found = match discovered {
                Ok(api) => Discovery::Ready(Methods::of(&api)),
                Err(Failure::ServerPassword) => Discovery::NeedsServerPassword,
                Err(failure) => Discovery::Failed(failure.to_string()),
            };
            // The screen may be gone meanwhile.
            let _ = discovery.try_set(found);
        });
    });
    discover.run(None);
    let retry = move |_| discover.run(entered.get_untracked());
    let form = move || match discovery.get() {
        Discovery::Pending => view! {
            <p class=style::HELP role="status">"Connecting to the server…"</p>
        }
        .into_any(),
        Discovery::Failed(message) => view! {
            <p class=style::NOTICE role="alert">{message}</p>
            <button type="button" class=CARD_SUBMIT on:click=retry>"Try again"</button>
        }
        .into_any(),
        Discovery::NeedsServerPassword => view! {
            <ServerPasswordPrompt
                retry=entered.get_untracked().is_some()
                submit=Callback::new(move |password: String| {
                    entered.set(Some(password.clone()));
                    discover.run(Some(password));
                })
            />
        }
        .into_any(),
        Discovery::Ready(methods) => view! {
            <SignInForm server=server.clone() methods server_password=entered.get_untracked() />
        }
        .into_any(),
    };
    view! {
        <section class=style::CARD aria-labelledby="sign-in-title">
            <header class=style::CARD_HEAD>
                <img src="icon.svg" alt="" width="40" height="40" />
                <div>
                    <h1 id="sign-in-title" class=style::CARD_TITLE>{title}</h1>
                    <p class=CARD_LEAD>{lead}</p>
                </div>
            </header>
            {message.map(|message| view! { <p class=style::NOTICE role="alert">{message}</p> })}
            {form}
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
    .into_any()
}

/// Asks for the server password of a password-protected server before it can be discovered.
#[component]
fn ServerPasswordPrompt(
    /// A password was entered already and the server refused it.
    retry: bool,
    /// Called with the entered password.
    submit: Callback<String>,
) -> impl IntoView {
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(retry.then(|| "incorrect server password".to_owned()));
    let on_submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let entered = password.get_untracked();
        if entered.is_empty() {
            error.set(Some("Enter the server password".to_owned()));
            return;
        }
        submit.run(entered);
    };
    view! {
        <form class=CARD_FORM on:submit=on_submit novalidate>
            <div class=style::FIELD>
                <label for="sign-in-server-password" class=style::LABEL>"Server password"</label>
                <input
                    id="sign-in-server-password"
                    class=style::INPUT
                    type="password"
                    autocomplete="off"
                    aria-describedby="sign-in-server-password-help"
                    bind:value=password
                />
                <p id="sign-in-server-password-help" class=style::HELP>
                    "This server is password-protected; ask its operator for the password."
                </p>
            </div>
            <FormError error />
            <button type="submit" class=CARD_SUBMIT>"Continue"</button>
        </form>
    }
}

/// The sign-in form of `server` offering `methods`; `server_password` was entered before
/// discovery and is stored after a successful sign-in.
#[component]
fn SignInForm(server: Server, methods: Methods, server_password: Option<String>) -> impl IntoView {
    let state = AppState::expect();
    let use_token = RwSignal::new(methods.token_page.is_some());
    let token = RwSignal::new(String::new());
    let login = RwSignal::new(server.login.clone().unwrap_or_default());
    let password = RwSignal::new(String::new());
    let asked_password = RwSignal::new(String::new());
    let needs_server_password = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let accepts_username = methods.password.unwrap_or(false);
    let url = server.url.clone();
    let server_kind = server.kind.forced();
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let (auth, remembered) = if use_token.get_untracked() {
            let token = token.get_untracked().trim().to_owned();
            if token.is_empty() {
                error.set(Some("Enter your API token".to_owned()));
                return;
            }
            (Auth::Token(Token::new(token)), None)
        } else {
            let name = login.get_untracked().trim().to_owned();
            if name.is_empty() || password.with_untracked(String::is_empty) {
                error.set(Some(
                    if accepts_username {
                        "Enter your username or e-mail and your password"
                    } else {
                        "Enter your e-mail and password"
                    }
                    .to_owned(),
                ));
                return;
            }
            let auth = Auth::Credentials {
                email: name.clone(),
                password: password.get_untracked(),
            };
            (auth, Some(name))
        };
        let asked = asked_password.get_untracked();
        let server_password = if needs_server_password.get_untracked() && !asked.is_empty() {
            Some(asked)
        } else {
            server_password.clone()
        };
        sign_in(
            state,
            Attempt {
                url: url.clone(),
                server_kind,
                auth,
                login: remembered,
                server_password,
            },
            Feedback {
                busy,
                error,
                needs_server_password,
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
    let (login_label, login_type, password_tab) = if accepts_username {
        ("Username or e-mail", "text", "Username and password")
    } else {
        ("E-mail", "email", "E-mail and password")
    };
    let method_tabs = (methods.token_page.is_some() && methods.password.is_some()).then(|| {
        view! {
            <div class=style::SEGMENTED_TABS role="group" aria-label="Sign-in method">
                <button
                    type="button"
                    class=style::TAB_SEGMENT
                    aria-pressed=move || use_token.get().to_string()
                    on:click=move |_| use_token.set(true)
                >
                    "API token"
                </button>
                <button
                    type="button"
                    class=style::TAB_SEGMENT
                    aria-pressed=move || (!use_token.get()).to_string()
                    on:click=move |_| use_token.set(false)
                >
                    {password_tab}
                </button>
            </div>
        }
    });
    let register_url = server.url.clone();
    let account_tabs = methods.register_form.then(|| {
        view! {
            <div class=style::SEGMENTED_TABS role="group" aria-label="Account">
                <button type="button" class=style::TAB_SEGMENT aria-pressed="true">"Sign in"</button>
                <button
                    type="button"
                    class=style::TAB_SEGMENT
                    aria-pressed="false"
                    on:click=move |_| state.screen.set(Screen::Register { server: register_url.clone() })
                >
                    "Register"
                </button>
            </div>
        }
    });
    let token_page = methods.token_page;
    let token_help = token_page.map_or_else(String::new, |page| {
        format!(
            "Create a token with full access on your {} account page. ",
            page_host(page)
        )
    });
    let token_field = move || {
        view! {
            <div class=style::FIELD>
                <label for="sign-in-token" class=style::LABEL>"API token"</label>
                <input
                    id="sign-in-token"
                    class=style::INPUT
                    type="password"
                    autocomplete="off"
                    spellcheck="false"
                    required
                    aria-describedby="sign-in-token-help"
                    bind:value=token
                />
                <p id="sign-in-token-help" class=style::HELP>
                    {token_help.clone()}
                    "It is kept in "
                    {store} "."
                </p>
                {token_page.map(|page| view! {
                    <button type="button" class=FIELD_LINK on:click=move |_| open_external(page)>
                        "Open the API token page"
                    </button>
                })}
            </div>
        }
    };
    let password_fields = move || {
        view! {
            <div class=style::FIELD>
                <label for="sign-in-login" class=style::LABEL>{login_label}</label>
                <input
                    id="sign-in-login"
                    class=style::INPUT
                    type=login_type
                    autocomplete="username"
                    required
                    bind:value=login
                />
            </div>
            <div class=style::FIELD>
                <label for="sign-in-password" class=style::LABEL>"Password"</label>
                <input
                    id="sign-in-password"
                    class=style::INPUT
                    type="password"
                    autocomplete="current-password"
                    required
                    bind:value=password
                />
            </div>
        }
    };
    let has_method = methods.token_page.is_some() || methods.password.is_some();
    let form = has_method.then(|| {
        view! {
            <form class=CARD_FORM on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
                {move || if use_token.get() {
                    token_field().into_any()
                } else {
                    password_fields().into_any()
                }}
                <Show when=move || needs_server_password.get()>
                    <div class=style::FIELD>
                        <label for="sign-in-asked-password" class=style::LABEL>"Server password"</label>
                        <input
                            id="sign-in-asked-password"
                            class=style::INPUT
                            type="password"
                            autocomplete="off"
                            aria-describedby="sign-in-asked-password-help"
                            bind:value=asked_password
                        />
                        <p id="sign-in-asked-password-help" class=style::HELP>
                            "This server is password-protected; ask its operator for the password."
                        </p>
                    </div>
                </Show>
                <FormError error />
                <button type="submit" class=CARD_SUBMIT disabled=move || busy.get()>
                    {move || if busy.get() { "Signing in…" } else { "Sign in" }}
                </button>
            </form>
        }
    });
    let none = (!has_method).then(|| {
        view! {
            <p class=style::NOTICE role="alert">
                "This server offers no way to sign in that Overseer supports."
            </p>
        }
    });
    let links = methods.website.map(|website| {
        let host = page_host(website.register);
        view! {
            <p class="flex flex-wrap gap-4 mt-4 text-sm">
                <button
                    type="button"
                    class=style::LINK_BUTTON
                    on:click=move |_| open_external(website.register)
                >
                    {format!("Register on {host}")}
                </button>
                <button
                    type="button"
                    class=style::LINK_BUTTON
                    on:click=move |_| open_external(website.recover_password)
                >
                    "Forgot your password?"
                </button>
            </p>
        }
    });
    view! {
        {account_tabs}
        {method_tabs}
        {form}
        {none}
        {links}
    }
}

/// A sign-in to try.
struct Attempt {
    /// Server base URL.
    url: String,
    /// The contract to force ([`ServerKind::forced`](crate::profiles::ServerKind::forced)).
    server_kind: Option<ServerKind>,
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
    /// Shows the server password field.
    needs_server_password: RwSignal<bool>,
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
            attempt.server_kind,
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
                feedback.needs_server_password.set(true);
                feedback
                    .error
                    .set(Some("incorrect server password".to_owned()));
            }
            Err(Failure::Other(message)) => feedback.error.set(Some(message)),
        }
    });
}

#[cfg(test)]
mod tests {
    use overseer::model::platform::{Plugins, ServerKind, Subsystems};
    use overseer::net::api::AccountWebsite;

    use super::Methods;

    /// screeps.com's account website.
    const WEBSITE: AccountWebsite = AccountWebsite {
        register: "https://screeps.com/a/#!/register",
        recover_password: "https://screeps.com/a/#!/register/ask-recover",
        api_tokens: "https://screeps.com/a/#!/account/auth-tokens",
    };

    /// The methods of a server of `kind` with `plugins`, as discovery reports them.
    fn methods(kind: ServerKind, plugins: Plugins, accepts_username: bool) -> Methods {
        let subsystems = Subsystems::of(kind, plugins);
        let website = (kind == ServerKind::Official).then_some(WEBSITE);
        Methods::new(subsystems, Some(accepts_username), website)
    }

    #[test]
    fn each_server_offers_what_its_subsystems_allow() {
        // screeps.com: API token or e-mail sign-in, registration and recovery on its website.
        assert_eq!(
            methods(ServerKind::Official, Plugins::NONE, false),
            Methods {
                token_page: Some(WEBSITE.api_tokens),
                password: Some(false),
                register_form: false,
                website: Some(WEBSITE),
            }
        );
        // `OpenScreeps`: username or e-mail sign-in and the "Register" tab.
        let private = Methods {
            token_page: None,
            password: Some(true),
            register_form: true,
            website: None,
        };
        assert_eq!(
            methods(ServerKind::OpenScreeps, Plugins::NONE, true),
            private
        );
        // A community server with screepsmod-auth: the same, without a token page to create
        // its API tokens on.
        let auth = Plugins {
            screepsmod_auth: true,
            screepsmod_admin_utils: true,
            room_history: false,
        };
        assert_eq!(methods(ServerKind::Community, auth, true), private);
        // The stock community server: no sign-in Overseer supports.
        assert_eq!(
            Methods::new(Subsystems::NONE, None, None),
            Methods {
                token_page: None,
                password: None,
                register_form: false,
                website: None,
            }
        );
        // A website the subsystems do not list is not offered.
        assert_eq!(
            Methods::new(Subsystems::NONE, None, Some(WEBSITE)).website,
            None
        );
    }
}
