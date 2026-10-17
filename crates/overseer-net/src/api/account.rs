//! Account area: the signed-in user's profile, preferences, badge, e-mail and username, world
//! status, start room, respawn and statistics overview, and the subsystems password sign-in
//! ([`PasswordSignIn`]), password accounts ([`PasswordAccount`]: registration and password
//! changes), API tokens ([`ApiTokens`]), lifting their rate limits ([`RateLimitLift`]), the
//! account website ([`AccountWebsite`]) and the CPU allocation between shards
//! ([`AccountApi::set_cpu_shards`]).

mod api_tokens;
mod password;
mod rate_limit_lift;

use std::collections::BTreeMap;
use std::future::ready;

use overseer_core::api::plugins::{screepsmod_admin_utils, screepsmod_auth};
use overseer_core::api::{ApiError, Reply, community, official, openscreeps};
use overseer_core::model::account::{
    Availability, Overview, Profile, StartRoom, UserBadgeInput, UserNotifyPrefs, UserRoomStatName,
    UserWorldStatus,
};
use overseer_core::model::platform::Subsystem;

use super::{Api, Community, Official, OpenScreeps, delegate};
use crate::Error;

pub use api_tokens::{ApiTokens, ApiTokensApi};
pub use password::{PasswordAccount, PasswordAccountApi, PasswordSignIn, PasswordSignInApi};
pub use rate_limit_lift::{LIFT_TOKEN_PREFIX, LiftPage, LiftRequest, RateLimitLift};

/// The signed-in user's account. The availability checks need no sign-in.
pub trait AccountApi {
    /// The signed-in user's private profile (`GET /api/auth/me`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send), for example
    /// [`ApiError::Unauthorized`] without a valid token.
    fn me(&self) -> impl Future<Output = Result<Profile, Error>>;

    /// Updates the user's e-mail notification preferences (`POST /api/user/notify-prefs`);
    /// fields left `None` keep their saved value.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn set_notify_prefs(&self, prefs: UserNotifyPrefs) -> impl Future<Output = Result<(), Error>>;

    /// The user's username alone (`GET /api/user/name`), which restricted API tokens may read
    /// too ([`Subsystem::UsernameLookup`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unsupported`] on a server without the username lookup and the errors
    /// of [`Client::send`](crate::Client::send).
    fn name(&self) -> impl Future<Output = Result<String, Error>>;

    /// Per-room series of `stat` over the last `interval` minutes (8, 180 or 1440) and the
    /// totals of every statistic (`GET /api/user/overview`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn overview(
        &self,
        interval: u64,
        stat: UserRoomStatName,
    ) -> impl Future<Output = Result<Overview, Error>>;

    /// Whether the user plays, has lost every spawn or has nothing in the world
    /// (`GET /api/user/world-status`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn world_status(&self) -> impl Future<Output = Result<UserWorldStatus, Error>>;

    /// The room to center the world map on (`GET /api/user/world-start-room`): one of the
    /// user's rooms, else the server's default; `None` when the server names none. On
    /// screeps.com `shard: None` lets the server choose the shard (the user's); single-world
    /// servers ignore `shard` and answer rooms without one.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn start_room(
        &self,
        shard: Option<&str>,
    ) -> impl Future<Output = Result<Option<StartRoom>, Error>>;

    /// Abandons every room and object of the user (`POST /api/user/respawn`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send), for example when the
    /// server refuses right after a previous respawn.
    fn respawn(&self) -> impl Future<Output = Result<(), Error>>;

    /// Rooms where the user may not place a spawn after respawning
    /// (`GET /api/user/respawn-prohibited-rooms`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn respawn_prohibited_rooms(&self) -> impl Future<Output = Result<Vec<String>, Error>>;

    /// Replaces the user's badge (`POST /api/user/badge`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send), `invalid params` for a
    /// rejected badge.
    fn set_badge(&self, badge: UserBadgeInput) -> impl Future<Output = Result<(), Error>>;

    /// Replaces the CPU limit of every shard with `cpu`, by shard name
    /// (`POST /api/user/cpu-shards`; [`Subsystem::CpuShards`]). Omitted shards get 0. The
    /// limits must total the account's CPU, and the server refuses a change within 12 hours
    /// after the previous one ([`Profile::cpu_shards_updated`]).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Unsupported`] on a server without the CPU allocation (nothing is
    /// sent) and the errors of [`Client::send`](crate::Client::send), for example when the
    /// server refuses the limits.
    fn set_cpu_shards(&self, cpu: BTreeMap<String, u64>)
    -> impl Future<Output = Result<(), Error>>;

    /// Changes the account's e-mail address (`POST /api/user/email`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send): `invalid email`,
    /// `email already exists`.
    fn set_email(&self, email: &str) -> impl Future<Output = Result<(), Error>>;

    /// Chooses the username of an account that has none yet (Steam or GitHub sign-ups),
    /// optionally with an e-mail address (`POST /api/register/set-username`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send): `invalid username`,
    /// `username already set`, ….
    fn set_username(
        &self,
        username: &str,
        email: Option<&str>,
    ) -> impl Future<Output = Result<(), Error>>;

    /// Whether `username` is free for an account (`GET /api/register/check-username`); needs
    /// no sign-in. screepsmod-auth, which answers the check on community servers, compares it
    /// exactly, the others case-insensitively.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send) but the one meaning taken.
    fn username_available(
        &self,
        username: &str,
    ) -> impl Future<Output = Result<Availability, Error>>;

    /// Whether `email` is free for an account (`GET /api/register/check-email`); needs no
    /// sign-in.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send) but the one meaning taken.
    fn email_available(&self, email: &str) -> impl Future<Output = Result<Availability, Error>>;
}

/// The error a registration check answers for a value an account uses: `exists` natively,
/// `User Exists` from screepsmod-auth.
const EXISTS: &str = "exists";
/// screepsmod-auth's [`EXISTS`].
const USER_EXISTS: &str = "User Exists";

/// The [`Availability`] the reply of a registration check means: success is available, the
/// error `taken` taken, any other error stays one.
fn availability<T>(reply: Result<Reply<T>, Error>, taken: &str) -> Result<Availability, Error> {
    match reply {
        Ok(_) => Ok(Availability::Available),
        Err(Error::Api(ApiError::Server(message))) if message == taken => Ok(Availability::Taken),
        Err(error) => Err(error),
    }
}

impl AccountApi for Api {
    async fn me(&self) -> Result<Profile, Error> {
        delegate!(self, api => api.me().await)
    }

    async fn set_notify_prefs(&self, prefs: UserNotifyPrefs) -> Result<(), Error> {
        delegate!(self, api => api.set_notify_prefs(prefs).await)
    }

    async fn name(&self) -> Result<String, Error> {
        delegate!(self, api => api.name().await)
    }

    async fn overview(&self, interval: u64, stat: UserRoomStatName) -> Result<Overview, Error> {
        delegate!(self, api => api.overview(interval, stat).await)
    }

    async fn world_status(&self) -> Result<UserWorldStatus, Error> {
        delegate!(self, api => api.world_status().await)
    }

    async fn start_room(&self, shard: Option<&str>) -> Result<Option<StartRoom>, Error> {
        delegate!(self, api => api.start_room(shard).await)
    }

    async fn respawn(&self) -> Result<(), Error> {
        delegate!(self, api => api.respawn().await)
    }

    async fn respawn_prohibited_rooms(&self) -> Result<Vec<String>, Error> {
        delegate!(self, api => api.respawn_prohibited_rooms().await)
    }

    async fn set_badge(&self, badge: UserBadgeInput) -> Result<(), Error> {
        delegate!(self, api => api.set_badge(badge).await)
    }

    async fn set_cpu_shards(&self, cpu: BTreeMap<String, u64>) -> Result<(), Error> {
        delegate!(self, api => api.set_cpu_shards(cpu).await)
    }

    async fn set_email(&self, email: &str) -> Result<(), Error> {
        delegate!(self, api => api.set_email(email).await)
    }

    async fn set_username(&self, username: &str, email: Option<&str>) -> Result<(), Error> {
        delegate!(self, api => api.set_username(username, email).await)
    }

    async fn username_available(&self, username: &str) -> Result<Availability, Error> {
        delegate!(self, api => api.username_available(username).await)
    }

    async fn email_available(&self, email: &str) -> Result<Availability, Error> {
        delegate!(self, api => api.email_available(email).await)
    }
}

/// The [`AccountApi`] operations the official and `OpenScreeps` contracts send alike (their
/// requests and the replies read are the same types), from `$contract`'s endpoints.
macro_rules! shared_account_ops {
    ($contract:ident) => {
        async fn me(&self) -> Result<Profile, Error> {
            let reply = self.client.send(&$contract::endpoints::AuthMe).await?;
            Ok(reply.body.into())
        }

        async fn set_notify_prefs(&self, prefs: UserNotifyPrefs) -> Result<(), Error> {
            let set = $contract::endpoints::UserNotifyPrefsSet { body: prefs };
            self.client.send(&set).await?;
            Ok(())
        }

        async fn name(&self) -> Result<String, Error> {
            let reply = self.client.send(&$contract::endpoints::UserName).await?;
            Ok(reply.body.username)
        }

        async fn overview(&self, interval: u64, stat: UserRoomStatName) -> Result<Overview, Error> {
            let overview = $contract::endpoints::UserOverview {
                interval: Some(interval),
                stat_name: Some(stat),
            };
            Ok(self.client.send(&overview).await?.body.into())
        }

        async fn world_status(&self) -> Result<UserWorldStatus, Error> {
            let reply = self
                .client
                .send(&$contract::endpoints::UserWorldStatus)
                .await?;
            Ok(reply.body.status)
        }

        async fn respawn(&self) -> Result<(), Error> {
            self.client.send(&$contract::endpoints::UserRespawn).await?;
            Ok(())
        }

        async fn respawn_prohibited_rooms(&self) -> Result<Vec<String>, Error> {
            let reply = self
                .client
                .send(&$contract::endpoints::UserRespawnProhibitedRooms)
                .await?;
            Ok(reply.body.rooms)
        }

        async fn set_badge(&self, badge: UserBadgeInput) -> Result<(), Error> {
            let set = $contract::endpoints::UserBadgeSet {
                body: $contract::types::UserBadgeSetRequest { badge },
            };
            self.client.send(&set).await?;
            Ok(())
        }

        async fn set_email(&self, email: &str) -> Result<(), Error> {
            let set = $contract::endpoints::UserEmailSet {
                body: $contract::types::UserEmailRequest {
                    email: email.to_owned(),
                },
            };
            self.client.send(&set).await?;
            Ok(())
        }

        async fn set_username(&self, username: &str, email: Option<&str>) -> Result<(), Error> {
            let set = $contract::endpoints::RegisterSetUsername {
                body: $contract::types::RegisterSetUsernameRequest {
                    username: username.to_owned(),
                    email: email.map(str::to_owned),
                    ..Default::default()
                },
            };
            self.client.send(&set).await?;
            Ok(())
        }

        async fn username_available(&self, username: &str) -> Result<Availability, Error> {
            let check = $contract::endpoints::RegisterCheckUsername {
                username: username.to_owned(),
            };
            availability(self.client.send(&check).await, EXISTS)
        }

        async fn email_available(&self, email: &str) -> Result<Availability, Error> {
            let check = $contract::endpoints::RegisterCheckEmail {
                email: email.to_owned(),
            };
            availability(self.client.send(&check).await, EXISTS)
        }
    };
}

impl AccountApi for Official {
    shared_account_ops!(official);

    async fn start_room(&self, shard: Option<&str>) -> Result<Option<StartRoom>, Error> {
        let start = official::endpoints::UserWorldStartRoom {
            shard: shard.map(str::to_owned),
        };
        let room: Option<StartRoom> = self.client.send(&start).await?.body.into();
        // Asked for a shard, screeps.com answers a bare room on it.
        Ok(room.map(|room| StartRoom {
            shard: room.shard.or_else(|| shard.map(str::to_owned)),
            room: room.room,
        }))
    }

    async fn set_cpu_shards(&self, cpu: BTreeMap<String, u64>) -> Result<(), Error> {
        let set = official::endpoints::UserCpuShardsSet {
            body: official::types::UserCpuShardsRequest { cpu },
        };
        self.client.send(&set).await?;
        Ok(())
    }
}

impl AccountApi for OpenScreeps {
    shared_account_ops!(openscreeps);

    async fn start_room(&self, _shard: Option<&str>) -> Result<Option<StartRoom>, Error> {
        let start = openscreeps::endpoints::UserWorldStartRoom { shard: None };
        Ok(self.client.send(&start).await?.body.into())
    }

    fn set_cpu_shards(
        &self,
        _cpu: BTreeMap<String, u64>,
    ) -> impl Future<Output = Result<(), Error>> {
        ready(Err(Error::Unsupported(Subsystem::CpuShards)))
    }
}

impl AccountApi for Community {
    async fn me(&self) -> Result<Profile, Error> {
        let reply = self.client.send(&community::endpoints::AuthMe).await?;
        Ok(reply.body.into())
    }

    async fn set_notify_prefs(&self, prefs: UserNotifyPrefs) -> Result<(), Error> {
        let set = community::endpoints::UserNotifyPrefsSet { body: prefs };
        self.client.send(&set).await?;
        Ok(())
    }

    async fn name(&self) -> Result<String, Error> {
        let auth = self
            .screepsmod_auth()
            .ok_or(Error::Unsupported(Subsystem::UsernameLookup))?;
        let reply = auth
            .client
            .send(&screepsmod_auth::endpoints::UserName)
            .await?;
        Ok(reply.body.username)
    }

    async fn overview(&self, interval: u64, stat: UserRoomStatName) -> Result<Overview, Error> {
        let overview = community::endpoints::UserOverview {
            interval: Some(interval),
            stat_name: Some(stat),
        };
        Ok(self.client.send(&overview).await?.body.into())
    }

    async fn world_status(&self) -> Result<UserWorldStatus, Error> {
        let reply = self
            .client
            .send(&community::endpoints::UserWorldStatus)
            .await?;
        Ok(reply.body.status)
    }

    async fn start_room(&self, _shard: Option<&str>) -> Result<Option<StartRoom>, Error> {
        if let Some(admin_utils) = self.screepsmod_admin_utils() {
            let start = screepsmod_admin_utils::endpoints::UserWorldStartRoom { shard: None };
            Ok(admin_utils.client.send(&start).await?.body.into())
        } else {
            let start = community::endpoints::UserWorldStartRoom { shard: None };
            Ok(self.client.send(&start).await?.body.into())
        }
    }

    async fn respawn(&self) -> Result<(), Error> {
        self.client.send(&community::endpoints::UserRespawn).await?;
        Ok(())
    }

    async fn respawn_prohibited_rooms(&self) -> Result<Vec<String>, Error> {
        let reply = self
            .client
            .send(&community::endpoints::UserRespawnProhibitedRooms)
            .await?;
        Ok(reply.body.rooms)
    }

    async fn set_badge(&self, badge: UserBadgeInput) -> Result<(), Error> {
        let set = community::endpoints::UserBadgeSet {
            body: community::types::UserBadgeSetRequest { badge },
        };
        self.client.send(&set).await?;
        Ok(())
    }

    fn set_cpu_shards(
        &self,
        _cpu: BTreeMap<String, u64>,
    ) -> impl Future<Output = Result<(), Error>> {
        ready(Err(Error::Unsupported(Subsystem::CpuShards)))
    }

    async fn set_email(&self, email: &str) -> Result<(), Error> {
        let set = community::endpoints::UserEmailSet {
            body: community::types::UserEmailRequest {
                email: email.to_owned(),
            },
        };
        self.client.send(&set).await?;
        Ok(())
    }

    async fn set_username(&self, username: &str, email: Option<&str>) -> Result<(), Error> {
        let set = community::endpoints::RegisterSetUsername {
            body: community::types::RegisterSetUsernameRequest {
                username: username.to_owned(),
                email: email.map(str::to_owned),
                ..Default::default()
            },
        };
        self.client.send(&set).await?;
        Ok(())
    }

    // screepsmod-auth answers the checks in place of the server.

    async fn username_available(&self, username: &str) -> Result<Availability, Error> {
        let username = username.to_owned();
        if let Some(auth) = self.screepsmod_auth() {
            let check = screepsmod_auth::endpoints::RegisterCheckUsername { username };
            availability(auth.client.send(&check).await, USER_EXISTS)
        } else {
            let check = community::endpoints::RegisterCheckUsername { username };
            availability(self.client.send(&check).await, EXISTS)
        }
    }

    async fn email_available(&self, email: &str) -> Result<Availability, Error> {
        let email = email.to_owned();
        if let Some(auth) = self.screepsmod_auth() {
            let check = screepsmod_auth::endpoints::RegisterCheckEmail { email };
            availability(auth.client.send(&check).await, USER_EXISTS)
        } else {
            let check = community::endpoints::RegisterCheckEmail { email };
            availability(self.client.send(&check).await, EXISTS)
        }
    }
}

impl Api {
    /// The pages of the server's account website, when it has one
    /// ([`Subsystem::AccountWebsite`]): screeps.com, where accounts register, recover their
    /// password and manage their API tokens.
    #[must_use]
    pub const fn account_website(&self) -> Option<AccountWebsite> {
        match self {
            Self::Official(_) => Some(AccountWebsite {
                register: "https://screeps.com/a/#!/register",
                recover_password: "https://screeps.com/a/#!/register/ask-recover",
                api_tokens: "https://screeps.com/a/#!/account/auth-tokens",
            }),
            Self::OpenScreeps(_) | Self::Community(_) => None,
        }
    }
}

/// The pages of a server's account website ([`Subsystem::AccountWebsite`]); from
/// [`Api::account_website`]. Open them in a browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountWebsite {
    /// Registering an account (screeps.com: `#!/register`, linked by the official client).
    pub register: &'static str,
    /// Recovering a forgotten password (screeps.com: `#!/register/ask-recover`).
    pub recover_password: &'static str,
    /// Creating and revoking API tokens (screeps.com: `#!/account/auth-tokens`).
    pub api_tokens: &'static str,
}

#[cfg(test)]
mod tests {
    use overseer_core::api::{ApiError, Reply};
    use overseer_core::model::account::Availability;

    use super::{EXISTS, USER_EXISTS, availability};
    use crate::Error;

    fn failed(message: &str) -> Result<Reply<()>, Error> {
        Err(ApiError::Server(message.to_owned()).into())
    }

    #[test]
    fn checks_read_each_contracts_word_for_taken() {
        assert_eq!(
            availability(failed("exists"), EXISTS).unwrap(),
            Availability::Taken
        );
        assert_eq!(
            availability(failed("User Exists"), USER_EXISTS).unwrap(),
            Availability::Taken
        );
        // Another error stays one, also the other contract's word.
        assert!(matches!(
            availability(failed("invalid username"), EXISTS),
            Err(Error::Api(ApiError::Server(message))) if message == "invalid username"
        ));
        assert!(availability(failed("exists"), USER_EXISTS).is_err());
    }
}
