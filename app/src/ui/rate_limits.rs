//! The live session's rate limits: the Rate limits panel (account menu), the status bar's
//! `Reset limit`, and screeps.com's reset flow ([`crate::rate_limits`]).
//!
//! The reset page opens as a popup in a browser and resumes the world's overviews on its
//! success message (from `https://screeps.com` only); in the shell it opens in the system
//! browser, which cannot talk back, so the app rechecks when its window regains focus. A
//! recheck ([`crate::feed::Feed::recheck_rate_limit`]) sends nothing unless the world's
//! overviews are paused by the limit.

use std::time::Duration;

use leptos::html::Button;
use leptos::prelude::*;
use leptos::task::spawn_local;
use web_sys::KeyboardEvent;

use crate::profiles::ServerKind;
use crate::rate_limits::{self, Actions};
use crate::state::AppState;
use crate::ui::icon::{Icon, IconKind};
use crate::ui::world::MapState;

/// The panel, the clock of relative times and the reset flow; `Copy`, provided as context.
#[derive(Clone, Copy)]
pub struct RateLimitsUi {
    /// The Rate limits panel is shown.
    pub open: RwSignal<bool>,
    /// Now, in milliseconds since the Unix epoch, updated every second.
    pub now: RwSignal<u64>,
    /// The shell opened the reset page in the system browser: recheck once the window
    /// regains focus.
    awaiting_focus: StoredValue<bool>,
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
            awaiting_focus: StoredValue::new(false),
        }
    }

    /// The state provided by [`crate::ui::App`].
    pub fn expect() -> Self {
        expect_context()
    }

    /// Rechecks after a reset: on the page's success message (browser) or when the window
    /// regains focus after the shell opened the page. For as long as the page lives.
    pub fn listen(self, map: MapState) {
        let message = window_event_listener(leptos::ev::message, move |event| {
            if rate_limits::is_reset_success(&event.origin(), event.data().as_string().as_deref()) {
                recheck(map);
            }
        });
        let focus = window_event_listener(leptos::ev::focus, move |_| {
            if self.awaiting_focus.get_value() {
                self.awaiting_focus.set_value(false);
                recheck(map);
            }
        });
        std::mem::forget(message);
        std::mem::forget(focus);
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

/// The rate-limit actions of the live session (reactive).
pub fn session_actions(state: AppState, map: MapState) -> Actions {
    let paused = map
        .progress
        .with(|progress| progress.is_some_and(|progress| progress.stats_paused_until.is_some()));
    let server = state
        .session
        .with(|session| session.as_ref().map(|session| session.server.clone()));
    let (kind, api_token) = server
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
        .unwrap_or((ServerKind::Private, None));
    rate_limits::actions(kind, api_token, paused)
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
        session_actions(state, map).status_reset.then(|| {
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

/// The Rate limits panel: the endpoints with a known limit, and the reset and recheck
/// actions where they apply.
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
        let actions = session_actions(state, map);
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
                <button type="button" on:click=move |_| recheck(map)>
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
                    {waiting}
                    {rows}
                    {actions}
                </section>
            }
        })
    }
}

/// `at` (milliseconds since the Unix epoch) as the browser's local date and time.
fn local_time(at: u64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "epoch milliseconds stay far below 2^53"
    )]
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(at as f64));
    String::from(date.to_locale_string("default", &wasm_bindgen::JsValue::UNDEFINED))
}
