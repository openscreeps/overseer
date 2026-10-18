//! The live session's rate limits: the Rate limits panel (account menu), the status bar's
//! `Reset limit` and `Rate limits lifted` items, the prompt to lift them, and the lift flow
//! ([`crate::rate_limits`]), as the server's lifting subsystem says
//! ([`overseer::Account::rate_limit_lift`]).
//!
//! A lift page (screeps.com's reset page) opens as a popup in a browser and resumes the
//! world's overviews on its success message (from the page's origin only); in the shell it
//! opens in the system browser, which cannot talk back, so the app rechecks when its window
//! regains focus. A lift request (screepsmod-auth) is sent, then the app rechecks. A recheck
//! ([`crate::feed::Feed::recheck_rate_limit`]) sends nothing unless the world's overviews are
//! paused by the limit.
//!
//! On API-token sessions of servers that lift rate limits (main window only,
//! [`rate_limits::follows_lift`]), [`RateLimitsUi::follow`] makes the session check whether
//! the token's limits are lifted ([`overseer::RateLimits::follow`]): it tells the session
//! when the window gets focus or the page becomes visible again (the user may have lifted
//! them on the website), after a reset and after `Check again`, and whether the world's
//! loading waits on the limits. It shows the session's exemption
//! ([`overseer::RateLimits::exemption`]), which the world map and the request budget follow
//! on their own.

use std::time::Duration;

use futures_util::future::{AbortHandle, abortable};
use leptos::html::{Button, Section};
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::map::WorldProgress;
use overseer::model::platform::Subsystem;
use overseer::net::api::{LiftPage, RateLimitLift};
use overseer::rate_limits::{Exemption, Following};
use overseer::{Overseer, RateLimits};
use tokio::sync::watch;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{FocusEvent, HtmlElement, KeyboardEvent, Node};

use crate::rate_limits::{self, Actions, Warning, Warnings};
use crate::session;
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::style::{self, concatcp};
use crate::ui::toast::{ToastKind, Toasts};
use crate::ui::world::MapState;

/// The rate-limits panel: a [`style::FLOATING`] panel under the top bar.
const LIMITS_PANEL: &str = concatcp!(
    style::FLOATING,
    " top-[calc(var(--spacing-topbar-safe)+0.5rem)] z-40 \
    w-[min(34rem,calc(100vw-1.5rem-var(--spacing-safe-l)-var(--spacing-safe-r)))] \
    origin-top-right"
);
/// The lift prompt: a [`style::FLOATING`] panel over the status bar, with a warning edge.
const LIFT_PANEL: &str = concatcp!(
    style::FLOATING,
    " bottom-[calc(var(--spacing-statusbar-safe)+1rem)] z-61 \
    w-[min(26rem,calc(100vw-1.5rem-var(--spacing-safe-l)-var(--spacing-safe-r)))] border-l-3 \
    border-l-warn origin-bottom-right"
);

/// What [`RateLimitsUi::follow`] tracks of the live session.
#[derive(Debug, Default)]
struct Monitor {
    /// The session followed ([`crate::state::Session::epoch`]).
    epoch: Option<u64>,
    /// The session's exemption, while followed.
    limits: Option<RateLimits>,
    /// Keeps the session checking its exemption.
    following: Option<Following>,
    /// Stops copying the session's exemption into [`RateLimitsUi::exemption`].
    mirror: Option<AbortHandle>,
    /// The lift (its end) whose ending toast is shown, and the toast.
    toasted: Option<(u64, u64)>,
    /// The prompts shown in this session.
    warnings: Warnings,
}

/// The panel, the clock of relative times, the lift and the reset flow; `Copy`, provided as
/// context.
#[derive(Clone, Copy)]
pub struct RateLimitsUi {
    /// The Rate limits panel is shown.
    pub open: RwSignal<bool>,
    /// Now, in milliseconds since the Unix epoch, updated every second.
    pub now: RwSignal<u64>,
    /// The live session's exemption from rate limits as the session knows it
    /// ([`overseer::RateLimits::exemption`]), while the window follows it; see
    /// [`Self::lifted_until`].
    pub exemption: RwSignal<Exemption>,
    /// The prompt to lift the limits, while shown.
    pub warning: RwSignal<Option<Warning>>,
    /// The shell opened the lift page in the system browser: recheck once the window
    /// regains focus.
    awaiting_focus: StoredValue<bool>,
    /// The lift page opened last, whose success message is awaited.
    page: StoredValue<Option<LiftPage>>,
    monitor: StoredValue<Monitor>,
}

impl RateLimitsUi {
    /// The panel closed and the clock running for as long as the page lives.
    pub fn new() -> Self {
        let now = RwSignal::new(epoch_millis());
        // Never cleared: the app lives as long as the page. Without the timer (no window)
        // relative times stay put.
        let _ = set_interval_with_handle(move || now.set(epoch_millis()), Duration::from_secs(1));
        Self {
            open: RwSignal::new(false),
            now,
            exemption: RwSignal::new(Exemption::default()),
            warning: RwSignal::new(None),
            awaiting_focus: StoredValue::new(false),
            page: StoredValue::new(None),
            monitor: StoredValue::new(Monitor::default()),
        }
    }

    /// The state provided by [`crate::ui::App`].
    pub fn expect() -> Self {
        expect_context()
    }

    /// Until when the token's limits are lifted, while they are (reactive; follows the clock
    /// only while a lift is known).
    pub fn lifted_until(self) -> Option<u64> {
        self.exemption
            .get()
            .until
            .filter(|until| *until > self.now.get())
    }

    /// Rechecks after a reset: on the lift page's success message (browser) or when the window
    /// regains focus after the shell opened the page; and tells the session when the window
    /// gets focus or the page becomes visible again ([`RateLimits::attention`]). For as long
    /// as the page lives.
    pub fn listen(self, map: MapState) {
        let message = window_event_listener(leptos::ev::message, move |event| {
            let data = event.data().as_string();
            let lifted = self.page.with_value(|page| {
                page.as_ref()
                    .is_some_and(|page| page.is_success(&event.origin(), data.as_deref()))
            });
            if lifted {
                self.lifted(map);
            }
        });
        let focus = window_event_listener(leptos::ev::focus, move |_| {
            if self.awaiting_focus.get_value() {
                self.awaiting_focus.set_value(false);
                self.lifted(map);
            }
            self.attention();
        });
        std::mem::forget(message);
        std::mem::forget(focus);
        let visible = Closure::<dyn Fn()>::new(move || {
            if document().visibility_state() == web_sys::VisibilityState::Visible {
                self.attention();
            }
        });
        // Lives as long as the page, like the listeners above.
        let _ = document()
            .add_event_listener_with_callback("visibilitychange", visible.as_ref().unchecked_ref());
        visible.forget();
    }

    /// The limits may have been lifted: resumes the world's overviews paused by the rate
    /// limit and asks the session to check the lift at once.
    fn lifted(self, map: MapState) {
        recheck(map);
        self.monitor.with_value(|monitor| {
            if let Some(limits) = &monitor.limits {
                limits.request_check();
            }
        });
    }

    /// The app got the user's attention back: the session checks the lift (throttled).
    fn attention(self) {
        self.monitor.with_value(|monitor| {
            if let Some(limits) = &monitor.limits {
                limits.attention();
            }
        });
    }

    /// Follows the lift of the live session's token on API-token sessions of servers that
    /// lift rate limits, in the main window ([`rate_limits::follows_lift`]): keeps the
    /// session checking it (see the module docs), toasts before it ends and prompts to lift
    /// the limits when an endpoint runs low. As long as the page lives.
    pub fn follow(self, state: AppState, map: MapState) {
        let toasts = Toasts::expect();
        let lift_again = Callback::new(move |()| start_reset(state, self, map));
        let popout = state.is_popout();
        Effect::new(move |_| {
            let now = self.now.get();
            let (lifts, api_token) = session_lift(state);
            let session = state
                .session
                .with(|session| {
                    session
                        .as_ref()
                        .map(|session| (session.epoch(), session.overseer.clone()))
                })
                .filter(|_| rate_limits::follows_lift(lifts, api_token, popout));
            match session {
                Some((epoch, overseer)) => {
                    let held_back = map.progress.with_untracked(|progress| {
                        progress
                            .as_ref()
                            .is_some_and(WorldProgress::waits_on_budget)
                    });
                    self.tick(now, epoch, held_back, &overseer, toasts, lift_again);
                }
                None => self.stop(toasts),
            }
        });
    }

    /// Forgets the lift: no session, or one it does not apply to.
    fn stop(self, toasts: Toasts) {
        let stale = self
            .monitor
            .try_update_value(|monitor| {
                let monitor = std::mem::take(monitor);
                if let Some(mirror) = monitor.mirror {
                    mirror.abort();
                }
                monitor.toasted
            })
            .flatten();
        if let Some((_, toast)) = stale {
            toasts.dismiss(toast);
        }
        if self
            .exemption
            .with_untracked(|exemption| *exemption != Exemption::default())
        {
            self.exemption.set(Exemption::default());
        }
        if self.warning.with_untracked(Option::is_some) {
            self.warning.set(None);
        }
    }

    /// Removes the ending toast unless it is the one of the lift ending at `keep`.
    fn dismiss_toast(self, toasts: Toasts, keep: Option<u64>) {
        let stale = self
            .monitor
            .try_update_value(|monitor| monitor.toasted.take_if(|(until, _)| Some(*until) != keep))
            .flatten();
        if let Some((_, toast)) = stale {
            toasts.dismiss(toast);
        }
    }

    /// Starts following session `epoch` of `overseer` unless it does already: the session
    /// checks the lift, and [`Self::exemption`] copies what it knows.
    fn start(self, epoch: u64, overseer: &Overseer) {
        let fresh = self
            .monitor
            .with_value(|monitor| monitor.epoch != Some(epoch));
        if !fresh {
            return;
        }
        let limits = overseer.rate_limits();
        // Fails only on a server that does not lift rate limits, which `follows_lift` excludes.
        let following = limits.follow().ok();
        let mirror = self.mirror(limits.exemption());
        self.monitor.update_value(|monitor| {
            if let Some(mirror) = monitor.mirror.take() {
                mirror.abort();
            }
            *monitor = Monitor {
                epoch: Some(epoch),
                limits: Some(limits),
                following,
                mirror: Some(mirror),
                toasted: monitor.toasted,
                warnings: Warnings::default(),
            };
        });
        self.warning.set(None);
    }

    /// Copies every value of `exemption` into [`Self::exemption`] until aborted.
    fn mirror(self, mut exemption: watch::Receiver<Exemption>) -> AbortHandle {
        let (future, handle) = abortable(async move {
            loop {
                let value = *exemption.borrow_and_update();
                self.exemption.set(value);
                if exemption.changed().await.is_err() {
                    break;
                }
            }
        });
        spawn_local(async move {
            // `Err` only says it was aborted.
            let _ = future.await;
        });
        handle
    }

    /// One second of following session `epoch` at `now`, with loading `held_back` by the
    /// limits or not ([`WorldProgress::waits_on_budget`]).
    fn tick(
        self,
        now: u64,
        epoch: u64,
        held_back: bool,
        overseer: &Overseer,
        toasts: Toasts,
        lift_again: Callback<()>,
    ) {
        self.start(epoch, overseer);
        self.monitor.with_value(|monitor| {
            if let Some(following) = &monitor.following {
                following.set_held_back(held_back);
            }
        });
        let exemption = self.exemption.get_untracked();
        let lifted = exemption.until_after(now);
        // The ending toast belongs to one lift: gone once it ended or another replaced it.
        self.dismiss_toast(toasts, lifted);
        if let Some(until) = lifted {
            if self.warning.with_untracked(Option::is_some) {
                self.warning.set(None);
            }
            if let Some(text) = rate_limits::ending_label(until, now)
                && self.monitor.with_value(|monitor| monitor.toasted.is_none())
            {
                let toast = toasts.show_action(ToastKind::Info, text, "Lift again", lift_again);
                self.monitor
                    .update_value(|monitor| monitor.toasted = Some((until, toast)));
            }
            return;
        }
        // Not lifted (as far as known once a check completed): prompt when an endpoint runs
        // low.
        if !exemption.settled || self.warning.with_untracked(Option::is_some) {
            return;
        }
        let limits = overseer.client().rate_limits();
        let warning = self
            .monitor
            .try_update_value(|monitor| {
                monitor.warnings.next(
                    limits
                        .iter()
                        .map(|endpoint| (endpoint.method.as_str(), endpoint.path, endpoint.limit)),
                    now,
                )
            })
            .flatten();
        if warning.is_some() {
            self.warning.set(warning);
        }
    }

    /// `Not now` on the prompt for `endpoint`: no prompt for it again in this session.
    fn decline(self, endpoint: &str) {
        self.monitor
            .update_value(|monitor| monitor.warnings.decline(endpoint));
        self.warning.set(None);
    }
}

/// Now, in milliseconds since the Unix epoch.
pub fn epoch_millis() -> u64 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the epoch time in milliseconds is a non-negative integer far below u64::MAX"
    )]
    let now = js_sys::Date::now() as u64;
    now
}

/// Whether the live session's server lifts rate limits
/// ([`Subsystem::RateLimitLift`]) and whether the session signed in with an API token
/// (reactive); neither without a session.
fn session_lift(state: AppState) -> (bool, Option<bool>) {
    let session = state.session.with(|session| {
        session.as_ref().map(|session| {
            (
                session.server.clone(),
                session.overseer.world().supports(Subsystem::RateLimitLift),
            )
        })
    });
    session.map_or((false, None), |(url, lifts)| {
        let api_token = state.profiles.with(|profiles| {
            profiles
                .server(&url)
                .and_then(|server| server.user.as_ref().and_then(|user| user.api_token))
        });
        (lifts, api_token)
    })
}

/// The host of the web page where the live session's token gets its limits lifted, for the
/// texts naming it; `None` where a request lifts them, and without a session or a lift.
fn lift_page_host(state: AppState) -> Option<String> {
    state.session.with(|session| {
        match session
            .as_ref()?
            .overseer
            .account()
            .rate_limit_lift()
            .ok()?
        {
            RateLimitLift::Page(page) => Some(rate_limits::page_host(&page.url)),
            RateLimitLift::Request(_) => None,
        }
    })
}

/// The rate-limit actions of the live session (reactive).
pub fn session_actions(state: AppState, map: MapState, ui: RateLimitsUi) -> Actions {
    let paused = map
        .progress
        .with(|progress| progress.is_some_and(|progress| progress.stats_paused_until.is_some()));
    let (lifts, api_token) = session_lift(state);
    rate_limits::actions(lifts, api_token, paused, ui.lifted_until().is_some())
}

/// Resumes the world's overviews paused by the rate limit, if any.
pub fn recheck(map: MapState) {
    map.feed.with_untracked(|feed| {
        if let Some(feed) = feed {
            feed.recheck_rate_limit();
        }
    });
}

/// Lifts the rate limits of the live session's API token as its server does (only the
/// token's first 8 characters leave the app): opens the lift page, or sends the lift request
/// and rechecks. Failures go to the banner.
pub fn start_reset(state: AppState, ui: RateLimitsUi, map: MapState) {
    let lift = state.session.with_untracked(|session| {
        session
            .as_ref()
            .map(|session| session.overseer.account().rate_limit_lift())
    });
    let lift = match lift {
        Some(Ok(lift)) => lift,
        Some(Err(error)) => {
            state.banner.set(Some(format!(
                "Could not lift the rate limits: {}",
                session::error_message(&error)
            )));
            return;
        }
        None => {
            state
                .banner
                .set(Some("No API token to reset the rate limits of.".to_owned()));
            return;
        }
    };
    match lift {
        RateLimitLift::Page(page) => {
            let url = page.url.clone();
            let host = rate_limits::page_host(&url);
            ui.page.set_value(Some(page));
            let platform = state.platform();
            if platform.is_tauri() {
                ui.awaiting_focus.set_value(true);
            }
            spawn_local(async move {
                if let Err(error) = platform.open_reset_page(&url).await {
                    ui.awaiting_focus.set_value(false);
                    state
                        .banner
                        .set(Some(format!("Could not open {host}'s reset page: {error}")));
                }
            });
        }
        RateLimitLift::Request(request) => {
            spawn_local(async move {
                match request.send().await {
                    Ok(()) => ui.lifted(map),
                    Err(error) => state.banner.set(Some(format!(
                        "Could not lift the rate limits: {}",
                        session::message(&error)
                    ))),
                }
            });
        }
    }
}

/// `Reset limit` in the status bar, while the world's overviews wait for a limit that lifting
/// can lift.
#[component]
pub fn StatusReset() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let ui = RateLimitsUi::expect();
    move || {
        session_actions(state, map, ui).status_reset.then(|| {
            let title = match lift_page_host(state) {
                Some(host) => format!("Lift the rate limits of your API token on {host}"),
                None => "Lift the rate limits of your API token".to_owned(),
            };
            view! {
                <span class="inline-flex flex-none items-center gap-2">
                    <button
                        type="button"
                        class=concatcp!(style::LINK_BUTTON, " text-[length:inherit]")
                        title=title
                        on:click=move |_| start_reset(state, ui, map)
                    >
                        "Reset limit"
                    </button>
                </span>
            }
        })
    }
}

/// `Rate limits lifted · 1 h 52 min` in the status bar while the token's limits are lifted.
#[component]
pub fn StatusLifted() -> impl IntoView {
    let ui = RateLimitsUi::expect();
    let label = Memo::new(move |_| {
        ui.exemption
            .get()
            .until
            .and_then(|until| Some((until, rate_limits::lifted_label(until, ui.now.get())?)))
    });
    move || {
        label.get().map(|(until, label)| {
            view! {
                <span
                    class="inline-flex flex-none items-center gap-2 text-ok"
                    title=format!(
                        "Your API token's rate limits are lifted until {}",
                        local_clock(until),
                    )
                >
                    {label}
                </span>
            }
        })
    }
}

/// The Rate limits panel: the endpoints with a known limit, the lift, and the reset and
/// recheck actions where they apply.
#[component]
pub fn RateLimitsPanel() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let ui = RateLimitsUi::expect();
    let close = NodeRef::<Button>::new();
    // The panel takes the focus as soon as its close button mounts (an effect runs before the
    // next input event): Escape right after the panel opens closes it.
    Effect::new(move |_| {
        if let Some(close) = close.get() {
            let _ = close.focus();
        }
    });
    let keydown = move |event: KeyboardEvent| {
        if event.key() == "Escape" {
            event.prevent_default();
            ui.open.set(false);
        }
    };
    let rows = move || {
        let now = ui.now.get();
        let limits = state.session.with(|session| {
            session
                .as_ref()
                .map(|session| session.overseer.client().rate_limits())
                .unwrap_or_default()
        });
        if limits.is_empty() {
            return view! {
                <p class=style::EMPTY_STATE>
                    <Icon kind=IconKind::Check class=style::EMPTY_ICON />
                    "No endpoint has reported a rate limit in this session yet."
                </p>
            }
            .into_any();
        }
        let rows = limits
            .into_iter()
            .map(|endpoint| {
                let reset = endpoint.limit.reset.saturating_mul(1000);
                let resets = rate_limits::time_until(reset, now)
                    .map_or_else(|| "already reset".to_owned(), |left| format!("in {left}"));
                view! {
                    <tr>
                        <th scope="row" class=style::TABLE_ROW>
                            <code class="font-mono text-[0.92em]">
                                {format!("{} /{}", endpoint.method, endpoint.path)}
                            </code>
                        </th>
                        <td class=style::TABLE_CELL>{endpoint.limit.limit}</td>
                        <td class=style::TABLE_CELL>{endpoint.limit.remaining}</td>
                        <td class=style::TABLE_CELL title=local_time(reset)>{resets}</td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <table class="w-full border-collapse text-sm tabular-nums">
                <thead>
                    <tr>
                        <th scope="col" class=style::TABLE_COLUMN>"Endpoint"</th>
                        <th scope="col" class=style::TABLE_COLUMN>"Limit"</th>
                        <th scope="col" class=style::TABLE_COLUMN>"Remaining"</th>
                        <th scope="col" class=style::TABLE_COLUMN>"Resets"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        }
        .into_any()
    };
    let lifted = move || {
        let until = ui.lifted_until()?;
        let left = rate_limits::time_until(until, ui.now.get())?;
        Some(view! {
            <p
                class="flex items-center gap-2 mb-4 px-3 py-2 bg-ok-soft border-l-3 border-ok rounded-r-sm"
                role="status"
            >
                <Icon kind=IconKind::Check />
                {format!(
                    "Your API token's rate limits are lifted until {} ({left} left): the map loads without waiting for its budget.",
                    local_clock(until),
                )}
            </p>
        })
    };
    let waiting = move || {
        let until = map
            .progress
            .with(|progress| progress.and_then(|progress| progress.stats_paused_until))?;
        let left = rate_limits::time_until(until, ui.now.get())
            .map_or_else(|| "shortly".to_owned(), |left| format!("in {left}"));
        Some(view! {
            <p class=style::NOTICE>
                {format!(
                    "The map's room owners wait for the map-stats limit; loading resumes {left}."
                )}
            </p>
        })
    };
    let tauri = state
        .platform
        .with_value(crate::platform::Platform::is_tauri);
    let actions = move || {
        let actions = session_actions(state, map, ui);
        let reset = actions.reset.then(|| {
            let (help, label) = match lift_page_host(state) {
                Some(host) => (
                    format!(
                        "{host} lets you lift your API token's limits early: its reset page asks you to solve a captcha. Only the first 8 characters of your token are sent, in the page's address; the full token never leaves Overseer.{}",
                        if tauri {
                            " The page opens in your browser; Overseer checks again when you come back."
                        } else {
                            ""
                        },
                    ),
                    format!("Reset on {host}"),
                ),
                None => (
                    "The server lifts your API token's limits early for about 2 hours on request. Only the first 8 characters of your token are sent; the full token never leaves Overseer.".to_owned(),
                    "Lift limits".to_owned(),
                ),
            };
            view! {
                <p class=style::HELP>{help}</p>
                <button type="button" class=style::BUTTON_PRIMARY on:click=move |_| start_reset(state, ui, map)>
                    {label}
                </button>
            }
        });
        let check = actions.check_again.then(|| {
            view! {
                <button type="button" class=style::BUTTON on:click=move |_| ui.lifted(map)>
                    "Check again"
                </button>
            }
        });
        view! { <div class=style::ACTIONS>{reset}{check}</div> }
    };
    move || {
        ui.open.get().then(|| {
            view! {
                <section
                    class=LIMITS_PANEL
                    role="dialog"
                    aria-labelledby="rate-limits-title"
                    on:keydown=keydown
                >
                    <div class=style::FLOATING_HEAD>
                        <h2 id="rate-limits-title" class=style::FLOATING_TITLE>
                            <Icon kind=IconKind::Gauge class=style::ICON_MUTED />
                            "Rate limits"
                        </h2>
                        <button
                            type="button"
                            class=concatcp!(style::ICON_BUTTON_GHOST_SM, " relative ", style::TIP_LEFT)
                            node_ref=close
                            aria-label="Close rate limits"
                            data-tip="Close"
                            on:click=move |_| ui.open.set(false)
                        >
                            <Icon kind=IconKind::Close />
                        </button>
                    </div>
                    {lifted}
                    {waiting}
                    {rows}
                    {actions}
                </section>
            }
        })
    }
}

/// The prompt to lift the limits when an endpoint runs low ([`RateLimitsUi::warning`]): a
/// non-modal dialog that never takes the focus (typing elsewhere goes on), with `Lift limits
/// for 2 hours` (the lift page or request) and `Not now` (Escape inside it too). Closing it
/// while it holds the focus gives the focus back to where it came from.
#[component]
pub fn LiftPrompt() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let ui = RateLimitsUi::expect();
    let dialog = NodeRef::<Section>::new();
    let came_from = StoredValue::new_local(None::<HtmlElement>);
    let close = move || {
        let inside = dialog
            .get_untracked()
            .zip(document().active_element())
            .is_some_and(|(dialog, active)| dialog.contains(Some(active.unchecked_ref::<Node>())));
        let from = came_from.get_value();
        came_from.set_value(None);
        if inside && let Some(from) = from.filter(|from| from.is_connected()) {
            let _ = from.focus();
        }
    };
    let focusin = move |event: FocusEvent| {
        let from = event
            .related_target()
            .and_then(|target| target.dyn_into::<HtmlElement>().ok());
        let outside = dialog.get_untracked().is_some_and(|dialog| {
            from.as_ref()
                .is_some_and(|from| !dialog.contains(Some(from.unchecked_ref::<Node>())))
        });
        if outside {
            came_from.set_value(from);
        }
    };
    let not_now = move || {
        if let Some(warning) = ui.warning.get_untracked() {
            close();
            ui.decline(&warning.endpoint);
        }
    };
    let lift = move || {
        close();
        ui.warning.set(None);
        start_reset(state, ui, map);
    };
    let keydown = move |event: KeyboardEvent| {
        if event.key() == "Escape" {
            event.prevent_default();
            not_now();
        }
    };
    move || {
        let warning = ui.warning.get()?;
        let reset = warning.reset;
        let text = move || warning.text(ui.now.get());
        Some(view! {
            <section
                class=LIFT_PANEL
                role="dialog"
                aria-labelledby="lift-prompt-title"
                aria-describedby="lift-prompt-text"
                node_ref=dialog
                on:keydown=keydown
                on:focusin=focusin
            >
                <div class=style::FLOATING_HEAD>
                    <h2 id="lift-prompt-title" class=style::FLOATING_TITLE>
                        <Icon kind=IconKind::Gauge class=style::ICON_MUTED />
                        "Rate limit running low"
                    </h2>
                </div>
                <p id="lift-prompt-text" class="mb-3" title=local_time(reset)>{text}</p>
                <p class=concatcp!(style::HELP, " mb-3")>
                    {move || match lift_page_host(state) {
                        Some(host) => format!(
                            "{host} can lift your API token's rate limits for 2 hours: its reset page asks you to solve a captcha. Only the first 8 characters of your token are sent."
                        ),
                        None => "The server can lift your API token's rate limits for 2 hours. Only the first 8 characters of your token are sent.".to_owned(),
                    }}
                </p>
                <div class=style::ACTIONS>
                    <button type="button" class=style::BUTTON_PRIMARY on:click=move |_| lift()>
                        "Lift limits for 2 hours"
                    </button>
                    <button type="button" class=style::BUTTON on:click=move |_| not_now()>
                        "Not now"
                    </button>
                </div>
            </section>
        })
    }
}

/// `at` (milliseconds since the Unix epoch) as the browser's local date and time.
fn local_time(at: u64) -> String {
    String::from(date(at).to_locale_string("default", &wasm_bindgen::JsValue::UNDEFINED))
}

/// `at` (milliseconds since the Unix epoch) as the browser's local time of day.
fn local_clock(at: u64) -> String {
    String::from(date(at).to_locale_time_string("default"))
}

/// `at` (milliseconds since the Unix epoch) as a JavaScript date.
fn date(at: u64) -> js_sys::Date {
    #[expect(
        clippy::cast_precision_loss,
        reason = "epoch milliseconds stay far below 2^53"
    )]
    js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at as f64))
}
