//! The live session's rate limits: the Rate limits panel (account menu), the status bar's
//! `Reset limit` and `Rate limits lifted` items, the prompt to lift them, and screeps.com's
//! reset flow ([`crate::rate_limits`]).
//!
//! The reset page opens as a popup in a browser and resumes the world's overviews on its
//! success message (from `https://screeps.com` only); in the shell it opens in the system
//! browser, which cannot talk back, so the app rechecks when its window regains focus. A
//! recheck ([`crate::feed::Feed::recheck_rate_limit`]) sends nothing unless the world's
//! overviews are paused by the limit.
//!
//! On screeps.com API-token sessions (main window only), [`RateLimitsUi::follow`] checks
//! whether the token's limits are lifted ([`overseer::Account::rate_limit_exemption`]): when
//! the session starts, after a reset, after `Check again`, after HTTP 429 replies (at most
//! every [`RATE_LIMITED_DEBOUNCE_MS`]), every [`EXEMPT_RECHECK_MS`] while lifted and when the
//! lift ends. The world map ignores its budget while lifted
//! ([`crate::feed::Feed::set_rate_limit_exempt_until`]).

use std::time::Duration;

use leptos::html::{Button, Section};
use leptos::prelude::*;
use leptos::task::spawn_local;
use overseer::Account;
use wasm_bindgen::JsCast;
use web_sys::{FocusEvent, HtmlElement, KeyboardEvent, Node};

use crate::profiles::ServerKind;
use crate::rate_limits::{
    self, Actions, EXEMPT_RECHECK_MS, RATE_LIMITED_DEBOUNCE_MS, Warning, Warnings,
};
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::toast::{ToastKind, Toasts};
use crate::ui::world::MapState;

/// What [`RateLimitsUi::follow`] tracks of the live session.
#[derive(Debug, Default)]
struct Monitor {
    /// The session followed ([`crate::state::Session::epoch`]).
    epoch: Option<u64>,
    /// A check of the lift is in flight.
    checking: bool,
    /// Check at once (session start, reset success, `Check again`).
    check_now: bool,
    /// When the latest check started (milliseconds since the Unix epoch).
    last_check: u64,
    /// When to check again (while lifted: slowly, and at the lift's end).
    next_check: Option<u64>,
    /// The client's count of HTTP 429 replies already seen.
    rate_limited: u64,
    /// HTTP 429 replies arrived since the latest check.
    after_rate_limited: bool,
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
    /// Until when the live session's token is exempt from rate limits (milliseconds since the
    /// Unix epoch), as last checked; see [`Self::lifted_until`].
    pub exempt_until: RwSignal<Option<u64>>,
    /// The prompt to lift the limits, while shown.
    pub warning: RwSignal<Option<Warning>>,
    /// The shell opened the reset page in the system browser: recheck once the window
    /// regains focus.
    awaiting_focus: StoredValue<bool>,
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
            exempt_until: RwSignal::new(None),
            warning: RwSignal::new(None),
            awaiting_focus: StoredValue::new(false),
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
        self.exempt_until
            .get()
            .filter(|until| *until > self.now.get())
    }

    /// Rechecks after a reset: on the page's success message (browser) or when the window
    /// regains focus after the shell opened the page. For as long as the page lives.
    pub fn listen(self, map: MapState) {
        let message = window_event_listener(leptos::ev::message, move |event| {
            if rate_limits::is_reset_success(&event.origin(), event.data().as_string().as_deref()) {
                self.lifted(map);
            }
        });
        let focus = window_event_listener(leptos::ev::focus, move |_| {
            if self.awaiting_focus.get_value() {
                self.awaiting_focus.set_value(false);
                self.lifted(map);
            }
        });
        std::mem::forget(message);
        std::mem::forget(focus);
    }

    /// The limits may have been lifted: resumes the world's overviews paused by the rate
    /// limit and checks the lift.
    fn lifted(self, map: MapState) {
        recheck(map);
        self.monitor
            .update_value(|monitor| monitor.check_now = true);
    }

    /// Follows the lift of the live session's token on screeps.com API-token sessions: checks
    /// it (see the module docs), tells the world map, toasts before it ends and prompts to
    /// lift the limits when an endpoint runs low. For the main window, as long as the page
    /// lives.
    pub fn follow(self, state: AppState, map: MapState) {
        let toasts = Toasts::expect();
        let lift_again = Callback::new(move |()| start_reset(state, self));
        Effect::new(move |_| {
            let until = self.exempt_until.get();
            map.feed.with(|feed| {
                if let Some(feed) = feed {
                    feed.set_rate_limit_exempt_until(until);
                }
            });
        });
        Effect::new(move |_| {
            let now = self.now.get();
            let (kind, api_token) = session_server(state);
            let session = state
                .session
                .with(|session| {
                    session
                        .as_ref()
                        .map(|session| (session.epoch(), session.overseer.clone()))
                })
                .filter(|_| rate_limits::lift_applies(kind, api_token));
            match session {
                Some((epoch, overseer)) => {
                    self.tick(now, epoch, &overseer, toasts, lift_again);
                }
                None => self.stop(toasts),
            }
        });
    }

    /// Forgets the lift: no session, or one it does not apply to.
    fn stop(self, toasts: Toasts) {
        let stale = self
            .monitor
            .try_update_value(|monitor| std::mem::take(monitor).toasted)
            .flatten();
        if let Some((_, toast)) = stale {
            toasts.dismiss(toast);
        }
        if self.exempt_until.with_untracked(Option::is_some) {
            self.exempt_until.set(None);
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

    /// One second of following session `epoch` at `now`.
    fn tick(
        self,
        now: u64,
        epoch: u64,
        overseer: &overseer::Overseer,
        toasts: Toasts,
        lift_again: Callback<()>,
    ) {
        let client = overseer.client();
        let rate_limited = client.rate_limited_count();
        let mut fresh = false;
        let due = self
            .monitor
            .try_update_value(|monitor| {
                if monitor.epoch != Some(epoch) {
                    *monitor = Monitor {
                        epoch: Some(epoch),
                        check_now: true,
                        rate_limited,
                        toasted: monitor.toasted,
                        ..Monitor::default()
                    };
                    fresh = true;
                }
                if rate_limited > monitor.rate_limited {
                    monitor.rate_limited = rate_limited;
                    monitor.after_rate_limited = true;
                }
                let due = !monitor.checking
                    && (monitor.check_now
                        || (monitor.after_rate_limited
                            && now.saturating_sub(monitor.last_check) >= RATE_LIMITED_DEBOUNCE_MS)
                        || monitor.next_check.is_some_and(|at| now >= at));
                if due {
                    monitor.checking = true;
                    monitor.check_now = false;
                    monitor.after_rate_limited = false;
                    monitor.next_check = None;
                    monitor.last_check = now;
                }
                due
            })
            .unwrap_or(false);
        if fresh {
            self.exempt_until.set(None);
            self.warning.set(None);
        }
        if due {
            self.check(epoch, overseer.account());
        }
        let lifted = self
            .exempt_until
            .get_untracked()
            .filter(|until| *until > now);
        // The ending toast belongs to one lift: gone once it ended or another replaced it.
        self.dismiss_toast(toasts, lifted);
        if let Some(until) = lifted {
            if let Some(text) = rate_limits::ending_label(until, now)
                && self.monitor.with_value(|monitor| monitor.toasted.is_none())
            {
                let toast = toasts.show_action(ToastKind::Info, text, "Lift again", lift_again);
                self.monitor
                    .update_value(|monitor| monitor.toasted = Some((until, toast)));
            }
            return;
        }
        // Not lifted (as far as known once a check completed: every check starts with a
        // `last_check`): prompt when an endpoint runs low.
        let ready = self
            .monitor
            .with_value(|monitor| monitor.last_check > 0 && !monitor.checking);
        if !ready || self.warning.with_untracked(Option::is_some) {
            return;
        }
        let limits = client.rate_limits();
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

    /// Checks the lift of session `epoch`'s token; a failure keeps what is known.
    fn check(self, epoch: u64, account: Account) {
        spawn_local(async move {
            let result = account.rate_limit_exemption().await;
            let now = epoch_millis();
            let live = self
                .monitor
                .try_update_value(|monitor| {
                    if monitor.epoch != Some(epoch) {
                        return false;
                    }
                    monitor.checking = false;
                    // While lifted: again now and then, and when it ends.
                    monitor.next_check = match result {
                        Ok(Some(until)) => Some(until.min(now + EXEMPT_RECHECK_MS)),
                        _ => None,
                    };
                    true
                })
                .unwrap_or(false);
            if let (true, Ok(until)) = (live, result) {
                self.exempt_until.set(until);
                if until.is_some() {
                    self.warning.set(None);
                }
            }
        });
    }

    /// `Not now` on the prompt for `endpoint`: no prompt for it again in this session.
    fn decline(self, endpoint: &str) {
        self.monitor
            .update_value(|monitor| monitor.warnings.decline(endpoint));
        self.warning.set(None);
    }
}

/// Now, in milliseconds since the Unix epoch.
fn epoch_millis() -> u64 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the epoch time in milliseconds is a non-negative integer far below u64::MAX"
    )]
    let now = js_sys::Date::now() as u64;
    now
}

/// The kind of the live session's server and whether it signed in with an API token
/// (reactive); a private server without a session.
fn session_server(state: AppState) -> (ServerKind, Option<bool>) {
    let server = state
        .session
        .with(|session| session.as_ref().map(|session| session.server.clone()));
    server
        .and_then(|url| {
            state.profiles.with(|profiles| {
                profiles.server(&url).map(|server| {
                    (
                        server.kind,
                        server.user.as_ref().and_then(|user| user.api_token),
                    )
                })
            })
        })
        .unwrap_or((ServerKind::Private, None))
}

/// The rate-limit actions of the live session (reactive).
pub fn session_actions(state: AppState, map: MapState, ui: RateLimitsUi) -> Actions {
    let paused = map
        .progress
        .with(|progress| progress.is_some_and(|progress| progress.stats_paused_until.is_some()));
    let (kind, api_token) = session_server(state);
    rate_limits::actions(kind, api_token, paused, ui.lifted_until().is_some())
}

/// Resumes the world's overviews paused by the rate limit, if any.
pub fn recheck(map: MapState) {
    map.feed.with_untracked(|feed| {
        if let Some(feed) = feed {
            feed.recheck_rate_limit();
        }
    });
}

/// Opens screeps.com's reset page for the live session's API token (its first 8 characters
/// only); failures go to the banner.
pub fn start_reset(state: AppState, ui: RateLimitsUi) {
    let url = state.session.with_untracked(|session| {
        session.as_ref().and_then(|session| {
            session
                .overseer
                .tokens()
                .borrow()
                .as_ref()
                .and_then(|token| rate_limits::reset_url(token.as_str()))
        })
    });
    let Some(url) = url else {
        state
            .banner
            .set(Some("No API token to reset the rate limits of.".to_owned()));
        return;
    };
    let platform = state.platform();
    if platform.is_tauri() {
        ui.awaiting_focus.set_value(true);
    }
    spawn_local(async move {
        if let Err(error) = platform.open_reset_page(&url).await {
            ui.awaiting_focus.set_value(false);
            state.banner.set(Some(format!(
                "Could not open screeps.com's reset page: {error}"
            )));
        }
    });
}

/// `Reset limit` in the status bar, while the world's overviews wait for a limit the reset
/// page can lift.
#[component]
pub fn StatusReset() -> impl IntoView {
    let state = AppState::expect();
    let map = MapState::expect();
    let ui = RateLimitsUi::expect();
    move || {
        session_actions(state, map, ui).status_reset.then(|| {
            view! {
                <span class="status-item">
                    <button
                        type="button"
                        class="link status-action"
                        title="Lift the rate limits of your API token on screeps.com"
                        on:click=move |_| start_reset(state, ui)
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
        ui.exempt_until
            .get()
            .and_then(|until| Some((until, rate_limits::lifted_label(until, ui.now.get())?)))
    });
    move || {
        label.get().map(|(until, label)| {
            view! {
                <span
                    class="status-item status-lifted"
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
    Effect::new(move |_| {
        if ui.open.get() {
            request_animation_frame(move || {
                if let Some(close) = close.get_untracked() {
                    let _ = close.focus();
                }
            });
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
                <p class="empty-state">
                    <Icon kind=IconKind::Check />
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
                        <th scope="row">
                            <code>{format!("{} /{}", endpoint.method, endpoint.path)}</code>
                        </th>
                        <td>{endpoint.limit.limit}</td>
                        <td>{endpoint.limit.remaining}</td>
                        <td title=local_time(reset)>{resets}</td>
                    </tr>
                }
            })
            .collect_view();
        view! {
            <table class="limits">
                <thead>
                    <tr>
                        <th scope="col">"Endpoint"</th>
                        <th scope="col">"Limit"</th>
                        <th scope="col">"Remaining"</th>
                        <th scope="col">"Resets"</th>
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
            <p class="notice lifted" role="status">
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
            <p class="notice">
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
            view! {
                <p class="help">
                    "screeps.com lets you lift your API token's limits early: its reset page asks you to solve a captcha. Only the first 8 characters of your token are sent, in the page's address; the full token never leaves Overseer."
                    {tauri
                        .then_some(
                            " The page opens in your browser; Overseer checks again when you come back.",
                        )}
                </p>
                <button type="button" class="primary" on:click=move |_| start_reset(state, ui)>
                    "Reset on screeps.com"
                </button>
            }
        });
        let check = actions.check_again.then(|| {
            view! {
                <button type="button" on:click=move |_| ui.lifted(map)>
                    "Check again"
                </button>
            }
        });
        view! { <div class="actions">{reset}{check}</div> }
    };
    move || {
        ui.open.get().then(|| {
            view! {
                <section
                    class="panel floating rate-limits"
                    role="dialog"
                    aria-labelledby="rate-limits-title"
                    on:keydown=keydown
                >
                    <div class="panel-head">
                        <h2 id="rate-limits-title">
                            <Icon kind=IconKind::Gauge />
                            "Rate limits"
                        </h2>
                        <button
                            type="button"
                            class="icon-button sm ghost"
                            node_ref=close
                            aria-label="Close rate limits"
                            data-tip="Close"
                            data-tip-pos="left"
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
/// for 2 hours` (the reset page) and `Not now` (Escape inside it too). Closing it while it
/// holds the focus gives the focus back to where it came from.
#[component]
pub fn LiftPrompt() -> impl IntoView {
    let state = AppState::expect();
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
        start_reset(state, ui);
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
                class="panel floating lift-prompt"
                role="dialog"
                aria-labelledby="lift-prompt-title"
                aria-describedby="lift-prompt-text"
                node_ref=dialog
                on:keydown=keydown
                on:focusin=focusin
            >
                <div class="panel-head">
                    <h2 id="lift-prompt-title">
                        <Icon kind=IconKind::Gauge />
                        "Rate limit running low"
                    </h2>
                </div>
                <p id="lift-prompt-text" title=local_time(reset)>{text}</p>
                <p class="help">
                    "screeps.com can lift your API token's rate limits for 2 hours: its reset page asks you to solve a captcha. Only the first 8 characters of your token are sent."
                </p>
                <div class="actions">
                    <button type="button" class="primary" on:click=move |_| lift()>
                        "Lift limits for 2 hours"
                    </button>
                    <button type="button" on:click=move |_| not_now()>
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
