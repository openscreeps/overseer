//! Lifting the rate limits of an API token ([`RateLimitLift`]): the reset page of screeps.com,
//! the request of screepsmod-auth and [`Api::rate_limit_lift`], which selects one.

use overseer_core::Token;
use overseer_core::api::plugins::screepsmod_auth;

use super::super::Api;
use crate::{Client, Error};

/// How many leading characters of an API token lifting its rate limits sends: screeps.com's
/// reset page and screepsmod-auth's `POST /api/user/noratelimit` both take a token prefix (the
/// account page sends 8).
pub const LIFT_TOKEN_PREFIX: usize = 8;

/// screeps.com's reset page, up to the token prefix (the page of the token list and of
/// node-screeps-api's `rateLimitResetUrl`).
const OFFICIAL_LIFT_PAGE: &str = "https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=";
/// The origin of screeps.com's reset page.
const OFFICIAL_ORIGIN: &str = "https://screeps.com";
/// What screeps.com's reset page posts to its opener once the limits are lifted.
const OFFICIAL_LIFT_SUCCESS: &str = "screepsTokenSuccess";

/// The first [`LIFT_TOKEN_PREFIX`] characters of `token`; `None` for a token not longer than
/// that, which would go out whole.
fn lift_prefix(token: &str) -> Option<&str> {
    let (end, _) = token.char_indices().nth(LIFT_TOKEN_PREFIX)?;
    token.get(..end)
}

/// How the rate limits of an API token are lifted on this server
/// ([`Subsystem::RateLimitLift`](crate::model::platform::Subsystem::RateLimitLift)); from
/// [`Api::rate_limit_lift`]. Either way the lift lasts about two hours, and the token's entry
/// in [`ApiTokens::auth_tokens`](super::ApiTokens::auth_tokens) tells until when
/// (`noRatelimitUntil`).
#[derive(Clone, Debug)]
pub enum RateLimitLift {
    /// The user lifts them on a web page (screeps.com: a reCAPTCHA).
    Page(LiftPage),
    /// One request lifts them (screepsmod-auth).
    Request(LiftRequest),
}

/// The web page where the user lifts an API token's rate limits ([`RateLimitLift::Page`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiftPage {
    /// The page for the token, holding only its prefix. Open it in a browser (a popup in a web
    /// page, so that it can report back).
    pub url: String,
    /// The origin the page posts its result from.
    pub origin: &'static str,
    /// What the page posts (as a string message) to the window that opened it once the limits
    /// are lifted; it then closes.
    pub success: &'static str,
}

impl LiftPage {
    /// Whether a `message` event from `origin` carrying `data` says the page lifted the
    /// limits.
    #[must_use]
    pub fn is_success(&self, origin: &str, data: Option<&str>) -> bool {
        origin == self.origin && data == Some(self.success)
    }
}

/// The request lifting an API token's rate limits ([`RateLimitLift::Request`]).
#[derive(Clone)]
pub struct LiftRequest {
    client: Client,
    /// The token's first [`LIFT_TOKEN_PREFIX`] characters.
    prefix: String,
}

impl std::fmt::Debug for LiftRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiftRequest")
            .field("prefix", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl LiftRequest {
    /// Lifts the token's rate limits (screepsmod-auth: `POST /api/user/noratelimit` with the
    /// token prefix, signed in).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`]: screepsmod-auth answers HTTP 404 for a prefix
    /// of no token of the account and HTTP 400 when the exemption fails.
    pub async fn send(&self) -> Result<(), Error> {
        let lift = screepsmod_auth::endpoints::UserNoRateLimit {
            body: screepsmod_auth::types::UserNoRateLimitRequest {
                token: self.prefix.clone(),
            },
        };
        self.client.send(&lift).await?;
        Ok(())
    }
}

impl Api {
    /// How the rate limits of the API token `token` are lifted, when the server lets them be
    /// ([`Subsystem::RateLimitLift`](crate::model::platform::Subsystem::RateLimitLift)): on
    /// screeps.com's reset page ([`RateLimitLift::Page`], a reCAPTCHA), with screepsmod-auth by
    /// one request ([`RateLimitLift::Request`]). Only the token's first [`LIFT_TOKEN_PREFIX`]
    /// characters are used; `None` as well for a token no longer than that, which would go out
    /// whole. Session tokens (from a password sign-in) match no API token: lifting applies to
    /// API-token sessions only.
    #[must_use]
    pub fn rate_limit_lift(&self, token: &Token) -> Option<RateLimitLift> {
        let prefix = lift_prefix(token.as_str())?;
        match self {
            Self::Official(_) => {
                let encoded: String =
                    url::form_urlencoded::byte_serialize(prefix.as_bytes()).collect();
                Some(RateLimitLift::Page(LiftPage {
                    url: format!("{OFFICIAL_LIFT_PAGE}{encoded}"),
                    origin: OFFICIAL_ORIGIN,
                    success: OFFICIAL_LIFT_SUCCESS,
                }))
            }
            Self::OpenScreeps(_) => None,
            Self::Community(api) => api.screepsmod_auth().map(|auth| {
                RateLimitLift::Request(LiftRequest {
                    client: auth.client.clone(),
                    prefix: prefix.to_owned(),
                })
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use overseer_core::model::platform::{Plugins, ServerKind};
    use overseer_core::{ServerUrl, Token};

    use super::RateLimitLift;
    use crate::Client;
    use crate::api::Api;

    #[test]
    fn the_lift_page_gets_exactly_the_first_8_characters() {
        let client = Client::builder(ServerUrl::official()).build().unwrap();
        let api = Api::new(client, ServerKind::Official, Plugins::NONE);
        let page = |token: &str| match api.rate_limit_lift(&Token::new(token)) {
            Some(RateLimitLift::Page(page)) => Some(page.url),
            _ => None,
        };
        let token = "0123abcd-4567-89ef-0123-456789abcdef";
        let url = page(token).unwrap();
        assert_eq!(
            url,
            "https://screeps.com/a/#!/account/auth-tokens/noratelimit?token=0123abcd"
        );
        assert!(!url.contains("0123abcd-"));
        // Never the whole token: one of 8 characters or fewer has no page.
        assert_eq!(page("0123abcd"), None);
        assert_eq!(page(""), None);
        assert!(page("0123abcde").is_some_and(|url| url.ends_with("=0123abcd")));
        // Characters, not bytes; anything outside URL-safe ones is escaped.
        assert!(
            page("ééé a&b#cd").is_some_and(|url| url.ends_with("=%C3%A9%C3%A9%C3%A9+a%26b%23"))
        );
    }
}
