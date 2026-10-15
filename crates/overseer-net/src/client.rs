//! The shared HTTP client: token, sign-in, rate limits and world discovery.

mod flows;
mod rate_limit;

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use http::HeaderValue;
use overseer_core::api::{
    self, ApiError, Auth, Endpoint, RateLimit, Reply, RequestError, Terrain, endpoints, types,
};
use overseer_core::{ServerUrl, Token};
use reqwest_websocket::{Upgrade, WebSocket};

pub use rate_limit::{EndpointRateLimit, RateLimitPolicy};

use crate::socket::{ReconnectPolicy, Socket};
use crate::{Error, USER_AGENT, World};
use rate_limit::RateLimits;

/// HTTP API client and socket connector for one account on one server.
///
/// Clones share the token, the sign-in state, the rate limits, the terrain cache and the cached
/// [`World`].
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    server: ServerUrl,
    user_agent: HeaderValue,
    /// Server, current token and server password. Locked only to copy or update it, never
    /// across an `.await`.
    session: Mutex<api::Session>,
    /// The session's token, published to [`Client::token_updates`] on every change.
    tokens: tokio::sync::watch::Sender<Option<Token>>,
    credentials: Option<Credentials>,
    /// Serializes sign-ins, so concurrent rejections sign in once.
    sign_in: tokio::sync::Mutex<()>,
    world: Mutex<Option<World>>,
    default_shard: Option<String>,
    reconnect: ReconnectPolicy,
    keep_alive: Option<Duration>,
    rate_limit: Option<RateLimitPolicy>,
    rate_limits: Mutex<RateLimits>,
    terrain: Mutex<TerrainCache>,
}

/// Decoded terrain by `(shard, room)`; terrain never changes.
type TerrainCache = HashMap<(Option<String>, String), Arc<Terrain>>;

/// E-mail and password for servers with password sign-in.
#[derive(Clone)]
struct Credentials {
    email: String,
    password: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Credentials(<redacted>)")
    }
}

impl Client {
    /// Starts configuring a client for `server`.
    pub fn builder(server: ServerUrl) -> ClientBuilder {
        ClientBuilder {
            server,
            token: None,
            credentials: None,
            server_password: None,
            http: None,
            reconnect: ReconnectPolicy::default(),
            keep_alive: Some(Duration::from_secs(30)),
            rate_limit: Some(RateLimitPolicy::default()),
            user_agent: None,
            default_shard: None,
        }
    }

    /// The server.
    #[must_use]
    pub fn server(&self) -> &ServerUrl {
        &self.inner.server
    }

    /// The current token, including tokens rotated by the server or issued by a sign-in.
    #[must_use]
    pub fn token(&self) -> Option<Token> {
        self.session().token().cloned()
    }

    /// Replaces the token used by every clone of this client and by future socket connections.
    pub fn set_token(&self, token: Option<Token>) {
        let mut session = self.session();
        session.set_token(token.clone());
        // Published under the session lock, so watchers see changes in the order they were made.
        self.inner.tokens.send_if_modified(|current| {
            let changed = *current != token;
            *current = token;
            changed
        });
    }

    /// Every change of the shared token: [`Self::set_token`], `X-Token` rotations, sign-ins and
    /// the socket's `auth ok` tokens. The receiver starts at the current token; persist each new
    /// value if you store tokens.
    #[must_use]
    pub fn token_updates(&self) -> tokio::sync::watch::Receiver<Option<Token>> {
        self.inner.tokens.subscribe()
    }

    /// Executes an endpoint; a rotated `X-Token` replaces the shared token.
    ///
    /// With credentials, signs in first when the endpoint takes a token and none is held, and
    /// signs in again and retries once when the server answers 401. Requests rejected with
    /// HTTP 429 are retried once per the [`RateLimitPolicy`]; the reported limits are kept for
    /// [`Self::rate_limit`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Request`] when the request cannot be built (for example a missing token),
    /// [`Error::Http`] for transport failures and [`Error::Api`] for error replies, including
    /// those of a needed sign-in and [`ApiError::RateLimited`] when a retry is not allowed or
    /// fails again.
    pub async fn send<E: Endpoint + Sync>(
        &self,
        endpoint: &E,
    ) -> Result<Reply<E::Response>, Error> {
        if let Some(credentials) = &self.inner.credentials
            && E::AUTH != Auth::None
            && self.token().is_none()
        {
            self.renew_token(credentials, None).await?;
        }
        let (used, result) = self.execute(endpoint).await;
        match (result, &self.inner.credentials) {
            (Err(Error::Api(ApiError::Unauthorized)), Some(credentials)) => {
                self.renew_token(credentials, used.as_ref()).await?;
                self.execute(endpoint).await.1
            }
            (result, _) => result,
        }
    }

    /// The latest rate limit the server reported for endpoint `E` (sent with authenticated
    /// replies and with HTTP 429), for showing remaining quota.
    #[must_use]
    pub fn rate_limit<E: Endpoint>(&self) -> Option<RateLimit> {
        lock(&self.inner.rate_limits).get::<E>()
    }

    /// The latest rate limit of every endpoint that reported one, ordered by path and method.
    #[must_use]
    pub fn rate_limits(&self) -> Vec<EndpointRateLimit> {
        lock(&self.inner.rate_limits).all()
    }

    /// The server's shards and capabilities and the signed-in user, fetched once and cached.
    ///
    /// Requests `GET /api/version`, and `GET /api/auth/me` when the client holds a token or
    /// credentials. A cached anonymous world is fetched again once the client can authenticate.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::send`].
    pub async fn world(&self) -> Result<World, Error> {
        let authenticated = self.inner.credentials.is_some() || self.token().is_some();
        if let Some(world) = lock(&self.inner.world)
            .as_ref()
            .filter(|world| world.user().is_some() || !authenticated)
        {
            return Ok(world.clone());
        }
        let version = self.send(&endpoints::PlatformVersion).await?.body;
        let user = if authenticated {
            Some(self.send(&endpoints::AuthMe).await?.body.id)
        } else {
            None
        };
        let mut world = World::new(version, user);
        if let Some(shard) = &self.inner.default_shard {
            world.set_shard(shard.clone());
        }
        *lock(&self.inner.world) = Some(world.clone());
        Ok(world)
    }

    /// Opens a socket connection for the [`World`] ([`Self::world`]), authenticated with the
    /// current token (anonymous without one).
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Self::world`] and [`Error::WebSocket`] when the connection or
    /// upgrade fails.
    pub async fn connect(&self) -> Result<Socket, Error> {
        let world = self.world().await?;
        let websocket = self.open().await?;
        Ok(Socket::new(self.clone(), world, websocket))
    }

    /// Opens a WebSocket to the server's socket endpoint.
    pub(crate) async fn open(&self) -> Result<WebSocket, Error> {
        Ok(self
            .inner
            .http
            .get(self.inner.server.socket().clone())
            .header(http::header::USER_AGENT, self.inner.user_agent.clone())
            .upgrade()
            .send()
            .await?
            .into_websocket()
            .await?)
    }

    pub(crate) fn reconnect_policy(&self) -> &ReconnectPolicy {
        &self.inner.reconnect
    }

    pub(crate) fn keep_alive(&self) -> Option<Duration> {
        self.inner.keep_alive
    }

    pub(crate) fn can_sign_in(&self) -> bool {
        self.inner.credentials.is_some()
    }

    /// Signs in with the credentials unless the token already differs from `rejected`; returns
    /// the token to use. Without credentials, fails with [`ApiError::Unauthorized`].
    pub(crate) async fn renew(&self, rejected: Option<&Token>) -> Result<Token, Error> {
        match &self.inner.credentials {
            Some(credentials) => self.renew_token(credentials, rejected).await,
            None => Err(ApiError::Unauthorized.into()),
        }
    }

    async fn renew_token(
        &self,
        credentials: &Credentials,
        rejected: Option<&Token>,
    ) -> Result<Token, Error> {
        let _signing_in = self.inner.sign_in.lock().await;
        if let Some(current) = self.token()
            && Some(&current) != rejected
        {
            return Ok(current);
        }
        let sign_in = endpoints::AuthSignin {
            body: types::AuthSigninRequest {
                email: credentials.email.clone(),
                password: credentials.password.clone(),
                recaptcha: None,
                steam_ticket: None,
            },
        };
        let token = Token::new(self.execute(&sign_in).await.1?.body.token);
        self.set_token(Some(token.clone()));
        Ok(token)
    }

    /// The shard a request names: `shard`, else the [`World`]'s default shard; `None` on
    /// single-world servers.
    pub(crate) async fn resolve_shard(&self, shard: Option<&str>) -> Result<Option<String>, Error> {
        Ok(self.world().await?.resolve_shard(shard))
    }

    /// Sends a request, retrying once after HTTP 429 when the policy allows; returns the token
    /// the last attempt used.
    async fn execute<E: Endpoint + Sync>(
        &self,
        endpoint: &E,
    ) -> (Option<Token>, Result<Reply<E::Response>, Error>) {
        let (used, result) = self.attempt(endpoint).await;
        if let (Err(Error::Api(ApiError::RateLimited(limit))), Some(policy)) =
            (&result, &self.inner.rate_limit)
            && let Some(wait) = policy.retry_delay(limit.as_ref(), SystemTime::now())
        {
            tokio::time::sleep(wait).await;
            return self.attempt(endpoint).await;
        }
        (used, result)
    }

    /// Sends one request with a copy of the session and records its rate limit; returns the
    /// token it used.
    async fn attempt<E: Endpoint + Sync>(
        &self,
        endpoint: &E,
    ) -> (Option<Token>, Result<Reply<E::Response>, Error>) {
        let mut session = self.session().clone();
        let used = session.token().cloned();
        let result = self.exchange(&mut session, endpoint).await;
        if session.token() != used.as_ref() {
            self.set_token(session.token().cloned());
        }
        let limit = match &result {
            Ok(reply) => reply.rate_limit,
            Err(Error::Api(ApiError::RateLimited(limit))) => *limit,
            Err(_) => None,
        };
        if let Some(limit) = limit {
            lock(&self.inner.rate_limits).record::<E>(limit);
        }
        (used, result)
    }

    async fn exchange<E: Endpoint + Sync>(
        &self,
        session: &mut api::Session,
        endpoint: &E,
    ) -> Result<Reply<E::Response>, Error> {
        let mut request = reqwest::Request::try_from(session.request(endpoint)?)?;
        request
            .headers_mut()
            .insert(http::header::USER_AGENT, self.inner.user_agent.clone());
        let mut response = self.inner.http.execute(request).await?;
        let status = response.status();
        let headers = std::mem::take(response.headers_mut());
        let body = response.bytes().await?;
        let mut reply = http::Response::new(body);
        *reply.status_mut() = status;
        *reply.headers_mut() = headers;
        Ok(session.response::<E, _>(reply)?)
    }

    fn session(&self) -> MutexGuard<'_, api::Session> {
        lock(&self.inner.session)
    }
}

/// Locks a mutex whose critical sections cannot panic halfway through an update.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Configures a [`Client`]; created by [`Client::builder`].
#[must_use]
pub struct ClientBuilder {
    server: ServerUrl,
    token: Option<Token>,
    credentials: Option<Credentials>,
    server_password: Option<String>,
    http: Option<reqwest::Client>,
    reconnect: ReconnectPolicy,
    keep_alive: Option<Duration>,
    rate_limit: Option<RateLimitPolicy>,
    user_agent: Option<String>,
    default_shard: Option<String>,
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("server", &self.server)
            .field("reconnect", &self.reconnect)
            .field("keep_alive", &self.keep_alive)
            .field("rate_limit", &self.rate_limit)
            .finish_non_exhaustive()
    }
}

impl ClientBuilder {
    /// Authenticates with an account API token or a session token.
    pub fn token(mut self, token: Token) -> Self {
        self.token = Some(token);
        self
    }

    /// Signs in with e-mail and password (`POST /api/auth/signin`, private servers) when a token
    /// is needed and none is held, and again when the server rejects the token.
    pub fn credentials(mut self, email: impl Into<String>, password: impl Into<String>) -> Self {
        self.credentials = Some(Credentials {
            email: email.into(),
            password: password.into(),
        });
        self
    }

    /// Sends `X-Server-Password` to a password-protected private server.
    pub fn server_password(mut self, password: impl Into<String>) -> Self {
        self.server_password = Some(password.into());
        self
    }

    /// Uses a caller-configured `reqwest` client. It must allow HTTP/1.1 for socket upgrades and
    /// should not follow redirects (tokens must never follow a redirect). The default client is
    /// HTTP/1.1-only, follows no redirects and has a 10 s connect timeout.
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// How a dropped socket connection is re-established; [`ReconnectPolicy::default`] when not
    /// set.
    pub const fn reconnect(mut self, policy: ReconnectPolicy) -> Self {
        self.reconnect = policy;
        self
    }

    /// Interval of socket keep-alive pings (default 30 s); `None` disables them. A connection
    /// that receives no frame for two intervals is considered dead and reconnected.
    pub const fn keep_alive(mut self, interval: Option<Duration>) -> Self {
        self.keep_alive = interval;
        self
    }

    /// How requests rejected with HTTP 429 are retried ([`RateLimitPolicy::default`] when not
    /// set); `None` returns [`ApiError::RateLimited`] without retrying.
    pub const fn rate_limit(mut self, policy: Option<RateLimitPolicy>) -> Self {
        self.rate_limit = policy;
        self
    }

    /// The `User-Agent` of every request and socket upgrade (default [`USER_AGENT`]); it
    /// replaces one configured on a custom `reqwest` client.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    /// The shard socket helpers use when none is named, instead of the first shard listed.
    /// Ignored by single-world servers.
    pub fn default_shard(mut self, shard: impl Into<String>) -> Self {
        self.default_shard = Some(shard.into());
        self
    }

    /// Builds the client.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Request`] for a user agent that is not a valid header value and
    /// [`Error::Http`] when the default `reqwest` client's TLS backend cannot be initialised.
    ///
    /// TLS uses rustls with the `ring` provider, installed as the process default unless the
    /// application installed another provider first.
    pub fn build(self) -> Result<Client, Error> {
        // Err only means a provider is already installed, which then stays in use.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let user_agent = HeaderValue::try_from(self.user_agent.as_deref().unwrap_or(USER_AGENT))
            .map_err(|error| RequestError::Http(error.into()))?;
        let http = match self.http {
            Some(http) => http,
            None => reqwest::Client::builder()
                .http1_only()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .build()?,
        };
        let tokens = tokio::sync::watch::Sender::new(self.token.clone());
        let mut session = api::Session::new(self.server.clone(), self.token);
        session.set_server_password(self.server_password);
        Ok(Client {
            inner: Arc::new(Inner {
                http,
                server: self.server,
                user_agent,
                session: Mutex::new(session),
                tokens,
                credentials: self.credentials,
                sign_in: tokio::sync::Mutex::new(()),
                world: Mutex::new(None),
                default_shard: self.default_shard,
                reconnect: self.reconnect,
                keep_alive: self.keep_alive,
                rate_limit: self.rate_limit,
                rate_limits: Mutex::new(RateLimits::default()),
                terrain: Mutex::new(HashMap::new()),
            }),
        })
    }
}
