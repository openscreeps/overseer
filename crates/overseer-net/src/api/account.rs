//! Account area: the signed-in user's profile, preferences, badge, e-mail and username, world
//! status, start room, respawn and statistics overview, and the subsystems password sign-in
//! ([`PasswordSignIn`]), password accounts ([`PasswordAccount`]: registration and password
//! changes), API tokens ([`ApiTokens`]), lifting their rate limits ([`RateLimitLift`]) and the
//! account website ([`AccountWebsite`]).

use overseer_core::Token;
use overseer_core::api::plugins::{screepsmod_admin_utils, screepsmod_auth};
use overseer_core::api::{ApiError, Reply, community, official, openscreeps};
use overseer_core::model::account::{
    AuthToken, AuthTokenRequest, Availability, Overview, Profile, Registration, StartRoom,
    UserBadgeInput, UserNotifyPrefs, UserRoomStatName, UserWorldStatus,
};
use overseer_core::model::platform::Subsystem;

use super::{Api, Community, Official, OpenScreeps, ScreepsmodAuth, delegate};
use crate::{Client, Error};

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
}

impl AccountApi for OpenScreeps {
    shared_account_ops!(openscreeps);

    async fn start_room(&self, _shard: Option<&str>) -> Result<Option<StartRoom>, Error> {
        let start = openscreeps::endpoints::UserWorldStartRoom { shard: None };
        Ok(self.client.send(&start).await?.body.into())
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

/// Signing in with an e-mail address (or a username) and a password
/// ([`Subsystem::PasswordSignIn`]): native on screeps.com and `OpenScreeps`, provided by
/// screepsmod-auth on community servers. From [`Api::password_sign_in`].
pub trait PasswordSignIn {
    /// Signs in with `email` (or, where [`Self::accepts_username`], the username) and
    /// `password` (`POST /api/auth/signin`) and returns the session token. The token is not
    /// stored: [`Client`](crate::Client) signs in itself with its credentials.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Http`] for transport failures and [`Error::Api`] for error replies,
    /// [`ApiError::Unauthorized`] for wrong credentials.
    fn sign_in(&self, email: &str, password: &str) -> impl Future<Output = Result<Token, Error>>;

    /// Whether the account's username signs in too, in place of its e-mail address: on
    /// `OpenScreeps` and with screepsmod-auth; screeps.com takes the e-mail address only.
    fn accepts_username(&self) -> bool;
}

/// The API of a server offering [`PasswordSignIn`]; from [`Api::password_sign_in`].
#[derive(Clone, Copy, Debug)]
pub enum PasswordSignInApi<'a> {
    /// Native on screeps.com.
    Official(&'a Official),
    /// Native on `OpenScreeps`.
    OpenScreeps(&'a OpenScreeps),
    /// screepsmod-auth on a community server.
    ScreepsmodAuth(ScreepsmodAuth<'a>),
}

/// Accounts of the server's own with a password ([`Subsystem::PasswordAccounts`]):
/// registration and password changes, native on `OpenScreeps` and provided by screepsmod-auth
/// on community servers. screeps.com offers both on its website only (reCAPTCHA; see
/// [`Api::account_website`]). From [`Api::password_account`].
pub trait PasswordAccount {
    /// Registers a new account (`POST /api/register/submit`); it signs in afterwards with
    /// [`PasswordSignIn`] (registering issues no token). Needs no sign-in.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send), the server's message for
    /// a rejected registration (`username already exists`, `Error: User already exists…`, …).
    fn register(&self, registration: Registration) -> impl Future<Output = Result<(), Error>>;

    /// Changes the signed-in account's password (`POST /api/user/password`); `old_password`
    /// is the current one, `None` for an account without a password (Steam or GitHub
    /// sign-ups). `OpenScreeps` revokes the account's sessions afterwards: sign in again.
    /// screepsmod-auth erases the stored password and crashes the server when given a wrong
    /// current password, so there the current password is checked first by signing in with it
    /// (`GET /api/auth/me`, then `POST /api/auth/signin`), and a wrong one sends no change.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WrongPassword`] for a wrong (or missing) current password, and the
    /// errors of [`Client::send`](crate::Client::send).
    fn set_password(
        &self,
        old_password: Option<&str>,
        password: &str,
    ) -> impl Future<Output = Result<(), Error>>;
}

/// The API of a server offering [`PasswordAccount`]; from [`Api::password_account`].
#[derive(Clone, Copy, Debug)]
pub enum PasswordAccountApi<'a> {
    /// Native on `OpenScreeps`.
    OpenScreeps(&'a OpenScreeps),
    /// screepsmod-auth on a community server.
    ScreepsmodAuth(ScreepsmodAuth<'a>),
}

impl Api {
    /// Password sign-in ([`PasswordSignIn`]), when the server offers it
    /// ([`Subsystem::PasswordSignIn`]): screeps.com, `OpenScreeps`, and community servers with
    /// screepsmod-auth.
    #[must_use]
    pub fn password_sign_in(&self) -> Option<PasswordSignInApi<'_>> {
        match self {
            Self::Official(api) => Some(PasswordSignInApi::Official(api)),
            Self::OpenScreeps(api) => Some(PasswordSignInApi::OpenScreeps(api)),
            Self::Community(api) => api.screepsmod_auth().map(PasswordSignInApi::ScreepsmodAuth),
        }
    }

    /// Registration and password changes ([`PasswordAccount`]), when the server offers them
    /// ([`Subsystem::PasswordAccounts`]): `OpenScreeps`, and community servers with
    /// screepsmod-auth.
    #[must_use]
    pub fn password_account(&self) -> Option<PasswordAccountApi<'_>> {
        match self {
            Self::Official(_) => None,
            Self::OpenScreeps(api) => Some(PasswordAccountApi::OpenScreeps(api)),
            Self::Community(api) => api
                .screepsmod_auth()
                .map(PasswordAccountApi::ScreepsmodAuth),
        }
    }

    /// Persistent API tokens, when the server offers them ([`Subsystem::ApiTokens`]):
    /// screeps.com, and community servers with screepsmod-auth.
    #[must_use]
    pub fn api_tokens(&self) -> Option<ApiTokensApi<'_>> {
        match self {
            Self::Official(api) => Some(ApiTokensApi::Official(api)),
            Self::OpenScreeps(_) => None,
            Self::Community(api) => api.screepsmod_auth().map(ApiTokensApi::ScreepsmodAuth),
        }
    }

    /// How the rate limits of the API token `token` are lifted, when the server lets them be
    /// ([`Subsystem::RateLimitLift`]): on screeps.com's reset page ([`RateLimitLift::Page`],
    /// a reCAPTCHA), with screepsmod-auth by one request ([`RateLimitLift::Request`]). Only
    /// the token's first [`LIFT_TOKEN_PREFIX`] characters are used; `None` as well for a token
    /// no longer than that, which would go out whole. Session tokens (from a password sign-in)
    /// match no API token: lifting applies to API-token sessions only.
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
/// ([`Subsystem::RateLimitLift`]); from [`Api::rate_limit_lift`]. Either way the lift lasts
/// about two hours, and the token's entry in [`ApiTokens::auth_tokens`] tells until when
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

impl PasswordSignIn for PasswordSignInApi<'_> {
    async fn sign_in(&self, email: &str, password: &str) -> Result<Token, Error> {
        match self {
            Self::Official(api) => api.sign_in(email, password).await,
            Self::OpenScreeps(api) => api.sign_in(email, password).await,
            Self::ScreepsmodAuth(api) => api.sign_in(email, password).await,
        }
    }

    fn accepts_username(&self) -> bool {
        match self {
            Self::Official(api) => api.accepts_username(),
            Self::OpenScreeps(api) => api.accepts_username(),
            Self::ScreepsmodAuth(api) => api.accepts_username(),
        }
    }
}

/// [`PasswordSignIn`] for an API holding `client`, sending `$contract`'s endpoint, whose
/// `email` field takes a username too when `$username`. The sign-in itself must not sign in:
/// it is sent with `send_without_sign_in`.
macro_rules! password_sign_in {
    ($api:ty, $contract:ident, $username:literal) => {
        impl PasswordSignIn for $api {
            async fn sign_in(&self, email: &str, password: &str) -> Result<Token, Error> {
                let sign_in = $contract::endpoints::AuthSignin {
                    body: $contract::types::AuthSigninRequest {
                        email: email.to_owned(),
                        password: password.to_owned(),
                        recaptcha: None,
                        steam_ticket: None,
                    },
                };
                let reply = self.client.send_without_sign_in(&sign_in).await?;
                Ok(Token::new(reply.body.token))
            }

            fn accepts_username(&self) -> bool {
                $username
            }
        }
    };
}

password_sign_in!(Official, official, false);
password_sign_in!(OpenScreeps, openscreeps, true);
password_sign_in!(ScreepsmodAuth<'_>, screepsmod_auth, true);

impl PasswordAccount for PasswordAccountApi<'_> {
    async fn register(&self, registration: Registration) -> Result<(), Error> {
        match self {
            Self::OpenScreeps(api) => api.register(registration).await,
            Self::ScreepsmodAuth(api) => api.register(registration).await,
        }
    }

    async fn set_password(&self, old_password: Option<&str>, password: &str) -> Result<(), Error> {
        match self {
            Self::OpenScreeps(api) => api.set_password(old_password, password).await,
            Self::ScreepsmodAuth(api) => api.set_password(old_password, password).await,
        }
    }
}

impl PasswordAccount for OpenScreeps {
    async fn register(&self, registration: Registration) -> Result<(), Error> {
        let submit = openscreeps::endpoints::RegisterSubmit {
            body: registration.into(),
        };
        self.client.send(&submit).await?;
        Ok(())
    }

    async fn set_password(&self, old_password: Option<&str>, password: &str) -> Result<(), Error> {
        let set = openscreeps::endpoints::UserPasswordSet {
            body: openscreeps::types::UserPasswordSetRequest {
                old_password: old_password.unwrap_or_default().to_owned(),
                password: password.to_owned(),
            },
        };
        match self.client.send(&set).await {
            Ok(_) => Ok(()),
            // `OpenScreeps`' answer to a wrong `oldPassword` (`AccountsService::setPassword`,
            // <https://github.com/openscreeps/server/blob/cad1ca9d365320349dbe81fea2aa384c05c2727f/src/services/accounts.ts#L626-L637>).
            Err(Error::Api(ApiError::Server(message))) if message == "invalid password" => {
                Err(Error::WrongPassword)
            }
            Err(error) => Err(error),
        }
    }
}

impl PasswordAccount for ScreepsmodAuth<'_> {
    async fn register(&self, registration: Registration) -> Result<(), Error> {
        let submit = screepsmod_auth::endpoints::RegisterSubmit {
            body: registration.into(),
        };
        self.client.send(&submit).await?;
        Ok(())
    }

    async fn set_password(&self, old_password: Option<&str>, password: &str) -> Result<(), Error> {
        // The plugin's contract: never send a wrong `oldPassword` (it answers HTTP 401, then
        // stores an undefined password and crashes the backend). It checks the current
        // password only when the account has one.
        let me: Profile = self
            .client
            .send(&community::endpoints::AuthMe)
            .await?
            .body
            .into();
        if me.has_password != Some(false) {
            let old_password = old_password.ok_or(Error::WrongPassword)?;
            let name =
                me.username
                    .as_deref()
                    .or(me.email.as_deref())
                    .ok_or(Error::UnexpectedReply(
                        "an account without username or e-mail",
                    ))?;
            match self.sign_in(name, old_password).await {
                Ok(_) => {}
                Err(Error::Api(ApiError::Unauthorized)) => return Err(Error::WrongPassword),
                Err(error) => return Err(error),
            }
        }
        let set = screepsmod_auth::endpoints::UserPasswordSet {
            body: screepsmod_auth::types::UserPasswordSetRequest {
                old_password: old_password.map(str::to_owned),
                password: password.to_owned(),
            },
        };
        self.client.send(&set).await?;
        Ok(())
    }
}

/// Persistent API tokens of the account ([`Subsystem::ApiTokens`]): native on screeps.com,
/// provided by screepsmod-auth on community servers. From [`Api::api_tokens`].
pub trait ApiTokens {
    /// The account's API tokens (`GET /api/user/auth-token`). Entries hold the first 8
    /// characters of each token on screeps.com.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn auth_tokens(&self) -> impl Future<Output = Result<Vec<AuthToken>, Error>>;

    /// Generates an API token with the permissions of `request`
    /// (`POST /api/user/auth-token`). The token is returned only this once.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn create_auth_token(
        &self,
        request: AuthTokenRequest,
    ) -> impl Future<Output = Result<Token, Error>>;

    /// Revokes the API token whose entry id is `id` ([`AuthToken::id`])
    /// (`POST /api/user/delete-auth-token`).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Client::send`](crate::Client::send).
    fn delete_auth_token(&self, id: &str) -> impl Future<Output = Result<(), Error>>;
}

/// The API of a server offering [`ApiTokens`]; from [`Api::api_tokens`].
#[derive(Clone, Copy, Debug)]
pub enum ApiTokensApi<'a> {
    /// Native on screeps.com.
    Official(&'a Official),
    /// screepsmod-auth on a community server.
    ScreepsmodAuth(ScreepsmodAuth<'a>),
}

impl ApiTokens for ApiTokensApi<'_> {
    async fn auth_tokens(&self) -> Result<Vec<AuthToken>, Error> {
        match self {
            Self::Official(api) => api.auth_tokens().await,
            Self::ScreepsmodAuth(api) => api.auth_tokens().await,
        }
    }

    async fn create_auth_token(&self, request: AuthTokenRequest) -> Result<Token, Error> {
        match self {
            Self::Official(api) => api.create_auth_token(request).await,
            Self::ScreepsmodAuth(api) => api.create_auth_token(request).await,
        }
    }

    async fn delete_auth_token(&self, id: &str) -> Result<(), Error> {
        match self {
            Self::Official(api) => api.delete_auth_token(id).await,
            Self::ScreepsmodAuth(api) => api.delete_auth_token(id).await,
        }
    }
}

impl ApiTokens for Official {
    async fn auth_tokens(&self) -> Result<Vec<AuthToken>, Error> {
        let reply = self
            .client
            .send(&official::endpoints::UserAuthTokenList)
            .await?;
        let tokens = reply.body.tokens.unwrap_or_default();
        Ok(tokens.into_iter().map(AuthToken::from).collect())
    }

    async fn create_auth_token(&self, request: AuthTokenRequest) -> Result<Token, Error> {
        let create = official::endpoints::UserAuthTokenCreate {
            body: request.into(),
        };
        Ok(Token::new(self.client.send(&create).await?.body.token))
    }

    async fn delete_auth_token(&self, id: &str) -> Result<(), Error> {
        let delete = official::endpoints::UserAuthTokenDelete {
            body: official::types::UserAuthTokenDeleteRequest {
                token_id: id.to_owned(),
            },
        };
        self.client.send(&delete).await?;
        Ok(())
    }
}

impl ApiTokens for ScreepsmodAuth<'_> {
    async fn auth_tokens(&self) -> Result<Vec<AuthToken>, Error> {
        let reply = self
            .client
            .send(&screepsmod_auth::endpoints::UserAuthTokenList)
            .await?;
        let tokens = reply.body.tokens.unwrap_or_default();
        Ok(tokens.into_iter().map(AuthToken::from).collect())
    }

    async fn create_auth_token(&self, request: AuthTokenRequest) -> Result<Token, Error> {
        let create = screepsmod_auth::endpoints::UserAuthTokenCreate {
            body: request.into(),
        };
        Ok(Token::new(self.client.send(&create).await?.body.token))
    }

    async fn delete_auth_token(&self, id: &str) -> Result<(), Error> {
        let delete = screepsmod_auth::endpoints::UserAuthTokenDelete {
            body: screepsmod_auth::types::UserAuthTokenDeleteRequest {
                token_id: id.to_owned(),
            },
        };
        self.client.send(&delete).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use overseer_core::api::{ApiError, Reply};
    use overseer_core::model::account::Availability;
    use overseer_core::model::platform::{Plugins, ServerKind};
    use overseer_core::{ServerUrl, Token};

    use super::{EXISTS, RateLimitLift, USER_EXISTS, availability};
    use crate::api::Api;
    use crate::{Client, Error};

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
