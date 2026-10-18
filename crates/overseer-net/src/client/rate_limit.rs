//! Rate limits: the retry policy for HTTP 429 and the latest limit of each endpoint.

use std::collections::HashMap;
use std::time::Duration;

use http::Method;
use overseer_core::api::{Endpoint, RateLimit};

use crate::rt::{SystemTime, UNIX_EPOCH};

/// How requests rejected with HTTP 429 ([`ApiError::RateLimited`]) are retried.
///
/// A request that a limit refused is retried once after that limit's window resets
/// ([`RateLimit::reset`]), unless that is more than [`Self::max_wait`] away. This includes the
/// server-wide limit when the server reports it in the `X-RateLimit-*` headers, as screeps.com
/// does. A request refused without these headers is retried once after
/// [`Self::global_delay`]. A request that is rejected again returns the error. Set with
/// [`ClientBuilder::rate_limit`]; `None` there disables retries.
///
/// [`ApiError::RateLimited`]: overseer_core::api::ApiError::RateLimited
/// [`ClientBuilder::rate_limit`]: crate::ClientBuilder::rate_limit
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimitPolicy {
    /// Longest wait for an endpoint's window to reset.
    pub max_wait: Duration,
    /// Wait before retrying a request rejected by the server-wide limit.
    pub global_delay: Duration,
}

impl Default for RateLimitPolicy {
    /// Waits up to 60 s for an endpoint's window and 1 s after a server-wide rejection.
    fn default() -> Self {
        Self {
            max_wait: Duration::from_secs(60),
            global_delay: Duration::from_secs(1),
        }
    }
}

impl RateLimitPolicy {
    /// The wait before retrying a request rejected with `limit` at `now`; `None` when the
    /// window resets more than [`Self::max_wait`] after `now`. A reset in the past retries at
    /// once.
    #[must_use]
    pub fn retry_delay(&self, limit: Option<&RateLimit>, now: SystemTime) -> Option<Duration> {
        let Some(limit) = limit else {
            return Some(self.global_delay);
        };
        let wait = until_reset(limit, now);
        (wait <= self.max_wait).then_some(wait)
    }
}

/// The limit of each token per [`REQUEST_WINDOW`](super::REQUEST_WINDOW) over every endpoint.
/// screeps.com and screepsmod-auth report it as `X-RateLimit-Limit: 120` on the replies of
/// endpoints without their own limit and on the HTTP 429 that it causes. No endpoint limit has
/// this size: the endpoint limits are 60, 240, 360 and 1440.
const SERVER_WIDE_LIMIT: u32 = 120;

/// Whether the HTTP 429 that reported `limit` comes from the server-wide limit: it carries no
/// `X-RateLimit-*` headers (as the official contract describes it), or the headers of the
/// server-wide limit (as screeps.com sends them).
pub(crate) fn is_server_wide(limit: Option<&RateLimit>) -> bool {
    limit.is_none_or(|limit| limit.limit == SERVER_WIDE_LIMIT)
}

/// The time from `now` until the window of `limit` resets; zero for a reset in the past.
pub(crate) fn until_reset(limit: &RateLimit, now: SystemTime) -> Duration {
    let reset = UNIX_EPOCH + Duration::from_secs(limit.reset);
    reset.duration_since(now).unwrap_or(Duration::ZERO)
}

/// The latest rate limit the server reported for one endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointRateLimit {
    /// The endpoint's method.
    pub method: Method,
    /// The endpoint's path template ([`Endpoint::PATH`]).
    pub path: &'static str,
    /// The limit.
    pub limit: RateLimit,
}

/// The latest rate limit per endpoint (method and path template).
#[derive(Debug, Default)]
pub(crate) struct RateLimits(HashMap<(Method, &'static str), RateLimit>);

impl RateLimits {
    pub(crate) fn record<E: Endpoint>(&mut self, limit: RateLimit) {
        self.0.insert((E::METHOD, E::PATH), limit);
    }

    pub(crate) fn get<E: Endpoint>(&self) -> Option<RateLimit> {
        self.0.get(&(E::METHOD, E::PATH)).copied()
    }

    pub(crate) fn forget<E: Endpoint>(&mut self) {
        self.0.remove(&(E::METHOD, E::PATH));
    }

    /// Every recorded limit, ordered by path and method.
    pub(crate) fn all(&self) -> Vec<EndpointRateLimit> {
        let mut all: Vec<_> = self
            .0
            .iter()
            .map(|((method, path), limit)| EndpointRateLimit {
                method: method.clone(),
                path,
                limit: *limit,
            })
            .collect();
        all.sort_unstable_by(|a, b| (a.path, a.method.as_str()).cmp(&(b.path, b.method.as_str())));
        all
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use overseer_core::api::RateLimit;

    use super::RateLimitPolicy;
    use crate::rt::UNIX_EPOCH;

    const fn limit(reset: u64) -> RateLimit {
        RateLimit {
            limit: 60,
            remaining: 0,
            reset,
        }
    }

    #[test]
    fn endpoint_limits_wait_for_the_reset_within_the_bound() {
        let policy = RateLimitPolicy::default();
        let now = UNIX_EPOCH + Duration::from_millis(1_000_250);
        assert_eq!(
            policy.retry_delay(Some(&limit(1_010)), now),
            Some(Duration::from_millis(9_750))
        );
        assert_eq!(
            policy.retry_delay(Some(&limit(990)), now),
            Some(Duration::ZERO)
        );
        assert_eq!(
            policy.retry_delay(Some(&limit(1_060)), now),
            Some(Duration::from_millis(59_750))
        );
        assert_eq!(policy.retry_delay(Some(&limit(1_061)), now), None);
    }

    #[test]
    fn global_limits_wait_the_fixed_delay() {
        let policy = RateLimitPolicy {
            max_wait: Duration::ZERO,
            global_delay: Duration::from_millis(300),
        };
        assert_eq!(
            policy.retry_delay(None, UNIX_EPOCH),
            Some(Duration::from_millis(300))
        );
    }
}
