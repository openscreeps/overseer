//! The signed-in user's account: profile, overview statistics, world status, start room, respawn, CPU and its allocation between shards, resources, badge, e-mail, username and password, notification preferences, API tokens and power creeps.

mod power_creeps;

use std::collections::BTreeMap;
use std::sync::Arc;

use overseer_net::api::{
    AccountApi as _, ApiTokens as _, ApiTokensApi, PasswordAccount as _, RateLimitLift,
};
use overseer_net::core::Token;
use overseer_net::core::socket::{Channel, Payload};
use overseer_net::model::account::{
    AuthToken, AuthTokenRequest, Overview, Profile, StartRoom, UserBadgeInput, UserNotifyPrefs,
    UserRoomStatName, UserWorldStatus,
};
use overseer_net::model::platform::Subsystem;
use overseer_net::model::socket::{UserCpu, UserResources};
use overseer_net::rt::{SystemTime, UNIX_EPOCH};

use crate::rooms::StatInterval;
use crate::{Error, Overseer, Watch};

pub use power_creeps::PowerCreeps;

/// The signed-in user's account: profile, overview statistics, world status, start room, respawn, CPU and its allocation between shards, resources, badge, e-mail, username and password, notification preferences, API tokens and power creeps.
///
/// Created by [`Overseer::account`]; cheap to clone. Every operation needs a signed-in user
/// and fails with [`Error::Anonymous`] otherwise, and with [`Error::Closed`] after
/// [`Overseer::shutdown`]. Placing the first spawn is
/// [`Rooms::place_spawn`](crate::Rooms::place_spawn).
#[derive(Clone, Debug)]
pub struct Account {
    overseer: Overseer,
}

impl Account {
    pub(crate) fn new(overseer: Overseer) -> Self {
        Self { overseer }
    }

    /// The signed-in user's private profile (`GET /api/auth/me`): username, badge, credits,
    /// CPU, GCL and power, resources, notification preferences, …
    ///
    /// Read once per session (by [`Overseer::connect`] while discovering the server, so the
    /// first call makes no request) and then served from a cache shared by every handle;
    /// [`Self::refresh`] fetches it again. The account's own changes made through this crate
    /// ([`Self::set_badge`], [`Self::set_email`], [`Self::set_username`],
    /// [`Self::set_notify_prefs`], [`Self::set_cpu_shards`], [`Self::respawn`],
    /// [`PowerCreeps::start_experimentation`]) clear the cache, so the next call fetches it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn me(&self) -> Result<Arc<Profile>, Error> {
        let api = self.overseer.user_api()?;
        let mut cached = self.overseer.me_cache().lock().await;
        if let Some(me) = &*cached {
            return Ok(Arc::clone(me));
        }
        let me = Arc::new(api.me().await?);
        *cached = Some(Arc::clone(&me));
        Ok(me)
    }

    /// Fetches the signed-in user's profile again, replaces the cached one ([`Self::me`]) and
    /// returns it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails; the cache is kept then.
    pub async fn refresh(&self) -> Result<Arc<Profile>, Error> {
        let api = self.overseer.user_api()?;
        let mut cached = self.overseer.me_cache().lock().await;
        let me = Arc::new(api.me().await?);
        *cached = Some(Arc::clone(&me));
        Ok(me)
    }

    /// Per-room series of `stat` over `interval` and their totals, for the overview page
    /// (`GET /api/user/overview`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn overview(
        &self,
        interval: StatInterval,
        stat: UserRoomStatName,
    ) -> Result<Overview, Error> {
        let api = self.overseer.user_api()?;
        Ok(api.overview(interval.minutes(), stat).await?)
    }

    /// Whether the user plays (`normal`), has lost every spawn (`lost`) or has nothing in the
    /// world and must place a spawn (`empty`), via `GET /api/user/world-status`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn world_status(&self) -> Result<UserWorldStatus, Error> {
        Ok(self.overseer.user_api()?.world_status().await?)
    }

    /// The room to center the world map on when it opens (`GET /api/user/world-start-room`):
    /// one of the user's rooms, else the server's default start room; `None` when the server
    /// names no room. With `shard: None` a sharded server also chooses the shard (the user's,
    /// not the default one); with `Some(shard)` the room is on that shard. Single-world
    /// servers ignore `shard`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn start_room(&self, shard: Option<&str>) -> Result<Option<StartRoom>, Error> {
        let api = self.overseer.user_api()?;
        let shard = shard.and_then(|shard| self.overseer.resolve_shard(Some(shard)));
        Ok(api.start_room(shard.as_deref()).await?)
    }

    /// Abandons every room and object of the user (`POST /api/user/respawn`); afterwards
    /// the world status is `empty` and a spawn can be placed (rooms of
    /// [`Self::respawn_prohibited_rooms`] excepted).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails or the server refuses (for example right after a previous respawn).
    pub async fn respawn(&self) -> Result<(), Error> {
        self.overseer.user_api()?.respawn().await?;
        forget_me(&self.overseer).await;
        Ok(())
    }

    /// Rooms where the user may not place a spawn after respawning
    /// (`GET /api/user/respawn-prohibited-rooms`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn respawn_prohibited_rooms(&self) -> Result<Vec<String>, Error> {
        Ok(self.overseer.user_api()?.respawn_prohibited_rooms().await?)
    }

    /// CPU and Memory usage of each run of the user's code (`user:<user>/cpu`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] and [`Error::Closed`].
    pub fn cpu(&self) -> Result<Watch<UserCpu>, Error> {
        let user = self.user()?;
        Ok(self.overseer.watch(Channel::Cpu { user }, |payload| {
            if let Payload::Cpu(cpu) = payload {
                Some(cpu)
            } else {
                None
            }
        }))
    }

    /// Credits and account resources (pixels, CPU unlocks, access keys, …) whenever they
    /// change (`user:<user>/resources`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`] and [`Error::Closed`].
    pub fn resources(&self) -> Result<Watch<UserResources>, Error> {
        let user = self.user()?;
        Ok(self.overseer.watch(Channel::Resources { user }, |payload| {
            if let Payload::Resources(resources) = payload {
                Some(resources)
            } else {
                None
            }
        }))
    }

    /// The user's badge as an SVG document, optionally with a border
    /// (`GET /api/user/badge-svg` for the [username](Self::me)).
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the account has no username yet,
    /// [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when a request fails.
    pub async fn badge_svg(&self, border: bool) -> Result<String, Error> {
        let me = self.me().await?;
        let username = me
            .username
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("the account has no username yet".to_owned()))?;
        self.overseer.users().badge_svg(username, border).await
    }

    /// Replaces the user's badge (`POST /api/user/badge`): colors `#rrggbb`, a shape 1–24
    /// (or the account's custom paths) and a shape parameter -100..100.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails or the server rejects the badge (`invalid params`).
    pub async fn set_badge(&self, badge: UserBadgeInput) -> Result<(), Error> {
        self.overseer.user_api()?.set_badge(badge).await?;
        forget_me(&self.overseer).await;
        Ok(())
    }

    /// Replaces the CPU limit of every shard with `cpu`, by shard name
    /// (`POST /api/user/cpu-shards`), on servers with the CPU allocation
    /// ([`Subsystem::CpuShards`]). Omitted shards get 0. The limits must total the account's
    /// CPU ([`Profile::cpu`]), and the server refuses a change within 12 hours after the
    /// previous one ([`Profile::cpu_shards_updated`]). After success the cached profile is
    /// cleared, so the next [`Self::me`] reads the new allocation ([`Profile::cpu_shards`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`], [`Error::Net`] with
    /// [`Unsupported`](overseer_net::Error::Unsupported) on a server without the CPU
    /// allocation (nothing is sent), and [`Error::Net`] when the request fails or the server
    /// refuses the limits.
    pub async fn set_cpu_shards(&self, cpu: BTreeMap<String, u64>) -> Result<(), Error> {
        self.overseer.user_api()?.set_cpu_shards(cpu).await?;
        forget_me(&self.overseer).await;
        Ok(())
    }

    /// Changes the account's e-mail address (`POST /api/user/email`; screeps.com confirms it
    /// by e-mail first, so the [profile](Self::me) shows it as `email_dirty` until then).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails or the server refuses the address (`invalid email`, `email already exists`).
    pub async fn set_email(&self, email: &str) -> Result<(), Error> {
        self.overseer.user_api()?.set_email(email).await?;
        forget_me(&self.overseer).await;
        Ok(())
    }

    /// Chooses the username of an account that has none yet (Steam or GitHub sign-ups),
    /// optionally storing an e-mail address (`POST /api/register/set-username`). Check the
    /// name first with [`Api::username_available`](overseer_net::api::AccountApi::username_available)
    /// ([`Overseer::api`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails or the server refuses (`invalid username`, `username already set`, …).
    pub async fn set_username(&self, username: &str, email: Option<&str>) -> Result<(), Error> {
        self.overseer
            .user_api()?
            .set_username(username, email)
            .await?;
        forget_me(&self.overseer).await;
        Ok(())
    }

    /// Changes the account's password (`POST /api/user/password`), on servers with password
    /// accounts ([`Subsystem::PasswordAccounts`]; servers with an
    /// [account website](Subsystem::AccountWebsite) change passwords there).
    /// `old_password` is the current one, `None` for an account without a password yet.
    /// `OpenScreeps` ends every session of the account afterwards: connect again with the new
    /// password.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`], [`Error::Net`] with
    /// [`Unsupported`](overseer_net::Error::Unsupported) on a server without password
    /// accounts (nothing is sent), [`Error::Net`] with
    /// [`WrongPassword`](overseer_net::Error::WrongPassword) for a wrong current password
    /// (on every server alike), and [`Error::Net`] when a request fails.
    pub async fn set_password(
        &self,
        old_password: Option<&str>,
        password: &str,
    ) -> Result<(), Error> {
        self.overseer
            .user_api()?
            .password_account()
            .ok_or(overseer_net::Error::Unsupported(
                Subsystem::PasswordAccounts,
            ))?
            .set_password(old_password, password)
            .await?;
        forget_me(&self.overseer).await;
        Ok(())
    }

    /// The user's e-mail notification preferences, from the [profile](Self::me) (defaults
    /// when none were saved).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::me`].
    pub async fn notify_prefs(&self) -> Result<UserNotifyPrefs, Error> {
        Ok(self.me().await?.notify_prefs.clone())
    }

    /// Updates the user's e-mail notification preferences (`POST /api/user/notify-prefs`);
    /// fields left `None` keep their saved value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn set_notify_prefs(&self, prefs: UserNotifyPrefs) -> Result<(), Error> {
        self.overseer.user_api()?.set_notify_prefs(prefs).await?;
        forget_me(&self.overseer).await;
        Ok(())
    }

    /// The user's power creeps.
    #[must_use]
    pub fn power_creeps(&self) -> PowerCreeps {
        PowerCreeps::new(self.overseer.clone())
    }

    /// The account's persistent API tokens (`GET /api/user/auth-token`), on servers with API
    /// tokens ([`Subsystem::ApiTokens`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`], [`Error::Net`] with
    /// [`Unsupported`](overseer_net::Error::Unsupported) on a server without API tokens
    /// (nothing is sent), and [`Error::Net`] when the request fails.
    pub async fn auth_tokens(&self) -> Result<Vec<AuthToken>, Error> {
        Ok(self.tokens()?.auth_tokens().await?)
    }

    /// Until when the session's API token is exempt from rate limits, in milliseconds since
    /// the Unix epoch, on servers that lift them ([`Subsystem::RateLimitLift`]): the
    /// `noRatelimitUntil` of its entry in [`Self::auth_tokens`] (one
    /// `GET /api/user/auth-token`, which reports no rate limit of its own), while it lies in
    /// the future. [`Self::rate_limit_lift`] says how the user lifts them, for about two
    /// hours.
    ///
    /// The list does not hold whole tokens: each entry's `token` is its first 8 characters,
    /// matched against the session's token. `None` when the
    /// exemption has passed or was never granted, and for sessions not signed in with an API
    /// token (a password sign-in's session token matches no entry).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] with [`Unsupported`](overseer_net::Error::Unsupported) on a
    /// server that does not lift rate limits (nothing is sent), and the errors of
    /// [`Self::auth_tokens`].
    pub async fn rate_limit_exemption(&self) -> Result<Option<u64>, Error> {
        self.require(Subsystem::RateLimitLift)?;
        let tokens = self.auth_tokens().await?;
        let session = self.overseer.tokens().borrow().clone();
        Ok(session.and_then(|session| exemption(&tokens, &session, epoch_millis())))
    }

    /// How the session's API token gets its rate limits lifted ([`Subsystem::RateLimitLift`]):
    /// a web page for the user to open, or a request to send
    /// ([`Api::rate_limit_lift`](overseer_net::Api::rate_limit_lift)). Only the token's first
    /// 8 characters are used. Check [`Self::rate_limit_exemption`] afterwards.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`], and [`Error::Net`] with
    /// [`Unsupported`](overseer_net::Error::Unsupported) on a server that does not lift rate
    /// limits, or for a session token no longer than 8 characters.
    pub fn rate_limit_lift(&self) -> Result<RateLimitLift, Error> {
        let api = self.overseer.user_api()?;
        let token = self.overseer.tokens().borrow().clone();
        token
            .and_then(|token| api.rate_limit_lift(&token))
            .ok_or_else(|| overseer_net::Error::Unsupported(Subsystem::RateLimitLift).into())
    }

    /// Generates a persistent API token with the permissions of `request` (`type: full`, or
    /// selected endpoints, websockets and Memory segments) via `POST /api/user/auth-token`.
    /// The token is returned only this once.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::auth_tokens`].
    pub async fn create_auth_token(&self, request: AuthTokenRequest) -> Result<Token, Error> {
        Ok(self.tokens()?.create_auth_token(request).await?)
    }

    /// Revokes the API token with entry id `id` (see [`Self::auth_tokens`]) via
    /// `POST /api/user/delete-auth-token`.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::auth_tokens`].
    pub async fn delete_auth_token(&self, id: &str) -> Result<(), Error> {
        Ok(self.tokens()?.delete_auth_token(id).await?)
    }

    /// The server's API tokens, or [`overseer_net::Error::Unsupported`].
    fn tokens(&self) -> Result<ApiTokensApi<'_>, Error> {
        self.overseer
            .user_api()?
            .api_tokens()
            .ok_or_else(|| overseer_net::Error::Unsupported(Subsystem::ApiTokens).into())
    }

    /// Fails with [`overseer_net::Error::Unsupported`] unless the server offers `subsystem`
    /// (and the session is open and signed in).
    fn require(&self, subsystem: Subsystem) -> Result<(), Error> {
        if self.overseer.user_api()?.supports(subsystem) {
            Ok(())
        } else {
            Err(overseer_net::Error::Unsupported(subsystem).into())
        }
    }

    /// The signed-in user's id for a channel.
    fn user(&self) -> Result<String, Error> {
        self.overseer.ensure_open()?;
        Ok(self.overseer.require_user()?.to_owned())
    }
}

/// Clears the cached profile after a change of the account.
async fn forget_me(overseer: &Overseer) {
    *overseer.me_cache().lock().await = None;
}

/// How many leading characters of a token the API token list ([`Account::auth_tokens`])
/// reports.
const TOKEN_PREFIX_CHARS: usize = 8;

/// The first [`TOKEN_PREFIX_CHARS`] characters of `token`; `None` when it is shorter.
fn token_prefix(token: &str) -> Option<&str> {
    let end = token
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(token.len()))
        .nth(TOKEN_PREFIX_CHARS)?;
    token.get(..end)
}

/// The `noRatelimitUntil` of the entry of `tokens` whose prefix is `session`'s, when after
/// `now` (milliseconds since the Unix epoch).
fn exemption(tokens: &[AuthToken], session: &Token, now: u64) -> Option<u64> {
    let prefix = token_prefix(session.as_str())?;
    tokens
        .iter()
        .find(|entry| token_prefix(&entry.token) == Some(prefix))?
        .no_ratelimit_until
        .filter(|until| *until > now)
}

/// Now, in milliseconds since the Unix epoch.
fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use overseer_net::core::Token;
    use overseer_net::model::account::AuthToken;

    use super::{exemption, token_prefix};

    fn entry(token: &str, until: Option<u64>) -> AuthToken {
        AuthToken {
            id: format!("id-{token}"),
            description: None,
            full: true,
            no_ratelimit_until: until,
            token: token.to_owned(),
        }
    }

    #[test]
    fn the_session_token_matches_its_entry_by_the_first_8_characters() {
        let now = 1_700_000_000_000;
        let session = Token::new("0123abcd-4567-89ef-0123-456789abcdef");
        let tokens = [
            entry("ffffeeee", Some(now + 9_000_000)),
            entry("0123abcd", Some(now + 7_200_000)),
        ];
        assert_eq!(exemption(&tokens, &session, now), Some(now + 7_200_000));
        // A longer entry still matches by its prefix.
        let longer = [entry("0123abcd-4567", Some(now + 60_000))];
        assert_eq!(exemption(&longer, &session, now), Some(now + 60_000));
        // A past or missing exemption, or no entry of the session (a password sign-in's
        // session token), is none.
        let past = [
            entry("0123abcd", Some(now - 1)),
            entry("ffffeeee", Some(now + 1)),
        ];
        assert_eq!(exemption(&past, &session, now), None);
        assert_eq!(exemption(&past, &session, now - 1), None);
        assert_eq!(exemption(&[entry("0123abcd", None)], &session, now), None);
        assert_eq!(
            exemption(&[entry("0123abce", Some(now + 1))], &session, now),
            None
        );
        assert_eq!(exemption(&[], &session, now), None);
        // Fewer than 8 characters match nothing.
        assert_eq!(
            exemption(&[entry("0123", Some(now + 1))], &Token::new("0123"), now),
            None
        );
        assert_eq!(
            exemption(&[entry("", Some(now + 1))], &Token::new(""), now),
            None
        );
    }

    #[test]
    fn prefixes_count_characters() {
        assert_eq!(token_prefix("0123abcd"), Some("0123abcd"));
        assert_eq!(token_prefix("0123abcdef"), Some("0123abcd"));
        assert_eq!(token_prefix("0123abc"), None);
        assert_eq!(token_prefix("éééééééééé"), Some("éééééééé"));
    }
}
