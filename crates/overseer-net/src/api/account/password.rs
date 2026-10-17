//! The password subsystems: password sign-in ([`PasswordSignIn`]) and password accounts
//! ([`PasswordAccount`]: registration and password changes). This module holds the traits, the
//! implementations for each server and the [`Api`] methods that select them.

use overseer_core::Token;
use overseer_core::api::plugins::screepsmod_auth;
use overseer_core::api::{ApiError, community, official, openscreeps};
use overseer_core::model::account::{Profile, Registration};

use super::super::{Api, Official, OpenScreeps, ScreepsmodAuth};
use crate::Error;

/// Signing in with an e-mail address (or a username) and a password
/// ([`Subsystem::PasswordSignIn`](crate::model::platform::Subsystem::PasswordSignIn)): native
/// on screeps.com and `OpenScreeps`, provided by screepsmod-auth on community servers. From
/// [`Api::password_sign_in`].
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

/// Accounts of the server's own with a password
/// ([`Subsystem::PasswordAccounts`](crate::model::platform::Subsystem::PasswordAccounts)):
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
    /// ([`Subsystem::PasswordSignIn`](crate::model::platform::Subsystem::PasswordSignIn)):
    /// screeps.com, `OpenScreeps`, and community servers with screepsmod-auth.
    #[must_use]
    pub fn password_sign_in(&self) -> Option<PasswordSignInApi<'_>> {
        match self {
            Self::Official(api) => Some(PasswordSignInApi::Official(api)),
            Self::OpenScreeps(api) => Some(PasswordSignInApi::OpenScreeps(api)),
            Self::Community(api) => api.screepsmod_auth().map(PasswordSignInApi::ScreepsmodAuth),
        }
    }

    /// Registration and password changes ([`PasswordAccount`]), when the server offers them
    /// ([`Subsystem::PasswordAccounts`](crate::model::platform::Subsystem::PasswordAccounts)):
    /// `OpenScreeps`, and community servers with screepsmod-auth.
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
