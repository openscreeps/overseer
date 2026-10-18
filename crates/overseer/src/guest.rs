//! A server without an account: discovery before sign-in, the checks of a new username or
//! e-mail address, and registration.

use overseer_net::api::{AccountApi as _, PasswordAccount as _};
use overseer_net::model::account::{Availability, Registration, check_email, check_username};
use overseer_net::model::platform::{ServerKind, Subsystem};
use overseer_net::{Api, Client};

use crate::Error;
use crate::core::ServerUrl;

/// The operations of a server that need no account: what the server offers (for a sign-in
/// screen), whether a username or an e-mail address is free, and registration.
///
/// Created by [`Guest::discover`] before a session exists, or by
/// [`Overseer::guest`](crate::Overseer::guest) for the server of a session. Cheap to clone. A
/// new account signs in with [`Overseer::connect`](crate::Overseer::connect) after
/// [`Guest::register`].
#[derive(Clone, Debug)]
pub struct Guest {
    api: Api,
}

impl Guest {
    /// Discovers the server with base URL `url` without an account (`GET /api/version`, and
    /// `GET /api/authmod` on community servers), sending `server_password` as
    /// `X-Server-Password` when given and forcing the contract `server_kind` unless `None`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] when the client cannot be built or the server cannot be
    /// discovered; a missing or wrong server password has the refusal
    /// [`Refusal::ServerPassword`](crate::core::api::Refusal::ServerPassword)
    /// ([`Error::refusal`]).
    pub async fn discover(
        url: ServerUrl,
        server_password: Option<String>,
        server_kind: Option<ServerKind>,
    ) -> Result<Self, Error> {
        let mut builder = Client::builder(url);
        if let Some(password) = server_password {
            builder = builder.server_password(password);
        }
        if let Some(kind) = server_kind {
            builder = builder.server_kind(kind);
        }
        let api = builder.build()?.api().await?;
        Ok(Self { api })
    }

    pub(crate) const fn new(api: Api) -> Self {
        Self { api }
    }

    /// The server's operations in Overseer's types: its subsystems, sign-in methods and
    /// account website ([`overseer_net::api`]).
    #[must_use]
    pub const fn api(&self) -> &Api {
        &self.api
    }

    /// Whether the server registers accounts through its API
    /// ([`Subsystem::PasswordAccounts`]).
    #[must_use]
    pub fn can_register(&self) -> bool {
        self.api.password_account().is_some()
    }

    /// Whether no account uses `username` (`GET /api/register/check-username`). A username
    /// that breaks [`check_username`] is not sent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for a username that breaks the rule, and [`Error::Net`]
    /// when the request fails.
    pub async fn username_available(&self, username: &str) -> Result<Availability, Error> {
        check_username(username).map_err(|error| Error::InvalidInput(error.to_string()))?;
        Ok(self.api.username_available(username).await?)
    }

    /// Whether no account uses the e-mail address `email` (`GET /api/register/check-email`).
    /// An address that breaks [`check_email`] is not sent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for an address that breaks the rule, and [`Error::Net`]
    /// when the request fails.
    pub async fn email_available(&self, email: &str) -> Result<Availability, Error> {
        check_email(email).map_err(|error| Error::InvalidInput(error.to_string()))?;
        Ok(self.api.email_available(email).await?)
    }

    /// Registers the account `registration` (`POST /api/register/submit`). Registering issues
    /// no token: the account signs in afterwards with its username and password. Fields that
    /// break a rule ([`Registration::errors`]) are not sent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] for a field that breaks a rule,
    /// [`overseer_net::Error::Unsupported`] of [`Subsystem::PasswordAccounts`] (in
    /// [`Error::Net`]) when the server does not register accounts, and [`Error::Net`] when the
    /// server rejects the registration (its error string as-is).
    pub async fn register(&self, registration: Registration) -> Result<(), Error> {
        let errors = registration.errors();
        if let Some(error) = errors
            .username
            .map(|error| error.to_string())
            .or_else(|| errors.email.map(|error| error.to_string()))
            .or_else(|| errors.password.map(|error| error.to_string()))
        {
            return Err(Error::InvalidInput(error));
        }
        let Some(accounts) = self.api.password_account() else {
            return Err(overseer_net::Error::Unsupported(Subsystem::PasswordAccounts).into());
        };
        Ok(accounts.register(registration).await?)
    }
}
