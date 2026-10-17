//! The API tokens subsystem ([`ApiTokens`]): the trait, the implementations for each server
//! and [`Api::api_tokens`], which selects the implementation.

use overseer_core::Token;
use overseer_core::api::official;
use overseer_core::api::plugins::screepsmod_auth;
use overseer_core::model::account::{AuthToken, AuthTokenRequest};

use super::super::{Api, Official, ScreepsmodAuth};
use crate::Error;

/// Persistent API tokens of the account
/// ([`Subsystem::ApiTokens`](crate::model::platform::Subsystem::ApiTokens)): native on
/// screeps.com, provided by screepsmod-auth on community servers. From [`Api::api_tokens`].
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

impl Api {
    /// Persistent API tokens, when the server offers them
    /// ([`Subsystem::ApiTokens`](crate::model::platform::Subsystem::ApiTokens)): screeps.com,
    /// and community servers with screepsmod-auth.
    #[must_use]
    pub fn api_tokens(&self) -> Option<ApiTokensApi<'_>> {
        match self {
            Self::Official(api) => Some(ApiTokensApi::Official(api)),
            Self::OpenScreeps(_) => None,
            Self::Community(api) => api.screepsmod_auth().map(ApiTokensApi::ScreepsmodAuth),
        }
    }
}
