//! Per-token rate limits: how long a pause lasts, lifting them, and where its actions apply.
//!
//! screeps.com limits each API token per endpoint (map stats: 60 requests per hour) and lets
//! the user lift the limits early; so does screepsmod-auth on community servers. Whether and
//! how is the server's [`Subsystem::RateLimitLift`](overseer::model::platform::Subsystem):
//! [`overseer::Account::rate_limit_lift`] answers a web page (screeps.com: the user solves a
//! reCAPTCHA there, and the page posts its success to the window that opened it) or a request
//! (screepsmod-auth). Only the token's first 8 characters leave the app. It applies to API
//! tokens only: password sign-ins hold session tokens, which match no API token.
//!
//! A lift lasts about two hours. The session checks until when
//! ([`overseer::RateLimits::follow`], `noRatelimitUntil`): the app follows it in the main
//! window, tells it when the user's attention comes back, and gives it the world's loading
//! state. While lifted the app says so and how long is left, and [`EXEMPT_WARNING_MS`] before
//! the end offers to lift them again. While not lifted, an endpoint running low ([`is_low`])
//! brings a prompt to lift them, at most once per endpoint and window ([`Warnings`]).

use std::collections::HashSet;

use overseer::core::api::RateLimit;
use overseer::map::STATS_FAR_SHARE;

/// The host of a lift page `url` for the texts naming it (`screeps.com`); `the website` when
/// it has none.
pub fn page_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "the website".to_owned())
}

/// The rate-limit actions a session offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    /// `Reset limit` in the status bar: the world's overviews wait for the limit, lifting
    /// applies and the limits are not lifted.
    pub status_reset: bool,
    /// The lift action in the Rate limits panel (`Reset on screeps.com`): lifting applies and
    /// the limits are not lifted.
    pub reset: bool,
    /// `Check again` in the Rate limits panel: the world's overviews wait for the limit.
    pub check_again: bool,
}

/// Whether lifting the rate limits (and the lifted state) applies to a session on a server
/// that `lifts` them ([`Subsystem::RateLimitLift`](overseer::model::platform::Subsystem)),
/// signed in with an API token or not
/// ([`SavedUser::api_token`](overseer_storage::profiles::SavedUser::api_token); `None`,
/// recorded before the app tracked it, keeps it since lifting reports a token it does not
/// know): lifting applies to API tokens only.
pub const fn lift_applies(lifts: bool, api_token: Option<bool>) -> bool {
    lifts && !matches!(api_token, Some(false))
}

/// Whether a window follows the lift of its session (checks it, shows it, prompts): the main
/// window (not a `popout`) of a session the lift applies to ([`lift_applies`]).
pub const fn follows_lift(lifts: bool, api_token: Option<bool>, popout: bool) -> bool {
    !popout && lift_applies(lifts, api_token)
}

/// The actions of a session on a server that `lifts` rate limits or not, signed in with
/// `api_token` (see [`lift_applies`]), with the world's overview loading `paused` by the rate
/// limit or not, and the token's limits `lifted` or not.
pub const fn actions(lifts: bool, api_token: Option<bool>, paused: bool, lifted: bool) -> Actions {
    let reset = lift_applies(lifts, api_token) && !lifted;
    Actions {
        status_reset: reset && paused,
        reset,
        check_again: paused,
    }
}

/// How long before the end of a lift the app offers to lift the limits again (10 minutes).
pub const EXEMPT_WARNING_MS: u64 = 10 * 60_000;

/// Percentage of an endpoint's window under which it runs low (map stats aside, see
/// [`is_low`]).
pub const LOW_PERCENT: u64 = 20;

/// The path of the map-stats endpoint, whose budget the world map shares out.
const MAP_STATS: &str = "api/game/map-stats";

/// Whether the endpoint `path` with `limit` runs low: map stats once down to the share the
/// world map keeps for near rooms (30 %, where far rooms stop loading), any other endpoint
/// below [`LOW_PERCENT`] of its window. A refusal (HTTP 429, nothing left) always is.
pub fn is_low(path: &str, limit: RateLimit) -> bool {
    let (total, remaining) = (u64::from(limit.limit), u64::from(limit.remaining));
    if remaining == 0 {
        return true;
    }
    if path == MAP_STATS {
        let reserve = total - total * u64::from(STATS_FAR_SHARE) / 100;
        remaining <= reserve
    } else {
        remaining * 100 < total * LOW_PERCENT
    }
}

/// What the user knows an endpoint as: `map stats` for the world map's endpoint, else its
/// method and path (`GET /api/game/room-objects`).
pub fn endpoint_name(method: &str, path: &str) -> String {
    if path == MAP_STATS {
        "map stats".to_owned()
    } else {
        format!("{method} /{path}")
    }
}

/// An endpoint running low, for the prompt to lift the limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    /// Method and path (`POST /api/game/map-stats`): identifies the endpoint.
    pub endpoint: String,
    /// What the user knows it as ([`endpoint_name`]).
    pub name: String,
    /// Requests per window.
    pub limit: u32,
    /// Requests left in the window.
    pub remaining: u32,
    /// When the window resets, in milliseconds since the Unix epoch.
    pub reset: u64,
}

impl Warning {
    /// `Only 18 of 60 map stats requests are left until the limit resets in 25 min.`, or
    /// `No map stats request is left …` once none is.
    pub fn text(&self, now: u64) -> String {
        let resets = time_until(self.reset, now)
            .map_or_else(|| "resets".to_owned(), |left| format!("resets in {left}"));
        if self.remaining == 0 {
            format!("No {} request is left until the limit {resets}.", self.name)
        } else {
            format!(
                "Only {} of {} {} requests are left until the limit {resets}.",
                self.remaining, self.limit, self.name
            )
        }
    }
}

/// The prompts already shown in a session: at most one per endpoint and rate-limit window,
/// and none for an endpoint once the user answered `Not now`.
#[derive(Debug, Default)]
pub struct Warnings {
    /// Endpoints and the window (its reset, seconds since the Unix epoch) prompted for.
    shown: HashSet<(String, u64)>,
    /// Endpoints the user does not want to hear about again in this session.
    declined: HashSet<String>,
}

impl Warnings {
    /// The first endpoint of `limits` (method, path template as in
    /// [`EndpointRateLimit`](overseer::net::EndpointRateLimit), limit) running low
    /// ([`is_low`]) in a window not yet over at `now` (milliseconds since the Unix epoch) that
    /// was not prompted for in that window nor declined; it counts as shown.
    pub fn next<'a>(
        &mut self,
        limits: impl IntoIterator<Item = (&'a str, &'a str, RateLimit)>,
        now: u64,
    ) -> Option<Warning> {
        let (method, path, limit, endpoint) = limits
            .into_iter()
            .map(|(method, path, limit)| (method, path, limit, format!("{method} /{path}")))
            .find(|(_, path, limit, endpoint)| {
                limit.reset.saturating_mul(1000) > now
                    && is_low(path, *limit)
                    && !self.declined.contains(endpoint)
                    && !self.shown.contains(&(endpoint.clone(), limit.reset))
            })?;
        self.shown.insert((endpoint.clone(), limit.reset));
        Some(Warning {
            name: endpoint_name(method, path),
            endpoint,
            limit: limit.limit,
            remaining: limit.remaining,
            reset: limit.reset.saturating_mul(1000),
        })
    }

    /// The user answered `Not now` for `endpoint`: no prompt for it again in this session.
    pub fn decline(&mut self, endpoint: &str) {
        self.declined.insert(endpoint.to_owned());
    }
}

/// The time from `now` until `until` (milliseconds since the Unix epoch), rounded up: seconds
/// under a minute (`42 s`), minutes under an hour (`12 min`), else hours and minutes
/// (`1 h 5 min`). `None` once `until` has passed.
pub fn time_until(until: u64, now: u64) -> Option<String> {
    let left = until.checked_sub(now).filter(|left| *left > 0)?;
    let seconds = left.div_ceil(1000);
    if seconds < 60 {
        return Some(format!("{seconds} s"));
    }
    let minutes = seconds.div_ceil(60);
    if minutes < 60 {
        return Some(format!("{minutes} min"));
    }
    let (hours, minutes) = (minutes / 60, minutes % 60);
    Some(if minutes == 0 {
        format!("{hours} h")
    } else {
        format!("{hours} h {minutes} min")
    })
}

/// The status bar's note while the world's overviews wait for the limit that resets at
/// `until`: `waiting for the rate limit, resumes in 12 min`.
pub fn waiting_label(until: u64, now: u64) -> String {
    time_until(until, now).map_or_else(
        || "waiting for the rate limit, resuming".to_owned(),
        |left| format!("waiting for the rate limit, resumes in {left}"),
    )
}

/// The status bar's item while the token's limits are lifted until `until`:
/// `Rate limits lifted · 1 h 52 min`; `None` once the lift has ended.
pub fn lifted_label(until: u64, now: u64) -> Option<String> {
    time_until(until, now).map(|left| format!("Rate limits lifted · {left}"))
}

/// The toast [`EXEMPT_WARNING_MS`] before a lift ending at `until` ends:
/// `Your rate limits return in 10 min.`; `None` while more is left, and once it has ended.
pub fn ending_label(until: u64, now: u64) -> Option<String> {
    if until.saturating_sub(now) > EXEMPT_WARNING_MS {
        return None;
    }
    time_until(until, now).map(|left| format!("Your rate limits return in {left}."))
}

#[cfg(test)]
mod tests {
    use overseer::core::api::RateLimit;

    use super::{
        Actions, Warning, Warnings, actions, ending_label, follows_lift, is_low, lift_applies,
        lifted_label, page_host, time_until, waiting_label,
    };

    #[test]
    fn lift_texts_name_the_pages_host() {
        assert_eq!(
            page_host("https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=0123abcd"),
            "screeps.com"
        );
        assert_eq!(page_host("not a url"), "the website");
    }

    #[test]
    fn reset_actions_apply_to_api_tokens_on_servers_lifting_rate_limits() {
        // API tokens, and sign-ins saved before the app recorded how they signed in.
        for api_token in [Some(true), None] {
            assert!(lift_applies(true, api_token));
            assert_eq!(
                actions(true, api_token, true, false),
                Actions {
                    status_reset: true,
                    reset: true,
                    check_again: true,
                }
            );
            assert_eq!(
                actions(true, api_token, false, false),
                Actions {
                    status_reset: false,
                    reset: true,
                    check_again: false,
                }
            );
            // Lifted: nothing to reset.
            assert_eq!(
                actions(true, api_token, true, true),
                Actions {
                    status_reset: false,
                    reset: false,
                    check_again: true,
                }
            );
        }
        // Password sign-ins hold session tokens; other servers do not lift rate limits.
        for (lifts, api_token) in [
            (true, Some(false)),
            (false, Some(true)),
            (false, Some(false)),
            (false, None),
        ] {
            assert!(!lift_applies(lifts, api_token));
            assert_eq!(
                actions(lifts, api_token, true, false),
                Actions {
                    status_reset: false,
                    reset: false,
                    check_again: true,
                }
            );
            assert_eq!(actions(lifts, api_token, false, false), Actions::default());
        }
    }

    const fn limit(limit: u32, remaining: u32, reset: u64) -> RateLimit {
        RateLimit {
            limit,
            remaining,
            reset,
        }
    }

    #[test]
    fn map_stats_run_low_at_the_far_share_others_below_20_percent() {
        let map_stats = "api/game/map-stats";
        // 60 per hour: far rooms stop at 18 left (30 %).
        assert!(!is_low(map_stats, limit(60, 19, 0)));
        assert!(is_low(map_stats, limit(60, 18, 0)));
        assert!(is_low(map_stats, limit(60, 1, 0)));
        assert!(!is_low(map_stats, limit(100, 31, 0)));
        assert!(is_low(map_stats, limit(100, 30, 0)));
        let objects = "api/game/room-objects";
        assert!(!is_low(objects, limit(60, 12, 0)));
        assert!(is_low(objects, limit(60, 11, 0)));
        assert!(!is_low(objects, limit(100, 20, 0)));
        assert!(is_low(objects, limit(100, 19, 0)));
        // A refusal: nothing left, whatever the window.
        assert!(is_low(objects, limit(0, 0, 0)));
        assert!(is_low(objects, limit(1, 0, 0)));
        assert!(!is_low(objects, limit(1, 1, 0)));
    }

    #[test]
    fn each_endpoint_prompts_once_per_window_and_never_after_not_now() {
        let now = 1_700_000_000_000;
        let reset = now / 1000 + 1500;
        let mut warnings = Warnings::default();
        let healthy = [("POST", "api/game/map-stats", limit(60, 40, reset))];
        assert_eq!(warnings.next(healthy, now), None);
        let low = [
            ("GET", "api/game/room-objects", limit(60, 30, reset)),
            ("POST", "api/game/map-stats", limit(60, 18, reset)),
        ];
        let warning = warnings.next(low, now).unwrap();
        assert_eq!(
            warning,
            Warning {
                endpoint: "POST /api/game/map-stats".to_owned(),
                name: "map stats".to_owned(),
                limit: 60,
                remaining: 18,
                reset: reset * 1000,
            }
        );
        assert_eq!(
            warning.text(now),
            "Only 18 of 60 map stats requests are left until the limit resets in 25 min."
        );
        // Once per window, however low it goes.
        assert_eq!(warnings.next(low, now), None);
        let refused = [("POST", "api/game/map-stats", limit(60, 0, reset))];
        assert_eq!(warnings.next(refused, now), None);
        // A past window prompts nothing.
        let stale = [("GET", "api/game/room-objects", limit(60, 0, now / 1000))];
        assert_eq!(warnings.next(stale, now), None);
        // The next window prompts again, and other endpoints have their own.
        let next = [
            ("POST", "api/game/map-stats", limit(60, 0, reset + 3600)),
            ("GET", "api/game/room-objects", limit(60, 0, reset)),
        ];
        let warning = warnings.next(next, now).unwrap();
        assert_eq!(warning.endpoint, "POST /api/game/map-stats");
        assert_eq!(
            warning.text(now),
            "No map stats request is left until the limit resets in 1 h 25 min."
        );
        let warning = warnings.next(next, now).unwrap();
        assert_eq!(warning.name, "GET /api/game/room-objects");
        assert_eq!(warnings.next(next, now), None);
        // `Not now`: never again in this session.
        warnings.decline("POST /api/game/map-stats");
        let later = [("POST", "api/game/map-stats", limit(60, 0, reset + 7200))];
        assert_eq!(warnings.next(later, now), None);
        let other = [("GET", "api/game/room-objects", limit(60, 0, reset + 7200))];
        assert!(warnings.next(other, now).is_some());
    }

    #[test]
    fn only_main_windows_of_api_token_sessions_on_lifting_servers_follow_the_lift() {
        for api_token in [Some(true), None] {
            assert!(follows_lift(true, api_token, false));
            assert!(!follows_lift(true, api_token, true));
        }
        for (lifts, api_token) in [(true, Some(false)), (false, Some(true)), (false, None)] {
            for popout in [false, true] {
                assert!(!follows_lift(lifts, api_token, popout));
            }
        }
    }

    #[test]
    fn lifted_labels_count_down_and_end() {
        let now = 1_700_000_000_000;
        let until = now + 7_200_000;
        assert_eq!(
            lifted_label(until, now).as_deref(),
            Some("Rate limits lifted · 2 h")
        );
        assert_eq!(
            lifted_label(until, now + 400_000).as_deref(),
            Some("Rate limits lifted · 1 h 54 min")
        );
        assert_eq!(
            lifted_label(until, until - 30_000).as_deref(),
            Some("Rate limits lifted · 30 s")
        );
        assert_eq!(lifted_label(until, until), None);
        // The toast: from 10 minutes before the end.
        assert_eq!(ending_label(until, now), None);
        assert_eq!(ending_label(until, until - 600_001), None);
        assert_eq!(
            ending_label(until, until - 600_000).as_deref(),
            Some("Your rate limits return in 10 min.")
        );
        assert_eq!(
            ending_label(until, until - 1000).as_deref(),
            Some("Your rate limits return in 1 s.")
        );
        assert_eq!(ending_label(until, until), None);
    }

    #[test]
    fn times_left_round_up_to_seconds_minutes_and_hours() {
        let now = 1_700_000_000_000;
        assert_eq!(time_until(now, now), None);
        assert_eq!(time_until(now - 1, now), None);
        assert_eq!(time_until(now + 1, now).as_deref(), Some("1 s"));
        assert_eq!(time_until(now + 42_000, now).as_deref(), Some("42 s"));
        assert_eq!(time_until(now + 59_000, now).as_deref(), Some("59 s"));
        assert_eq!(time_until(now + 59_001, now).as_deref(), Some("1 min"));
        assert_eq!(time_until(now + 60_000, now).as_deref(), Some("1 min"));
        assert_eq!(time_until(now + 61_000, now).as_deref(), Some("2 min"));
        assert_eq!(time_until(now + 3_540_000, now).as_deref(), Some("59 min"));
        assert_eq!(time_until(now + 3_541_000, now).as_deref(), Some("1 h"));
        assert_eq!(time_until(now + 3_600_000, now).as_deref(), Some("1 h"));
        assert_eq!(
            time_until(now + 3_900_000, now).as_deref(),
            Some("1 h 5 min")
        );
        assert_eq!(
            waiting_label(now + 1_800_000, now),
            "waiting for the rate limit, resumes in 30 min"
        );
        assert_eq!(
            waiting_label(now, now),
            "waiting for the rate limit, resuming"
        );
    }
}
