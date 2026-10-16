//! The signed-in user's account: profile, overview statistics, world status, start room, respawn, CPU and resources, badge, notification preferences, API tokens and power creeps.

mod power_creeps;

use std::sync::Arc;

use overseer_net::core::Token;
use overseer_net::core::api::{endpoints, types};
use overseer_net::core::socket::{Channel, Payload, UserCpu, UserResources};
use overseer_net::rt::{SystemTime, UNIX_EPOCH};

use crate::rooms::StatInterval;
use crate::{Error, Overseer, Watch};

pub use power_creeps::PowerCreeps;

/// The room to center the world map on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartRoom {
    /// Shard of the room; `None` on single-world servers.
    pub shard: Option<String>,
    /// Room name, for example `W8N3`.
    pub room: String,
}

/// The signed-in user's account: profile, overview statistics, world status, start room, respawn, CPU and resources, badge, notification preferences, API tokens and power creeps.
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
    /// Fetched once per session and then served from a cache shared by every handle;
    /// [`Self::refresh`] fetches it again. The account's own changes made through this crate
    /// ([`Self::set_badge`], [`Self::set_notify_prefs`], [`Self::respawn`],
    /// [`PowerCreeps::start_experimentation`]) clear the cache, so the next call fetches it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn me(&self) -> Result<Arc<types::AuthMeResponse>, Error> {
        self.overseer.ensure_open()?;
        let mut cached = self.overseer.me_cache().lock().await;
        if let Some(me) = &*cached {
            return Ok(Arc::clone(me));
        }
        let me = Arc::new(self.overseer.send(&endpoints::AuthMe).await?);
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
    pub async fn refresh(&self) -> Result<Arc<types::AuthMeResponse>, Error> {
        let mut cached = self.overseer.me_cache().lock().await;
        let me = Arc::new(self.overseer.send(&endpoints::AuthMe).await?);
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
        stat: types::UserRoomStatName,
    ) -> Result<types::UserOverviewResponse, Error> {
        self.user()?;
        Ok(self
            .overseer
            .client()
            .overview(Some(interval.minutes()), Some(stat))
            .await?)
    }

    /// Whether the user plays (`normal`), has lost every spawn (`lost`) or has nothing in the
    /// world and must place a spawn (`empty`), via `GET /api/user/world-status`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn world_status(&self) -> Result<types::UserWorldStatus, Error> {
        Ok(self
            .overseer
            .send(&endpoints::UserWorldStatus)
            .await?
            .status)
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
        let shard = shard.and_then(|shard| self.overseer.resolve_shard(Some(shard)));
        let reply = self
            .overseer
            .send(&endpoints::UserWorldStartRoom {
                shard: shard.clone(),
            })
            .await?;
        Ok(reply.room.into_iter().next().map(|room| {
            // Without `shard` a sharded server names it: `shard/room`.
            match room.split_once('/') {
                Some((named, room)) => StartRoom {
                    shard: Some(named.to_owned()),
                    room: room.to_owned(),
                },
                None => StartRoom { shard, room },
            }
        }))
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
        self.overseer.send(&endpoints::UserRespawn).await?;
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
        Ok(self
            .overseer
            .send(&endpoints::UserRespawnProhibitedRooms)
            .await?
            .rooms)
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
    pub async fn set_badge(&self, badge: types::UserBadgeInput) -> Result<(), Error> {
        let body = types::UserBadgeSetRequest { badge };
        self.overseer
            .send(&endpoints::UserBadgeSet { body })
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
    pub async fn notify_prefs(&self) -> Result<types::UserNotifyPrefs, Error> {
        Ok(self.me().await?.notify_prefs.clone().unwrap_or_default())
    }

    /// Updates the user's e-mail notification preferences (`POST /api/user/notify-prefs`);
    /// fields left `None` keep their saved value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn set_notify_prefs(&self, prefs: types::UserNotifyPrefs) -> Result<(), Error> {
        self.overseer
            .send(&endpoints::UserNotifyPrefsSet { body: prefs })
            .await?;
        forget_me(&self.overseer).await;
        Ok(())
    }

    /// The user's power creeps.
    #[must_use]
    pub fn power_creeps(&self) -> PowerCreeps {
        PowerCreeps::new(self.overseer.clone())
    }

    /// The account's persistent API tokens (`GET /api/user/auth-token`). Official server
    /// only: other servers answer with an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn auth_tokens(&self) -> Result<Vec<types::UserAuthTokenListEntry>, Error> {
        Ok(self
            .overseer
            .send(&endpoints::UserAuthTokenList)
            .await?
            .tokens
            .unwrap_or_default())
    }

    /// Until when the session's API token is exempt from screeps.com's rate limits, in
    /// milliseconds since the Unix epoch: the `noRatelimitUntil` of its entry in
    /// [`Self::auth_tokens`] (one `GET /api/user/auth-token`, which reports no rate limit of
    /// its own), while it lies in the future. The user lifts the limits of a token on
    /// screeps.com's reset page (`#!/account/auth-tokens/noratelimit`, a captcha), for about
    /// two hours.
    ///
    /// The list does not hold whole tokens: each entry's `token` is its first 8 characters,
    /// matched against the session's token. `None` when the
    /// exemption has passed or was never granted, and for sessions not signed in with an API
    /// token (a password sign-in's session token matches no entry). Official server only:
    /// other servers answer with an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn rate_limit_exemption(&self) -> Result<Option<u64>, Error> {
        let tokens = self.auth_tokens().await?;
        let session = self.overseer.tokens().borrow().clone();
        Ok(session.and_then(|session| exemption(&tokens, &session, epoch_millis())))
    }

    /// Generates a persistent API token with the permissions of `request` (`type: full`, or
    /// selected endpoints, websockets and Memory segments) via `POST /api/user/auth-token`.
    /// The token is returned only this once. Official server only.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn create_auth_token(
        &self,
        request: types::UserAuthTokenCreateRequest,
    ) -> Result<Token, Error> {
        let reply = self
            .overseer
            .send(&endpoints::UserAuthTokenCreate { body: request })
            .await?;
        Ok(Token::new(reply.token))
    }

    /// Revokes the API token with entry id `id` (see [`Self::auth_tokens`]) via
    /// `POST /api/user/delete-auth-token`. Official server only.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Anonymous`], [`Error::Closed`] and [`Error::Net`] when the request
    /// fails.
    pub async fn delete_auth_token(&self, id: &str) -> Result<(), Error> {
        let body = types::UserAuthTokenDeleteRequest {
            token_id: id.to_owned(),
        };
        self.overseer
            .send(&endpoints::UserAuthTokenDelete { body })
            .await?;
        Ok(())
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
fn exemption(tokens: &[types::UserAuthTokenListEntry], session: &Token, now: u64) -> Option<u64> {
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
    use overseer_net::core::api::types::UserAuthTokenListEntry;

    use super::{exemption, token_prefix};

    fn entry(token: &str, until: Option<u64>) -> UserAuthTokenListEntry {
        UserAuthTokenListEntry {
            id: format!("id-{token}"),
            token: token.to_owned(),
            full: true,
            no_ratelimit_until: until,
            ..UserAuthTokenListEntry::default()
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
