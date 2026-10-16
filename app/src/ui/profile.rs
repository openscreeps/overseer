//! The signed-in account: badge, levels, CPU, credits, world status and start room; edits of
//! the notification preferences, e-mail and password; sign-out.

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Overseer;
use overseer::core::api::{endpoints, types};
use overseer::core::socket::{UserCpu, UserCpuCpu};

use crate::levels::{self, LevelProgress};
use crate::register::{confirm_error, email_error, password_error};
use crate::route::{Route, Shard, parse_room};
use crate::session;
use crate::state::AppState;
use crate::ui::form::{Field, FormError, FormStatus};
use crate::ui::icon::{Icon, IconKind};
use crate::ui::spawn_scoped;
use crate::ui::style::{self, concatcp};

/// Choices of the notification interval, in minutes (as the official client offers them).
const INTERVALS: [(u64, &str); 9] = [
    (5, "5 minutes"),
    (10, "10 minutes"),
    (30, "30 minutes"),
    (60, "1 hour"),
    (180, "3 hours"),
    (360, "6 hours"),
    (720, "12 hours"),
    (1440, "24 hours"),
    (4320, "3 days"),
];
/// Choices of the script error notification interval, in minutes (100000 is "never").
const ERROR_INTERVALS: [(u64, &str); 10] = [
    (0, "Immediately"),
    (10, "Every 10 minutes"),
    (30, "Every 30 minutes"),
    (60, "Every hour"),
    (180, "Every 3 hours"),
    (360, "Every 6 hours"),
    (720, "Every 12 hours"),
    (1440, "Every 24 hours"),
    (4320, "Every 3 days"),
    (100_000, "Never"),
];
/// The official client's default notification interval.
const DEFAULT_INTERVAL: u64 = 60;
/// The official client's default script error interval.
const DEFAULT_ERROR_INTERVAL: u64 = 30;

/// The profile's head: badge, name and server.
const HEAD: &str = "flex items-center gap-4 mb-5";
/// The account's badge.
const BADGE: &str = "flex-none size-[64px] rounded-full bg-panel-2";
/// The badge's placeholder while it loads.
const BADGE_SKELETON: &str = "flex-none size-[64px] rounded-full bg-linear-90 from-panel-2 \
    from-25% via-panel-3 via-50% to-panel-2 to-75% bg-size-[200%_100%] animate-shimmer \
    motion-reduce:animate-none";
/// The account's figures: names beside their values, stacked on phones.
const STATS: &str = "grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 max-xs:grid-cols-1 \
    max-xs:gap-x-4 max-xs:gap-y-0";
/// The name of a figure of [`STATS`].
const STAT_NAME: &str = "text-muted";
/// The value of a figure of [`STATS`].
const STAT_VALUE: &str = "flex flex-wrap items-center gap-x-3 gap-y-1 tabular-nums max-xs:mb-2";
/// A placeholder line of [`STATS`] while the profile loads, as tall as a figure.
const STAT_SKELETON: &str = "inline-block w-24 h-[1.1em] align-middle rounded-sm bg-linear-90 \
    from-panel-2 from-25% via-panel-3 via-50% to-panel-2 to-75% bg-size-[200%_100%] \
    animate-shimmer motion-reduce:animate-none";
/// A wide [`STAT_SKELETON`].
const STAT_SKELETON_WIDE: &str = "inline-block w-48 h-[1.1em] align-middle rounded-sm \
    bg-linear-90 from-panel-2 from-25% via-panel-3 via-50% to-panel-2 to-75% \
    bg-size-[200%_100%] animate-shimmer motion-reduce:animate-none";
/// The summary's buttons, set off from the figures by a rule.
const SUMMARY_ACTIONS: &str = "flex flex-wrap gap-2 mt-5 pt-4 border-t border-edge";

/// The profile screen.
#[component]
pub fn Profile() -> impl IntoView {
    let state = AppState::expect();
    let Some(session) = state.session.get_untracked() else {
        return view! {
            <section class=concatcp!(style::PANEL_NARROW, " ", style::STATE_CARD)>
                <span class=style::STATE_ICON>
                    <Icon kind=IconKind::User class=style::STATE_ICON_SVG />
                </span>
                <p class=style::STATE_TEXT>"You are not signed in."</p>
                <button
                    type="button"
                    class=style::BUTTON
                    on:click=move |_| state.navigate(Route::Servers)
                >
                    "Servers"
                </button>
            </section>
        }
        .into_any();
    };
    let overseer = session.overseer;
    let me = RwSignal::new(None::<Result<Arc<types::AuthMeResponse>, String>>);
    let reload = {
        let overseer = overseer.clone();
        move |fresh: bool| {
            let overseer = overseer.clone();
            spawn_scoped(async move {
                let account = overseer.account();
                let result = if fresh {
                    account.refresh().await
                } else {
                    account.me().await
                };
                me.set(Some(result.map_err(|error| session::describe(&error))));
            });
        }
    };
    reload(false);
    let reload = StoredValue::new_local(reload);
    view! {
        <div class=style::PAGE>
            <Summary overseer=overseer.clone() me />
            <div class="grid grid-cols-[repeat(auto-fit,minmax(17rem,1fr))] items-start gap-4">
                <NotifyPrefs overseer=overseer.clone() me reload=Callback::new(move |()| reload.with_value(|reload| reload(true))) />
                <Email overseer=overseer.clone() me reload=Callback::new(move |()| reload.with_value(|reload| reload(true))) />
                <Password overseer me />
            </div>
        </div>
    }
    .into_any()
}

/// Badge, name, levels, CPU, credits, world and start room, and the sign-out button.
#[component]
fn Summary(
    overseer: Overseer,
    me: RwSignal<Option<Result<Arc<types::AuthMeResponse>, String>>>,
) -> impl IntoView {
    let state = AppState::expect();
    let badge = RwSignal::new(None::<String>);
    let world_status = RwSignal::new(None::<String>);
    let start_room = RwSignal::new(None::<String>);
    let start_route = RwSignal::new(None::<Route>);
    let cpu = RwSignal::new(None::<UserCpu>);
    {
        let overseer = overseer.clone();
        spawn_scoped(async move {
            if let Ok(svg) = overseer.account().badge_svg(false).await {
                badge.set(Some(format!(
                    "data:image/svg+xml;base64,{}",
                    STANDARD.encode(svg)
                )));
            }
        });
    }
    {
        let overseer = overseer.clone();
        spawn_scoped(async move {
            let account = overseer.account();
            world_status.set(Some(match account.world_status().await {
                Ok(status) => world_status_text(status).to_owned(),
                Err(error) => session::describe(&error),
            }));
            start_room.set(Some(match account.start_room(None).await {
                Ok(Some(room)) => {
                    let slug = state
                        .session_server()
                        .and_then(|server| state.slug(&server.url));
                    start_route.set(slug.map(|server| Route::World {
                        server,
                        shard: Some(Shard::of(room.shard.as_deref())),
                        room: parse_room(&room.room),
                    }));
                    room.shard.map_or_else(
                        || room.room.clone(),
                        |shard| format!("{shard} / {}", room.room),
                    )
                }
                Ok(None) => "none".to_owned(),
                Err(error) => session::describe(&error),
            }));
        });
    }
    if let Ok(mut watch) = overseer.account().cpu() {
        spawn_scoped(async move {
            if let Some(latest) = watch.latest() {
                cpu.set(Some((*latest).clone()));
            }
            while let Ok(latest) = watch.changed().await {
                cpu.set(Some((*latest).clone()));
            }
        });
    }
    let signing_out = RwSignal::new(false);
    let sign_out = move |_| {
        signing_out.set(true);
        spawn_local(async move { state.sign_out().await });
    };
    let body = move || {
        match me.get() {
        None => view! {
            <div class=HEAD role="status">
                <span class=BADGE_SKELETON aria-hidden="true"></span>
                <span class=style::SKELETON_LINE_WIDE aria-hidden="true"></span>
                <span class="sr-only">"Loading the profile…"</span>
            </div>
            <div class=STATS aria-hidden="true">
                {(0..7)
                    .map(|_| view! { <span class=STAT_SKELETON></span> <span class=STAT_SKELETON_WIDE></span> })
                    .collect_view()}
            </div>
        }
        .into_any(),
        Some(Err(error)) => view! {
            <div class=concatcp!(style::STATE_CARD, " py-4") role="alert">
                <span class=style::STATE_ICON_ERROR>
                    <Icon kind=IconKind::Alert class=style::STATE_ICON_SVG />
                </span>
                <p class=style::STATE_TEXT>{error}</p>
            </div>
        }
        .into_any(),
        Some(Ok(me)) => {
            let username = me
                .username
                .clone()
                .unwrap_or_else(|| "(no username)".to_owned());
            let alt = format!("Badge of {username}");
            let gcl = levels::gcl(me.gcl.unwrap_or(0.0));
            let power = levels::power(me.power.unwrap_or(0.0));
            let cpu_limit = me
                .cpu
                .map_or_else(|| "unknown".to_owned(), |cpu| format!("{cpu} CPU"));
            let credits = format!("{:.3}", me.money);
            let server = state.session_server().map(|server| server.name);
            view! {
                <div class=HEAD>
                    {move || {
                        badge
                            .get()
                            .map_or_else(
                                || view! { <span class=BADGE_SKELETON aria-hidden="true"></span> }.into_any(),
                                |src| {
                                    view! {
                                        <img class=BADGE src=src alt=alt.clone() width="64" height="64" />
                                    }
                                        .into_any()
                                },
                            )
                    }}
                    <div>
                        <h1 id="profile-title" class="text-xl font-semibold leading-tight">
                            {username}
                        </h1>
                        {server.map(|server| view! { <p class="mt-1 text-muted">{server}</p> })}
                    </div>
                </div>
                <dl class=STATS>
                    <dt class=STAT_NAME>"Global Control Level"</dt>
                    <dd class=STAT_VALUE>
                        <LevelBar label="GCL" level=gcl />
                    </dd>
                    <dt class=STAT_NAME>"Power level"</dt>
                    <dd class=STAT_VALUE>
                        <LevelBar label="Power level" level=power />
                    </dd>
                    <dt class=STAT_NAME>"CPU limit"</dt>
                    <dd class=STAT_VALUE>{cpu_limit}</dd>
                    <dt class=STAT_NAME>"CPU used"</dt>
                    <dd class=STAT_VALUE>{move || cpu.get().map_or_else(|| "waiting for the next tick…".to_owned(), |cpu| cpu_text(&cpu))}</dd>
                    <dt class=STAT_NAME>"Credits"</dt>
                    <dd class=STAT_VALUE>{credits}</dd>
                    <dt class=STAT_NAME>"World status"</dt>
                    <dd class=STAT_VALUE>{move || pending_text(world_status.get())}</dd>
                    <dt class=STAT_NAME>"Start room"</dt>
                    <dd class=STAT_VALUE>
                        {move || pending_text(start_room.get())}
                        {move || {
                            start_route
                                .get()
                                .map(|route| {
                                    view! {
                                        <button
                                            type="button"
                                            class=style::LINK_BUTTON
                                            on:click=move |_| state.navigate(route.clone())
                                        >
                                            "Show on the map"
                                        </button>
                                    }
                                })
                        }}
                    </dd>
                </dl>
            }
            .into_any()
        }
    }
    };
    view! {
        <section class=style::PANEL aria-labelledby="profile-title">
            {body}
            <div class=SUMMARY_ACTIONS>
                <button type="button" class=style::BUTTON_PRIMARY on:click=move |_| open_map(state)>
                    <Icon kind=IconKind::Globe />
                    "Open the map"
                </button>
                <button
                    type="button"
                    class=style::BUTTON_DANGER
                    on:click=sign_out
                    disabled=move || signing_out.get()
                >
                    <Icon kind=IconKind::SignOut />
                    "Sign out"
                </button>
            </div>
        </section>
    }
}

/// A value still being fetched shows as a placeholder bar of the text's height.
fn pending_text(value: Option<String>) -> AnyView {
    value.map_or_else(
        || {
            view! { <span class=style::SKELETON_LINE role="status" aria-label="Loading"></span> }
                .into_any()
        },
        IntoAny::into_any,
    )
}

/// Opens the world of the signed-in server.
fn open_map(state: AppState) {
    if let Some(server) = state.session_server() {
        state.open_server(&server.url);
    }
}

/// A level with a progress bar towards the next one.
#[component]
fn LevelBar(label: &'static str, level: LevelProgress) -> impl IntoView {
    let percent = level.fraction() * 100.0;
    let text = format!(
        "{} / {}",
        format_points(level.progress),
        format_points(level.total)
    );
    view! {
        <span class="min-w-[4.5em]">{format!("Level {}", level.level)}</span>
        <progress
            class=style::PROGRESS
            max="100"
            value=format!("{percent:.1}")
            aria-label=format!("{label} progress to level {}", level.level + 1)
        ></progress>
        <span class="text-muted">{text}</span>
    }
}

/// Whole points with thousands separators.
fn format_points(points: f64) -> String {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "display of non-negative point counts far below u64::MAX"
    )]
    let whole = points.max(0.0).round() as u64;
    let digits = whole.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// The latest run's CPU and Memory use.
fn cpu_text(cpu: &UserCpu) -> String {
    let used = match cpu.cpu {
        UserCpuCpu::Number(ms) => format!("{ms:.2} ms"),
        UserCpuCpu::Error => "limit exceeded".to_owned(),
    };
    match cpu.memory {
        Some(bytes) => {
            #[expect(
                clippy::cast_precision_loss,
                reason = "Memory sizes are far below 2^52"
            )]
            let kilobytes = bytes as f64 / 1024.0;
            format!("{used}, Memory {kilobytes:.1} KB")
        }
        None => used,
    }
}

/// How the world status reads.
const fn world_status_text(status: types::UserWorldStatus) -> &'static str {
    match status {
        types::UserWorldStatus::Normal => "Playing",
        types::UserWorldStatus::Lost => "Lost every spawn",
        types::UserWorldStatus::Empty => "Not in the world (place a spawn)",
    }
}

/// E-mail notification preferences.
#[component]
fn NotifyPrefs(
    overseer: Overseer,
    me: RwSignal<Option<Result<Arc<types::AuthMeResponse>, String>>>,
    reload: Callback<()>,
) -> impl IntoView {
    let enabled = RwSignal::new(true);
    let messages = RwSignal::new(true);
    let online = RwSignal::new(false);
    let interval = RwSignal::new(DEFAULT_INTERVAL.to_string());
    let errors_interval = RwSignal::new(DEFAULT_ERROR_INTERVAL.to_string());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(None::<String>);
    // Fill the form from the profile whenever it (re)loads.
    Effect::new(move |_| {
        if let Some(Ok(me)) = me.get() {
            let prefs = me.notify_prefs.clone().unwrap_or_default();
            enabled.set(!prefs.disabled.unwrap_or(false));
            messages.set(!prefs.disabled_on_messages.unwrap_or(false));
            online.set(prefs.send_online.unwrap_or(false));
            interval.set(prefs.interval.unwrap_or(DEFAULT_INTERVAL).to_string());
            errors_interval.set(
                prefs
                    .errors_interval
                    .unwrap_or(DEFAULT_ERROR_INTERVAL)
                    .to_string(),
            );
        }
    });
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let prefs = types::UserNotifyPrefs {
            disabled: Some(!enabled.get_untracked()),
            disabled_on_messages: Some(!messages.get_untracked()),
            send_online: Some(online.get_untracked()),
            interval: interval.get_untracked().parse().ok(),
            errors_interval: errors_interval.get_untracked().parse().ok(),
        };
        let overseer = overseer.clone();
        busy.set(true);
        error.set(None);
        saved.set(None);
        spawn_local(async move {
            let result = overseer.account().set_notify_prefs(prefs).await;
            busy.set(false);
            match result {
                Ok(()) => {
                    saved.set(Some("Notification preferences saved.".to_owned()));
                    reload.run(());
                }
                Err(failure) => error.set(Some(session::describe(&failure))),
            }
        });
    };
    let options = |choices: &'static [(u64, &'static str)], value: RwSignal<String>| {
        move || {
            let current = value.get();
            let mut list: Vec<(String, String)> = choices
                .iter()
                .map(|(minutes, text)| (minutes.to_string(), (*text).to_owned()))
                .collect();
            if !list.iter().any(|(minutes, _)| *minutes == current) {
                list.push((current.clone(), format!("{current} minutes")));
            }
            list.into_iter()
                .map(|(minutes, text)| {
                    let selected = minutes == current;
                    view! { <option value=minutes selected=selected>{text}</option> }
                })
                .collect_view()
        }
    };
    view! {
        <section class=style::PANEL aria-labelledby="notify-title">
            <h2 id="notify-title" class=style::H2>"Notifications"</h2>
            <form class=style::FORM on:submit=submit aria-busy=move || busy.get().to_string()>
                <div class=style::CHECK_FIELD>
                    <input id="notify-enabled" class=style::CHECKBOX type="checkbox" bind:checked=enabled />
                    <label for="notify-enabled">"Send notification e-mails"</label>
                </div>
                <div class=style::CHECK_FIELD>
                    <input id="notify-messages" class=style::CHECKBOX type="checkbox" bind:checked=messages />
                    <label for="notify-messages">"Notify about new messages"</label>
                </div>
                <div class=style::CHECK_FIELD>
                    <input id="notify-online" class=style::CHECKBOX type="checkbox" bind:checked=online />
                    <label for="notify-online">"Notify even while I am online"</label>
                </div>
                <div class=style::FIELD>
                    <label for="notify-interval" class=style::LABEL>"Send interval"</label>
                    <select id="notify-interval" class=style::INPUT bind:value=interval>
                        {options(&INTERVALS, interval)}
                    </select>
                </div>
                <div class=style::FIELD>
                    <label for="notify-errors" class=style::LABEL>"Script error reports"</label>
                    <select id="notify-errors" class=style::INPUT bind:value=errors_interval>
                        {options(&ERROR_INTERVALS, errors_interval)}
                    </select>
                </div>
                <FormError error />
                <FormStatus message=saved />
                <button type="submit" class=style::BUTTON_PRIMARY disabled=move || busy.get()>
                    "Save notifications"
                </button>
            </form>
        </section>
    }
}

/// The account e-mail.
#[component]
fn Email(
    overseer: Overseer,
    me: RwSignal<Option<Result<Arc<types::AuthMeResponse>, String>>>,
    reload: Callback<()>,
) -> impl IntoView {
    let email = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(None::<String>);
    let current = move || {
        me.get()
            .and_then(Result::ok)
            .and_then(|me| me.email.clone().or_else(|| me.email_dirty.clone()))
    };
    Effect::new(move |_| {
        if let Some(address) = current() {
            email.set(address);
        }
    });
    let field_error = Signal::derive(move || {
        let text = email.get();
        let text = text.trim();
        (!text.is_empty())
            .then(|| email_error(text))
            .flatten()
            .map(str::to_owned)
    });
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let address = email.get_untracked().trim().to_owned();
        if let Some(rule) = email_error(&address) {
            error.set(Some(rule.to_owned()));
            return;
        }
        let overseer = overseer.clone();
        busy.set(true);
        error.set(None);
        saved.set(None);
        spawn_local(async move {
            let body = types::UserEmailRequest { email: address };
            let result = overseer
                .client()
                .send(&endpoints::UserEmailSet { body })
                .await;
            busy.set(false);
            match result {
                Ok(_) => {
                    saved.set(Some("E-mail address saved.".to_owned()));
                    reload.run(());
                }
                Err(failure) => error.set(Some(session::message(&failure))),
            }
        });
    };
    view! {
        <section class=style::PANEL aria-labelledby="email-title">
            <h2 id="email-title" class=style::H2>"E-mail"</h2>
            <form class=style::FORM on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
                <Field id="profile-email" label="E-mail address" kind="email" autocomplete="email" value=email error=field_error />
                <FormError error />
                <FormStatus message=saved />
                <button type="submit" class=style::BUTTON_PRIMARY disabled=move || busy.get()>
                    "Save e-mail"
                </button>
            </form>
        </section>
    }
}

/// The account password.
#[component]
fn Password(
    overseer: Overseer,
    me: RwSignal<Option<Result<Arc<types::AuthMeResponse>, String>>>,
) -> impl IntoView {
    let state = AppState::expect();
    let old = RwSignal::new(String::new());
    let new = RwSignal::new(String::new());
    let confirm = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let saved = RwSignal::new(None::<String>);
    // Accounts created through Steam or GitHub have no password yet.
    let has_password = move || {
        me.get()
            .and_then(Result::ok)
            .is_none_or(|me| me.password.unwrap_or(true))
    };
    let new_error = Signal::derive(move || {
        (!new.with(String::is_empty))
            .then(|| password_error(&new.get()))
            .flatten()
            .map(str::to_owned)
    });
    let confirm_message = Signal::derive(move || {
        (!confirm.with(String::is_empty))
            .then(|| confirm_error(&new.get(), &confirm.get()))
            .flatten()
            .map(str::to_owned)
    });
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let password = new.get_untracked();
        if let Some(rule) =
            password_error(&password).or_else(|| confirm_error(&password, &confirm.get_untracked()))
        {
            error.set(Some(rule.to_owned()));
            return;
        }
        let body = types::UserPasswordSetRequest {
            old_password: old.get_untracked(),
            password: password.clone(),
        };
        let overseer = overseer.clone();
        // Private servers revoke every session of the account: sign in again with the new
        // password under the account's name.
        let renew_as = state
            .session_server()
            .filter(|server| !server.kind.uses_screeps_accounts())
            .and_then(|server| server.user.and_then(|user| user.username).or(server.login));
        busy.set(true);
        error.set(None);
        saved.set(None);
        spawn_local(async move {
            let result = overseer
                .client()
                .send(&endpoints::UserPasswordSet { body })
                .await;
            busy.set(false);
            match result {
                Ok(_) => {
                    old.set(String::new());
                    new.set(String::new());
                    confirm.set(String::new());
                    saved.set(Some("Password changed.".to_owned()));
                    if let Some(login) = renew_as
                        && state.renew(login, password).await.is_ok()
                    {
                        state.banner.set(Some(
                            "Password changed; signed in again with the new password.".to_owned(),
                        ));
                    }
                }
                Err(failure) => error.set(Some(session::message(&failure))),
            }
        });
    };
    view! {
        <section class=style::PANEL aria-labelledby="password-title">
            <h2 id="password-title" class=style::H2>"Password"</h2>
            <form class=style::FORM on:submit=submit novalidate aria-busy=move || busy.get().to_string()>
                <Show when=has_password>
                    <Field id="password-old" label="Current password" kind="password" autocomplete="current-password" value=old />
                </Show>
                <Field
                    id="password-new"
                    label="New password"
                    kind="password"
                    autocomplete="new-password"
                    value=new
                    error=new_error
                    help="8 to 1024 characters."
                />
                <Field
                    id="password-confirm"
                    label="Repeat the new password"
                    kind="password"
                    autocomplete="new-password"
                    value=confirm
                    error=confirm_message
                />
                <FormError error />
                <FormStatus message=saved />
                <button type="submit" class=style::BUTTON_PRIMARY disabled=move || busy.get()>
                    "Change password"
                </button>
            </form>
        </section>
    }
}
