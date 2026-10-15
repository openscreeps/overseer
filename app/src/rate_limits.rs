//! Per-token rate limits: how long a pause lasts, screeps.com's official reset page, and where
//! its actions apply.
//!
//! screeps.com limits each API token per endpoint (map stats: 60 requests per hour) and lets
//! the user lift the limits early on its reset page
//! (`https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=<first 8 characters>`,
//! the page of the token list and of node-screeps-api's `rateLimitResetUrl`). The user solves
//! a reCAPTCHA there; the page then posts [`RESET_SUCCESS`] to the window that opened it and
//! closes. Only the token's first 8 characters leave the app, in the page's address; the page
//! sends them with the captcha (`POST /api/user/noratelimit`). It applies to API tokens only:
//! password sign-ins hold session tokens, which the page does not know.

use crate::profiles::ServerKind;

/// The reset page, up to the token prefix.
const RESET_PAGE: &str = "https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=";

/// How many leading characters of the token the reset page gets.
pub const TOKEN_PREFIX_CHARS: usize = 8;

/// The origin of the reset page's message.
pub const RESET_ORIGIN: &str = "https://screeps.com";

/// What the reset page posts to its opener once the limits are lifted.
pub const RESET_SUCCESS: &str = "screepsTokenSuccess";

/// The reset page for `token`: its first [`TOKEN_PREFIX_CHARS`] characters only. `None` for
/// a token not longer than that, which would go out whole.
pub fn reset_url(token: &str) -> Option<String> {
    let (end, _) = token.char_indices().nth(TOKEN_PREFIX_CHARS)?;
    let prefix: String =
        url::form_urlencoded::byte_serialize(token.get(..end)?.as_bytes()).collect();
    Some(format!("{RESET_PAGE}{prefix}"))
}

/// Whether a `message` event from `origin` carrying `data` says the reset page lifted the
/// limits.
pub fn is_reset_success(origin: &str, data: Option<&str>) -> bool {
    origin == RESET_ORIGIN && data == Some(RESET_SUCCESS)
}

/// The rate-limit actions a session offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    /// `Reset limit` in the status bar: the world's overviews wait for the limit, and the
    /// reset page applies.
    pub status_reset: bool,
    /// `Reset on screeps.com` in the Rate limits panel: the reset page applies.
    pub reset: bool,
    /// `Check again` in the Rate limits panel: the world's overviews wait for the limit.
    pub check_again: bool,
}

/// The actions of a session on a server of `kind`, signed in with an API token or not
/// ([`SavedUser::api_token`](crate::profiles::SavedUser::api_token); `None`, recorded before
/// the app tracked it, keeps the reset available since the page reports a token it does not
/// know), with the world's overview loading `paused` by the rate limit or not. The reset page
/// lifts the limits of screeps.com API tokens only.
pub const fn actions(kind: ServerKind, api_token: Option<bool>, paused: bool) -> Actions {
    let reset = matches!(kind, ServerKind::Official) && !matches!(api_token, Some(false));
    Actions {
        status_reset: reset && paused,
        reset,
        check_again: paused,
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

#[cfg(test)]
mod tests {
    use super::{Actions, actions, is_reset_success, reset_url, time_until, waiting_label};
    use crate::profiles::ServerKind;

    #[test]
    fn the_reset_page_gets_exactly_the_first_8_characters() {
        let token = "0123abcd-4567-89ef-0123-456789abcdef";
        let url = reset_url(token).unwrap();
        assert_eq!(
            url,
            "https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=0123abcd"
        );
        let (_, prefix) = url.split_once("token=").unwrap();
        assert_eq!(prefix.chars().count(), 8);
        assert!(!url.contains(token));
        assert!(!url.contains("0123abcd-"));
        // Never the whole token: one of 8 characters or fewer has no reset page.
        assert_eq!(reset_url("0123abcd"), None);
        assert_eq!(reset_url("abc"), None);
        assert_eq!(reset_url(""), None);
        assert!(reset_url("0123abcde").is_some_and(|url| url.ends_with("=0123abcd")));
        // Characters, not bytes; anything outside URL-safe ones is escaped.
        assert!(
            reset_url("ééé a&b#cd")
                .is_some_and(|url| url.ends_with("=%C3%A9%C3%A9%C3%A9+a%26b%23"))
        );
    }

    #[test]
    fn only_the_reset_page_success_from_screeps_com_counts() {
        assert!(is_reset_success(
            "https://screeps.com",
            Some("screepsTokenSuccess")
        ));
        assert!(!is_reset_success(
            "https://screeps.com.evil.example",
            Some("screepsTokenSuccess")
        ));
        assert!(!is_reset_success(
            "http://screeps.com",
            Some("screepsTokenSuccess")
        ));
        assert!(!is_reset_success("https://screeps.com", Some("other")));
        assert!(!is_reset_success("https://screeps.com", None));
    }

    #[test]
    fn reset_actions_apply_to_screeps_com_api_tokens_only() {
        // API tokens, and sign-ins saved before the app recorded how they signed in.
        for api_token in [Some(true), None] {
            assert_eq!(
                actions(ServerKind::Official, api_token, true),
                Actions {
                    status_reset: true,
                    reset: true,
                    check_again: true,
                }
            );
            assert_eq!(
                actions(ServerKind::Official, api_token, false),
                Actions {
                    status_reset: false,
                    reset: true,
                    check_again: false,
                }
            );
        }
        // Password sign-ins hold session tokens; other servers have no reset page.
        for (kind, api_token) in [
            (ServerKind::Official, Some(false)),
            (ServerKind::Season, Some(true)),
            (ServerKind::Season, None),
            (ServerKind::Private, Some(true)),
            (ServerKind::Private, None),
        ] {
            assert_eq!(
                actions(kind, api_token, true),
                Actions {
                    status_reset: false,
                    reset: false,
                    check_again: true,
                }
            );
            assert_eq!(actions(kind, api_token, false), Actions::default());
        }
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
