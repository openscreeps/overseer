//! The signed-in user's account: profile, overview statistics, world status, start room, respawn, CPU and resources, badge, notification preferences, API tokens and power creeps.

mod power_creeps;

use std::sync::Arc;

use overseer_net::core::Token;
use overseer_net::core::api::{endpoints, types};
use overseer_net::core::socket::{Channel, Payload, UserCpu, UserResources};

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
